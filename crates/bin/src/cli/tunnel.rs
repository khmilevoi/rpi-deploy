use std::time::Duration;

use crate::cli::config::ServerProfile;

pub const AGENT_SOCKET: &str = "/run/pi/agent.sock";

pub struct SshTunnel {
    child: Option<tokio::process::Child>,
    pub base_url: String,
}

impl SshTunnel {
    pub async fn open(profile: &ServerProfile) -> anyhow::Result<SshTunnel> {
        if let Ok(url) = std::env::var("PI_AGENT_URL") {
            return Ok(SshTunnel { child: None, base_url: url });
        }

        let port = free_local_port()?;
        let mut cmd = tokio::process::Command::new("ssh");
        if let Some(key) = &profile.key {
            cmd.arg("-i").arg(expand_home(key));
        }
        cmd.args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ExitOnForwardFailure=yes",
            "-L",
            &format!("{port}:{AGENT_SOCKET}"),
            "-N",
            &format!("{}@{}", profile.user, profile.host),
        ]);
        cmd.stdin(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::inherit());
        let child = cmd.spawn().map_err(|e| anyhow::anyhow!("cannot spawn ssh: {e}"))?;

        wait_port(port, Duration::from_secs(10)).await?;
        Ok(SshTunnel { child: Some(child), base_url: format!("http://127.0.0.1:{port}") })
    }
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.start_kill();
        }
    }
}

fn free_local_port() -> anyhow::Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

fn expand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest).display().to_string(),
        _ => path.to_string(),
    }
}

async fn wait_port(port: u16, budget: Duration) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "ssh tunnel did not come up on 127.0.0.1:{port} within {budget:?}; check `ssh` access to the Pi"
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
