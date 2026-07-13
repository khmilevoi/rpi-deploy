//! `rpi agent update` — board-side, runs under sudo. Obtains a fresh rpi
//! binary (npm channel or GitHub-direct download) and applies it through the
//! same swap+setup+restart path as `agent setup`. See
//! docs/superpowers/specs/2026-07-13-rpi-remote-agent-update-design.md.

use super::self_install::{self, SelfInstallAction};
use super::setup::{self, HostSys, SetupOpts, Sys};
use std::path::{Path, PathBuf};

/// Resolve the source binary for `version`. npm branch when the login user's
/// global npm has `rpi-deploy` installed (refresh it to `@version`); otherwise
/// download + verify the GitHub release archive from `base_url`.
#[allow(dead_code)]
pub(crate) async fn obtain_source_binary(
    sys: &dyn Sys,
    login_user: &str,
    base_url: &str,
    version: &str,
    workdir: &str,
) -> anyhow::Result<PathBuf> {
    if setup::resolve_npm_dist_binary(sys, login_user)
        .await
        .is_some()
    {
        sys.run(
            "sudo",
            &[
                "-u",
                login_user,
                "-i",
                "--",
                "npm",
                "i",
                "-g",
                &format!("rpi-deploy@{version}"),
            ],
        )
        .await
        .map_err(|e| anyhow::anyhow!("npm i -g rpi-deploy@{version}: {e}"))?;
        return setup::resolve_npm_dist_binary(sys, login_user)
            .await
            .ok_or_else(|| anyhow::anyhow!("npm install succeeded but dist/rpi not found"));
    }
    super::release::download_verified_binary(sys, base_url, version, workdir)
        .await
        .map_err(|e| anyhow::anyhow!(e))
}

/// CLI entrypoint for `rpi agent update`. Must run as root (under sudo).
#[allow(dead_code)]
pub async fn run_cmd(
    user: Option<String>,
    version: Option<String>,
    dry_run: bool,
) -> anyhow::Result<()> {
    let login_user = user
        .or_else(|| std::env::var("SUDO_USER").ok())
        .filter(|u| !u.is_empty() && u != "root")
        .ok_or_else(|| {
            anyhow::anyhow!(
                "cannot determine the SSH login user; run via `sudo rpi agent update` or pass --user <name>"
            )
        })?;

    // Read the injectable base/api URLs from env exactly once, here, so the
    // downstream Sys-driven helpers stay env-free and unit-testable.
    let base_url = super::release::release_base_url();
    let api_base = super::release::api_base_url();

    let sys = HostSys;
    let version = match version {
        Some(v) => v.trim_start_matches('v').to_string(),
        None => super::release::resolve_latest_version(&sys, &api_base)
            .await
            .map_err(|e| anyhow::anyhow!(e))?,
    };
    crate::output::info(format!("updating agent to v{version}"));

    if dry_run {
        let channel = if setup::resolve_npm_dist_binary(&sys, &login_user)
            .await
            .is_some()
        {
            "npm"
        } else {
            "github-direct"
        };
        crate::output::info(format!(
            "would update via the {channel} channel to v{version} (dry run — no changes made)"
        ));
        return Ok(());
    }

    let workdir = super::release::make_tempdir(&sys)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    let apply = apply_update(&sys, &login_user, &base_url, &version, &workdir).await;
    // Best-effort cleanup regardless of outcome.
    let _ = sys.run("rm", &["-rf", &workdir]).await;
    apply
}

/// Swap in the source binary, re-run the idempotent setup, and restart the
/// agent when the binary actually changed.
#[allow(dead_code)]
async fn apply_update(
    sys: &dyn Sys,
    login_user: &str,
    base_url: &str,
    version: &str,
    workdir: &str,
) -> anyhow::Result<()> {
    let source = obtain_source_binary(sys, login_user, base_url, version, workdir).await?;

    let action =
        self_install::ensure_installed(&source, Path::new(self_install::AGENT_BIN_PATH), false)
            .map_err(|e| anyhow::anyhow!("self-install {}: {e}", self_install::AGENT_BIN_PATH))?;

    match &action {
        SelfInstallAction::UpToDate | SelfInstallAction::AlreadyCanonical => {
            crate::output::success(format!(
                "ok (already on the requested binary): {}",
                self_install::AGENT_BIN_PATH
            ));
        }
        SelfInstallAction::Installed => {
            crate::output::success(format!(
                "installed: {} (v{version})",
                self_install::AGENT_BIN_PATH
            ));
        }
    }

    let opts = SetupOpts {
        login_user: login_user.to_string(),
        with_cloudflared: false,
        dry_run: false,
        cf_token: None,
        domain: None,
        tunnel_name: None,
    };
    let report = setup::setup(sys, &opts).await;
    report.print();
    if !report.errors.is_empty() {
        anyhow::bail!(
            "update completed with {} error(s); see above",
            report.errors.len()
        );
    }

    if matches!(action, SelfInstallAction::Installed) {
        if let Some(note) = setup::restart_agent_if_active(sys).await {
            crate::output::info(note);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::setup::fake::FakeSys;
    use std::path::Path;

    /// npm channel: `<npm root>/rpi-deploy/dist/rpi` exists → npm branch runs
    /// `npm i -g rpi-deploy@<version>` and returns the refreshed dist path.
    #[tokio::test]
    async fn obtain_source_uses_npm_branch_when_present() {
        let mut sys = FakeSys::default();
        sys.ok.insert(
            FakeSys::key("sudo", &["-u", "deploy", "-i", "--", "npm", "root", "-g"]),
            "/home/deploy/.npm-global/lib/node_modules".into(),
        );
        let dist = "/home/deploy/.npm-global/lib/node_modules/rpi-deploy/dist/rpi";
        sys.paths.insert(dist.into());
        sys.ok.insert(
            FakeSys::key(
                "sudo",
                &[
                    "-u",
                    "deploy",
                    "-i",
                    "--",
                    "npm",
                    "i",
                    "-g",
                    "rpi-deploy@0.22.0",
                ],
            ),
            String::new(),
        );
        let src = obtain_source_binary(&sys, "deploy", "file:///rel", "0.22.0", "/tmp/wd")
            .await
            .unwrap();
        assert_eq!(src, Path::new(dist));
        assert!(sys
            .calls()
            .iter()
            .any(|c| c.contains("npm i -g rpi-deploy@0.22.0")));
    }

    /// GitHub-direct channel: no npm dist → download+verify path is taken.
    #[tokio::test]
    async fn obtain_source_uses_github_branch_when_no_npm() {
        const BASE: &str = "file:///rel";
        let version = "0.22.0";
        let asset = crate::agent::release::asset_name(version, "aarch64-unknown-linux-musl");
        let work = "/tmp/wd";
        let archive = format!("{work}/{asset}");
        let sums = format!("{work}/SHA256SUMS");
        let hash = "d".repeat(64);
        let mut sys = FakeSys::default();
        // npm root fails → no npm branch
        sys.err.insert(FakeSys::key(
            "sudo",
            &["-u", "deploy", "-i", "--", "npm", "root", "-g"],
        ));
        sys.ok
            .insert(FakeSys::key("uname", &["-m"]), "aarch64".into());
        sys.ok.insert(
            FakeSys::key(
                "curl",
                &[
                    "-fsSL",
                    "-o",
                    &archive,
                    &format!("{BASE}/v{version}/{asset}"),
                ],
            ),
            String::new(),
        );
        sys.ok.insert(
            FakeSys::key(
                "curl",
                &[
                    "-fsSL",
                    "-o",
                    &sums,
                    &format!("{BASE}/v{version}/SHA256SUMS"),
                ],
            ),
            String::new(),
        );
        sys.files.insert(sums.clone(), format!("{hash}  {asset}\n"));
        sys.ok.insert(
            FakeSys::key("sha256sum", &[&archive]),
            format!("{hash}  {archive}"),
        );
        sys.ok.insert(
            FakeSys::key("tar", &["-xf", &archive, "-C", work]),
            String::new(),
        );
        sys.paths.insert(format!("{work}/rpi"));

        let src = obtain_source_binary(&sys, "deploy", BASE, version, work)
            .await
            .unwrap();
        assert_eq!(src, Path::new("/tmp/wd/rpi"));
    }
}
