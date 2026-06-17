use std::path::PathBuf;
use std::sync::Arc;

use crate::agent::config::AgentConfig;
use crate::agent::http::router;
use crate::agent::logfile;
use crate::agent::state::build_state;
use pi_domain::contracts::Clock;
use tracing_subscriber::prelude::*;

pub async fn run(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let config = AgentConfig::load(config_path.as_deref())?;

    let log_dir_available = match std::fs::create_dir_all(&config.logs.dir) {
        Ok(()) => {
            let _ = logfile::prune_old(&config.logs.dir, config.logs.retention_days);
            true
        }
        Err(e) => {
            eprintln!(
                "warning: cannot create log directory {}: {e} – agent logs to stderr only",
                config.logs.dir.display()
            );
            false
        }
    };

    let env_filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    let stderr_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);

    let file_layer: Option<_> = if log_dir_available {
        Some(
            tracing_subscriber::fmt::layer()
                .without_time()
                .with_ansi(false)
                .with_writer(logfile::DailyMakeWriter::new(config.logs.dir.clone())),
        )
    } else {
        None
    };

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stderr_layer)
        .with(file_layer)
        .init();

    let state = build_state(&config, log_dir_available)?;
    let hn = Arc::clone(&state.host_network);
    let ip = tokio::task::spawn_blocking(move || hn.primary_ipv4())
        .await
        .ok()
        .flatten();
    match ip {
        Some(ip) => tracing::info!("lan ip detected: {ip}"),
        None => {
            tracing::warn!("no non-loopback ipv4 detected; lan-exposed projects will log port only")
        }
    }
    let now = pi_infrastructure::sys::SystemClock::new().now_unix();
    let swept = state
        .history
        .sweep_interrupted(now)
        .await
        .map_err(|e| anyhow::anyhow!("startup sweep: {e}"))?;
    if swept > 0 {
        tracing::warn!("marked {swept} unfinished deployment(s) as interrupted (agent restart)");
    }
    let app = router(state);

    // windows
    if let Some(addr) = &config.tcp {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        tracing::info!("pi-agent listening on tcp {addr}");
        axum::serve(listener, app).await?;
        return Ok(());
    }

    // unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(parent) = config.socket.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::remove_file(&config.socket);
        let listener = tokio::net::UnixListener::bind(&config.socket)?;
        std::fs::set_permissions(&config.socket, std::fs::Permissions::from_mode(0o660))?;
        tracing::info!(
            "pi-agent listening on unix socket {}",
            config.socket.display()
        );
        axum::serve(listener, app).await?;
        Ok(())
    }

    #[cfg(not(unix))]
    anyhow::bail!(
        "unix sockets are unsupported on this OS; set `tcp = \"127.0.0.1:7700\"` in agent.toml (dev mode)"
    )
}
