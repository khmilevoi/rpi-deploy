use std::path::Path;
use crate::agent::setup::{user_exists, HostSys, Sys, UNIT_PATH};

pub struct UninstallOpts {
    pub purge: bool,
}

#[derive(Default)]
pub struct UninstallReport {
    pub removed: Vec<String>,
    pub kept: Vec<String>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

/// Remove the agent service/unit/user. Keeps data dirs unless `purge` (spec §5).
pub async fn uninstall(sys: &dyn Sys, opts: &UninstallOpts) -> UninstallReport {
    let mut rep = UninstallReport::default();

    if sys.run("systemctl", &["disable", "--now", "rpi-agent"]).await.is_err() {
        rep.warnings.push("systemctl disable --now rpi-agent failed (may already be stopped)".into());
    }
    if sys.exists(Path::new(UNIT_PATH)) {
        if sys.run("rm", &["-f", UNIT_PATH, &format!("{UNIT_PATH}.bak")]).await.is_ok() {
            rep.removed.push(UNIT_PATH.into());
        } else {
            rep.warnings.push(format!("failed to remove {UNIT_PATH}"));
        }
    }
    if sys.run("systemctl", &["daemon-reload"]).await.is_err() {
        rep.warnings.push("systemctl daemon-reload failed".into());
    }
    if user_exists(sys, "rpi-agent").await {
        if sys.run("userdel", &["rpi-agent"]).await.is_ok() {
            rep.removed.push("user rpi-agent".into());
        } else {
            rep.warnings.push("userdel rpi-agent failed (user may be in use)".into());
        }
    }

    if opts.purge {
        for dir in ["/var/lib/rpi", "/etc/rpi", "/var/log/rpi"] {
            if sys.exists(Path::new(dir)) {
                if sys.run("rm", &["-rf", dir]).await.is_ok() {
                    rep.removed.push(dir.into());
                } else {
                    rep.errors.push(format!("failed to purge {dir} (data remains on disk)"));
                }
            }
        }
    } else {
        for dir in ["/var/lib/rpi", "/etc/rpi", "/var/log/rpi"] {
            if sys.exists(Path::new(dir)) {
                rep.kept.push(dir.into());
            }
        }
    }
    rep
}

/// CLI entrypoint: confirm when purging, run uninstall, print the report.
pub async fn run_cmd(purge: bool, yes: bool) -> anyhow::Result<()> {
    if purge && !yes {
        anyhow::bail!(
            "--purge deletes /var/lib/rpi (secrets, deploy keys, state) irreversibly. \
             Re-run with --purge --yes to confirm."
        );
    }
    let report = uninstall(&HostSys, &UninstallOpts { purge }).await;
    for r in &report.removed { println!("removed: {r}"); }
    for k in &report.kept { println!("kept: {k}"); }
    for w in &report.warnings { println!("warning: {w}"); }
    for e in &report.errors { println!("error: {e}"); }
    if !report.kept.is_empty() {
        println!("note: data kept; re-run with `--purge` to delete it");
    }
    if !report.errors.is_empty() {
        anyhow::bail!("uninstall completed with {} error(s); see above", report.errors.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::setup::{fake::FakeSys, AGENT_TOML_PATH};

    fn installed_sys() -> FakeSys {
        let mut sys = FakeSys::default();
        sys.ok.insert(FakeSys::key("id", &["-u", "rpi-agent"]), "999".into());
        for p in ["/var/lib/rpi", "/etc/rpi", "/var/log/rpi", UNIT_PATH, AGENT_TOML_PATH] {
            sys.paths.insert(p.into());
        }
        sys
    }

    #[tokio::test]
    async fn default_keeps_data() {
        let sys = installed_sys();
        let report = uninstall(&sys, &UninstallOpts { purge: false }).await;
        let calls = sys.calls();
        assert!(calls.iter().any(|c| c == "systemctl disable --now rpi-agent"));
        assert!(calls.iter().any(|c| c == "userdel rpi-agent"));
        assert!(!calls.iter().any(|c| c.contains("rm -rf /var/lib/rpi")), "data preserved");
        assert!(report.kept.iter().any(|k| k.contains("/var/lib/rpi")));
    }

    #[tokio::test]
    async fn purge_removes_data() {
        let sys = installed_sys();
        let report = uninstall(&sys, &UninstallOpts { purge: true }).await;
        let calls = sys.calls();
        assert!(calls.iter().any(|c| c.contains("rm -rf /var/lib/rpi")));
        assert!(report.removed.iter().any(|r| r.contains("/var/lib/rpi")));
    }

    #[tokio::test]
    async fn purge_rm_failure_does_not_claim_removed() {
        let mut sys = installed_sys();
        sys.err.insert(FakeSys::key("rm", &["-rf", "/var/lib/rpi"]));
        let report = uninstall(&sys, &UninstallOpts { purge: true }).await;
        assert!(!report.removed.iter().any(|r| r.contains("/var/lib/rpi")), "must not claim removed on failure");
        assert!(report.errors.iter().any(|e| e.contains("/var/lib/rpi")), "failure recorded in errors");
    }
}
