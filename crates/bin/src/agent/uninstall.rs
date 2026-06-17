use std::path::Path;
use crate::agent::setup::{user_exists, HostSys, Sys, UNIT_PATH};

pub struct UninstallOpts {
    pub purge: bool,
    pub yes: bool,
}

#[derive(Default)]
pub struct UninstallReport {
    pub removed: Vec<String>,
    pub kept: Vec<String>,
}

/// Remove the agent service/unit/user. Keeps data dirs unless `purge` (spec §5).
pub async fn uninstall(sys: &dyn Sys, opts: &UninstallOpts) -> UninstallReport {
    let mut rep = UninstallReport::default();

    let _ = sys.run("systemctl", &["disable", "--now", "pi-agent"]).await;
    if sys.exists(Path::new(UNIT_PATH)) {
        let _ = sys.run("rm", &["-f", UNIT_PATH, &format!("{UNIT_PATH}.bak")]).await;
        rep.removed.push(UNIT_PATH.into());
    }
    let _ = sys.run("systemctl", &["daemon-reload"]).await;
    if user_exists(sys, "pi-agent").await {
        let _ = sys.run("userdel", &["pi-agent"]).await;
        rep.removed.push("user pi-agent".into());
    }

    if opts.purge {
        for dir in ["/var/lib/pi", "/etc/pi", "/var/log/pi"] {
            if sys.exists(Path::new(dir)) {
                let _ = sys.run("rm", &["-rf", dir]).await;
                rep.removed.push(dir.into());
            }
        }
    } else {
        for dir in ["/var/lib/pi", "/etc/pi", "/var/log/pi"] {
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
            "--purge deletes /var/lib/pi (secrets, deploy keys, state) irreversibly. \
             Re-run with --purge --yes to confirm."
        );
    }
    let report = uninstall(&HostSys, &UninstallOpts { purge, yes }).await;
    for r in &report.removed { println!("removed: {r}"); }
    for k in &report.kept { println!("kept: {k}"); }
    if !report.kept.is_empty() {
        println!("note: data kept; re-run with `--purge` to delete it");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::setup::{fake::FakeSys, AGENT_TOML_PATH};

    fn installed_sys() -> FakeSys {
        let mut sys = FakeSys::default();
        sys.ok.insert(FakeSys::key("id", &["-u", "pi-agent"]), "999".into());
        for p in ["/var/lib/pi", "/etc/pi", "/var/log/pi", UNIT_PATH, AGENT_TOML_PATH] {
            sys.paths.insert(p.into());
        }
        sys
    }

    #[tokio::test]
    async fn default_keeps_data() {
        let sys = installed_sys();
        let report = uninstall(&sys, &UninstallOpts { purge: false, yes: true }).await;
        let calls = sys.calls();
        assert!(calls.iter().any(|c| c == "systemctl disable --now pi-agent"));
        assert!(calls.iter().any(|c| c == "userdel pi-agent"));
        assert!(!calls.iter().any(|c| c.contains("rm -rf /var/lib/pi")), "data preserved");
        assert!(report.kept.iter().any(|k| k.contains("/var/lib/pi")));
    }

    #[tokio::test]
    async fn purge_removes_data() {
        let sys = installed_sys();
        let report = uninstall(&sys, &UninstallOpts { purge: true, yes: true }).await;
        let calls = sys.calls();
        assert!(calls.iter().any(|c| c.contains("rm -rf /var/lib/pi")));
        assert!(report.removed.iter().any(|r| r.contains("/var/lib/pi")));
    }
}
