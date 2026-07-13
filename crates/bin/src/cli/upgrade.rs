//! `rpi upgrade` — client-side pult. Triggers the board to update its own rpi
//! binary via `ssh -t <user>@<host> sudo rpi agent update --version <X>`. See
//! docs/superpowers/specs/2026-07-13-rpi-remote-agent-update-design.md.

use crate::cli::api::ApiClient;
use crate::cli::config::{ConnectOpts, ServerProfile};
use crate::cli::prompt::{InquirePrompter, Prompter};
use crate::cli::ssh::SshExec;
use crate::cli::tunnel::SshTunnel;

/// Resolve the version `rpi upgrade` will bring the board to: no flag → the
/// client's own version (keeps the client↔agent pair aligned); `latest` → the
/// newest published release; otherwise the explicit version (leading `v`
/// stripped).
pub async fn resolve_target_version(flag: Option<String>) -> anyhow::Result<String> {
    match flag.as_deref() {
        None => Ok(env!("CARGO_PKG_VERSION").to_string()),
        Some("latest") => github_latest_version().await,
        Some(v) => Ok(v.trim_start_matches('v').to_string()),
    }
}

/// Newest published release version (no leading `v`) via the GitHub API.
async fn github_latest_version() -> anyhow::Result<String> {
    let url = format!("{}/releases/latest", crate::agent::release::api_base_url());
    let body = reqwest::Client::new()
        .get(url)
        .header("User-Agent", "rpi-deploy")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    crate::agent::release::parse_latest_tag(&body).map_err(|e| anyhow::anyhow!(e))
}

/// Read the agent's reported version through a short-lived tunnel + handshake.
async fn read_agent_version(profile: &ServerProfile) -> Option<String> {
    let tunnel = SshTunnel::open(profile).await.ok()?;
    let api = ApiClient::new(tunnel.base_url.clone());
    api.version().await.ok().map(|v| v.version)
}

pub async fn run(version: Option<String>, yes: bool, connect: ConnectOpts) -> anyhow::Result<()> {
    if std::env::var("PI_AGENT_URL").is_ok() {
        anyhow::bail!(
            "rpi upgrade needs SSH access to the board; it is not applicable with PI_AGENT_URL set (local dev)"
        );
    }
    let profile = connect.resolve()?;
    let target = resolve_target_version(version).await?;

    match read_agent_version(&profile).await {
        Some(current) => crate::output::info(format!("agent update: {current} -> {target}")),
        None => crate::output::info(format!("agent update: (current unknown) -> {target}")),
    }

    if !yes {
        let mut p = InquirePrompter;
        if !p.confirm(&format!("update the board to v{target}?"), true)? {
            crate::output::info("aborted");
            return Ok(());
        }
    }

    let ssh = SshExec { profile: &profile };
    ssh.run_tty(&["sudo", "rpi", "agent", "update", "--version", &target])
        .await?;

    match read_agent_version(&profile).await {
        Some(v) if v == target => crate::output::success(format!("board is now on v{v}")),
        Some(v) => crate::output::warn(format!(
            "board reports v{v}, expected v{target} (a restart may still be pending)"
        )),
        None => crate::output::warn("could not read the board version after update"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn target_defaults_to_client_version() {
        let v = resolve_target_version(None).await.unwrap();
        assert_eq!(v, env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn explicit_version_strips_leading_v() {
        assert_eq!(
            resolve_target_version(Some("v0.22.0".into()))
                .await
                .unwrap(),
            "0.22.0"
        );
        assert_eq!(
            resolve_target_version(Some("0.22.0".into())).await.unwrap(),
            "0.22.0"
        );
    }
}
