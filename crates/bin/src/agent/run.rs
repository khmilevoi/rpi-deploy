use std::path::PathBuf;

use crate::agent::config::AgentConfig;
use crate::agent::http::router;
use crate::agent::state::build_state;
use pi_domain::contracts::Clock;

pub async fn run(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let config = AgentConfig::load(config_path.as_deref())?;
    let state = build_state(&config)?;
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
