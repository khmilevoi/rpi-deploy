use std::path::Path;
use async_trait::async_trait;

/// All OS effects setup needs, behind a trait so logic is testable off-Linux.
#[async_trait]
pub trait Sys: Send + Sync {
    async fn run(&self, program: &str, args: &[&str]) -> Result<String, String>;
    fn exists(&self, path: &Path) -> bool;
    fn read(&self, path: &Path) -> Option<String>;
    fn write(&self, path: &Path, content: &str) -> Result<(), String>;
}

/// True if a system user exists (`id -u <name>` succeeds).
pub async fn user_exists(sys: &dyn Sys, name: &str) -> bool {
    sys.run("id", &["-u", name]).await.is_ok()
}

/// True if `user` is a member of `group` (parsed from `id -nG <user>`).
pub async fn in_group(sys: &dyn Sys, user: &str, group: &str) -> bool {
    matches!(sys.run("id", &["-nG", user]).await, Ok(s) if s.split_whitespace().any(|g| g == group))
}

pub struct HostSys;

#[async_trait]
impl Sys for HostSys {
    async fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
        let out = tokio::process::Command::new(program)
            .args(args)
            .output()
            .await
            .map_err(|e| format!("spawn {program}: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
    fn write(&self, path: &Path, content: &str) -> Result<(), String> {
        std::fs::write(path, content).map_err(|e| format!("write {}: {e}", path.display()))
    }
}

pub const UNIT_PATH: &str = "/etc/systemd/system/pi-agent.service";
pub const AGENT_TOML_PATH: &str = "/etc/pi/agent.toml";

/// Canonical systemd unit — byte-for-byte the working install (spec §9).
pub const UNIT: &str = "\
[Unit]
Description=pi deploy agent
After=network-online.target docker.service
Wants=network-online.target

[Service]
User=pi-agent
Group=pi-agent
ExecStart=/usr/local/bin/pi agent run --config /etc/pi/agent.toml
RuntimeDirectory=pi
RuntimeDirectoryMode=0750
Restart=on-failure
Environment=HOME=/var/lib/pi
Environment=XDG_CONFIG_HOME=/var/lib/pi/.config
Environment=XDG_CACHE_HOME=/var/lib/pi/.cache
WorkingDirectory=/var/lib/pi

[Install]
WantedBy=multi-user.target
";

/// Canonical agent.toml — written only when /etc/pi/agent.toml is absent (spec §9).
pub const AGENT_TOML: &str = "\
data_dir = \"/var/lib/pi\"
socket = \"/run/pi/agent.sock\"
port_min = 8000
port_max = 8999
build_concurrency = 1
history_keep = 50

[timeouts]
fetch = \"2m\"
build = \"30m\"
up = \"5m\"

[gc]
disk_threshold_percent = 85
";

pub enum WriteAction {
    Wrote,
    Skipped,
    BackedUp,
}

/// Write the canonical unit; back up to *.bak only if an existing file differs.
pub fn write_unit_with_backup(sys: &dyn Sys, dry_run: bool) -> Result<WriteAction, String> {
    let path = Path::new(UNIT_PATH);
    if sys.exists(path) {
        if sys.read(path).as_deref() == Some(UNIT) {
            return Ok(WriteAction::Skipped);
        }
        if dry_run {
            return Ok(WriteAction::BackedUp);
        }
        let bak = format!("{UNIT_PATH}.bak");
        if let Some(old) = sys.read(path) {
            sys.write(Path::new(&bak), &old)?;
        }
        sys.write(path, UNIT)?;
        return Ok(WriteAction::BackedUp);
    }
    if !dry_run {
        sys.write(path, UNIT)?;
    }
    Ok(WriteAction::Wrote)
}

pub struct SetupOpts {
    pub login_user: String,
    pub with_cloudflared: bool,
    pub dry_run: bool,
}

#[derive(Default)]
pub struct SetupReport {
    pub created: Vec<String>,
    pub skipped: Vec<String>,
    pub repaired: Vec<String>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

impl SetupReport {
    pub fn print(&self) {
        for c in &self.created { println!("created: {c}"); }
        for r in &self.repaired { println!("repaired: {r}"); }
        for s in &self.skipped { println!("ok (already present): {s}"); }
        for w in &self.warnings { println!("warning: {w}"); }
        for e in &self.errors { println!("error: {e}"); }
        if self.repaired.iter().any(|r| r.contains("/var/log/pi")) {
            println!("note: run `sudo systemctl restart pi-agent` to activate file logs");
        }
    }
}

async fn ensure_dir(sys: &dyn Sys, path: &str, owner_group: Option<&str>, dry: bool, rep: &mut SetupReport, repair: bool) {
    if sys.exists(Path::new(path)) {
        rep.skipped.push(path.to_string());
        return;
    }
    if dry {
        if repair { rep.repaired.push(path.to_string()); } else { rep.created.push(path.to_string()); }
        return;
    }
    let args: Vec<&str> = match owner_group {
        Some(og) => vec!["-d", "-o", og, "-g", og, path],
        None => vec!["-d", path],
    };
    match sys.run("install", &args).await {
        Ok(_) => {
            if repair { rep.repaired.push(path.to_string()); } else { rep.created.push(path.to_string()); }
        }
        Err(e) => rep.errors.push(format!("mkdir {path} failed: {e}")),
    }
}

/// Idempotent agent bootstrap (spec §4). Adopt & preserve; never touches
/// secret.key/state.db. Returns a report; does not restart the agent.
pub async fn setup(sys: &dyn Sys, opts: &SetupOpts) -> SetupReport {
    let mut rep = SetupReport::default();
    let dry = opts.dry_run;

    // 1. service user
    if user_exists(sys, "pi-agent").await {
        rep.skipped.push("user pi-agent".into());
    } else if dry {
        rep.created.push("user pi-agent".into());
    } else {
        match sys.run("useradd", &["--system", "--no-create-home", "--shell", "/usr/sbin/nologin", "pi-agent"]).await {
            Ok(_) => rep.created.push("user pi-agent".into()),
            Err(e) => rep.errors.push(format!("useradd pi-agent failed: {e}")),
        }
    }

    // 2. pi-agent in docker group
    if in_group(sys, "pi-agent", "docker").await {
        rep.skipped.push("pi-agent in docker group".into());
    } else if dry {
        rep.created.push("pi-agent in docker group".into());
    } else {
        match sys.run("usermod", &["-aG", "docker", "pi-agent"]).await {
            Ok(_) => rep.created.push("pi-agent in docker group".into()),
            Err(e) => rep.errors.push(format!("usermod pi-agent docker failed: {e}")),
        }
    }

    // 3. login user in pi-agent group
    if in_group(sys, &opts.login_user, "pi-agent").await {
        rep.skipped.push(format!("{} in pi-agent group", opts.login_user));
    } else if dry {
        rep.created.push(format!("{} in pi-agent group", opts.login_user));
    } else {
        match sys.run("usermod", &["-aG", "pi-agent", &opts.login_user]).await {
            Ok(_) => rep.created.push(format!("{} in pi-agent group", opts.login_user)),
            Err(e) => rep.errors.push(format!("usermod {u} pi-agent failed: {e}", u = opts.login_user)),
        }
    }

    // 4-6. directories
    ensure_dir(sys, "/var/lib/pi", Some("pi-agent"), dry, &mut rep, false).await;
    ensure_dir(sys, "/var/log/pi", Some("pi-agent"), dry, &mut rep, true).await; // repair (§2.5)
    ensure_dir(sys, "/etc/pi", None, dry, &mut rep, false).await;

    // 7. agent.toml (only if absent)
    if sys.exists(Path::new(AGENT_TOML_PATH)) {
        rep.skipped.push(AGENT_TOML_PATH.into());
    } else if dry {
        rep.created.push(AGENT_TOML_PATH.into());
    } else {
        match sys.write(Path::new(AGENT_TOML_PATH), AGENT_TOML) {
            Ok(_) => rep.created.push(AGENT_TOML_PATH.into()),
            Err(e) => rep.errors.push(format!("write {AGENT_TOML_PATH} failed: {e}")),
        }
    }

    // 8. systemd unit + enable
    match write_unit_with_backup(sys, dry) {
        Ok(WriteAction::Skipped) => rep.skipped.push(UNIT_PATH.into()),
        Ok(WriteAction::BackedUp) => rep.repaired.push(format!("{UNIT_PATH} (backed up to .bak)")),
        Ok(WriteAction::Wrote) => rep.created.push(UNIT_PATH.into()),
        Err(e) => rep.warnings.push(format!("unit: {e}")),
    }
    if !dry {
        if sys.run("systemctl", &["daemon-reload"]).await.is_err() {
            rep.warnings.push("systemctl daemon-reload failed".into());
        }
        if sys.run("systemctl", &["enable", "--now", "pi-agent"]).await.is_err() {
            rep.warnings.push("systemctl enable --now pi-agent failed (is /usr/local/bin/pi installed?)".into());
        }
    }

    // 9. cloudflared (opt-in) — implemented in Task 13.
    if opts.with_cloudflared {
        cloudflared_bootstrap(sys, dry, &mut rep).await;
    }

    // 10. dependency checks (warn, never fail)
    if sys.run("docker", &["version", "--format", "{{.Server.Version}}"]).await.is_err() {
        rep.warnings.push("docker not available — install Docker Engine and add pi-agent to the docker group".into());
    }
    if sys.run("docker", &["compose", "version"]).await.is_err() {
        rep.warnings.push("docker compose plugin missing — install Docker Compose v2".into());
    }

    rep
}

const CLOUDFLARED_UNIT_PATH: &str = "/var/lib/pi/.config/systemd/user/cloudflared.service";

const CLOUDFLARED_UNIT: &str = "\
[Unit]
Description=cloudflared tunnel (pi-agent)
After=network-online.target

[Service]
ExecStart=/usr/local/bin/cloudflared tunnel run
Restart=on-failure

[Install]
WantedBy=default.target
";

/// Opt-in cloudflared scaffolding: enable linger and write the user unit.
/// The interactive `cloudflared tunnel login` step is left to the operator.
async fn cloudflared_bootstrap(sys: &dyn Sys, dry: bool, rep: &mut SetupReport) {
    if !dry {
        let _ = sys.run("loginctl", &["enable-linger", "pi-agent"]).await;
    }
    rep.created.push("systemd linger for pi-agent".into());

    if sys.exists(Path::new(CLOUDFLARED_UNIT_PATH)) {
        rep.skipped.push(CLOUDFLARED_UNIT_PATH.into());
    } else {
        if !dry {
            let _ = sys.run("install", &["-d", "-o", "pi-agent", "-g", "pi-agent", "/var/lib/pi/.config/systemd/user"]).await;
            let _ = sys.write(Path::new(CLOUDFLARED_UNIT_PATH), CLOUDFLARED_UNIT);
        }
        rep.created.push(CLOUDFLARED_UNIT_PATH.into());
    }
    rep.warnings.push(
        "cloudflared: finish manually — run `cloudflared tunnel login`, create a tunnel, \
         write /var/lib/pi/cloudflared/config.yml, add [cloudflared] to /etc/pi/agent.toml, \
         then `systemctl --user enable --now cloudflared` as pi-agent".into(),
    );
}

async fn run_with(sys: &dyn Sys, opts: &SetupOpts) -> anyhow::Result<SetupReport> {
    let report = setup(sys, opts).await;
    report.print();
    if opts.dry_run {
        println!("(dry run — no changes made)");
    }
    if !report.errors.is_empty() {
        anyhow::bail!("setup completed with {} error(s); see above", report.errors.len());
    }
    Ok(report)
}

/// CLI entrypoint: resolve the login user (--user or $SUDO_USER), run setup,
/// print the report. Must run as root (under sudo) on the Pi.
pub async fn run_cmd(user: Option<String>, with_cloudflared: bool, dry_run: bool) -> anyhow::Result<()> {
    let login_user = user
        .or_else(|| std::env::var("SUDO_USER").ok())
        .filter(|u| !u.is_empty() && u != "root")
        .ok_or_else(|| anyhow::anyhow!(
            "cannot determine the SSH login user; run via `sudo pi agent setup` or pass --user <name>"
        ))?;
    let opts = SetupOpts { login_user, with_cloudflared, dry_run };
    run_with(&HostSys, &opts).await.map(|_| ())
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FakeSys {
        pub paths: HashSet<String>,
        pub files: HashMap<String, String>,
        pub ok: HashMap<String, String>,   // "program a b" -> stdout
        pub err: HashSet<String>,          // "program a b" that fail
        pub calls: Mutex<Vec<String>>,
        pub writes: Mutex<Vec<(String, String)>>,
    }

    impl FakeSys {
        pub fn key(program: &str, args: &[&str]) -> String {
            std::iter::once(program).chain(args.iter().copied()).collect::<Vec<_>>().join(" ")
        }
        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Sys for FakeSys {
        async fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
            let k = FakeSys::key(program, args);
            self.calls.lock().unwrap().push(k.clone());
            if self.err.contains(&k) {
                return Err(format!("fake error: {k}"));
            }
            Ok(self.ok.get(&k).cloned().unwrap_or_default())
        }
        fn exists(&self, path: &Path) -> bool {
            self.paths.contains(path.to_str().unwrap())
        }
        fn read(&self, path: &Path) -> Option<String> {
            self.files.get(path.to_str().unwrap()).cloned()
        }
        fn write(&self, path: &Path, content: &str) -> Result<(), String> {
            self.writes.lock().unwrap().push((path.to_string_lossy().into(), content.into()));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::fake::FakeSys;

    #[tokio::test]
    async fn user_exists_reflects_id_result() {
        let mut sys = FakeSys::default();
        sys.ok.insert(FakeSys::key("id", &["-u", "pi-agent"]), "999".into());
        assert!(user_exists(&sys, "pi-agent").await);

        let mut absent = FakeSys::default();
        absent.err.insert(FakeSys::key("id", &["-u", "pi-agent"]));
        assert!(!user_exists(&absent, "pi-agent").await);
    }

    #[tokio::test]
    async fn in_group_parses_id_ng() {
        let mut sys = FakeSys::default();
        sys.ok.insert(FakeSys::key("id", &["-nG", "piuser"]), "piuser sudo docker pi-agent".into());
        assert!(in_group(&sys, "piuser", "docker").await);
        assert!(!in_group(&sys, "piuser", "wheel").await);
    }

    #[test]
    fn unit_template_matches_spec_byte_for_byte() {
        assert!(UNIT.starts_with("[Unit]\nDescription=pi deploy agent\n"));
        assert!(UNIT.contains("ExecStart=/usr/local/bin/pi agent run --config /etc/pi/agent.toml\n"));
        assert!(UNIT.contains("Environment=XDG_CACHE_HOME=/var/lib/pi/.cache\n"));
        assert!(UNIT.ends_with("WantedBy=multi-user.target\n"));
    }

    #[tokio::test]
    async fn write_unit_skips_when_identical() {
        let mut sys = FakeSys::default();
        sys.paths.insert(UNIT_PATH.into());
        sys.files.insert(UNIT_PATH.into(), UNIT.into());
        let action = write_unit_with_backup(&sys, false).unwrap();
        assert!(matches!(action, WriteAction::Skipped));
        assert!(sys.writes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn write_unit_backs_up_when_different() {
        let mut sys = FakeSys::default();
        sys.paths.insert(UNIT_PATH.into());
        sys.files.insert(UNIT_PATH.into(), "old=unit\n".into());
        let action = write_unit_with_backup(&sys, false).unwrap();
        assert!(matches!(action, WriteAction::BackedUp));
        let writes = sys.writes.lock().unwrap();
        assert!(writes.iter().any(|(p, _)| p.ends_with("pi-agent.service.bak")), "backup written");
        assert!(writes.iter().any(|(p, c)| p == UNIT_PATH && c == UNIT), "canonical written");
    }

    fn fresh_sys() -> FakeSys {
        let mut sys = FakeSys::default();
        // user absent, no dirs/files exist; group lookups succeed but show no membership.
        sys.err.insert(FakeSys::key("id", &["-u", "pi-agent"]));
        sys.ok.insert(FakeSys::key("id", &["-nG", "pi-agent"]), "pi-agent".into());
        sys.ok.insert(FakeSys::key("id", &["-nG", "piuser"]), "piuser sudo".into());
        sys.ok.insert(FakeSys::key("docker", &["version", "--format", "{{.Server.Version}}"]), "27.0".into());
        sys.ok.insert(FakeSys::key("docker", &["compose", "version"]), "v2".into());
        sys
    }

    #[tokio::test]
    async fn fresh_install_creates_user_dirs_unit() {
        let sys = fresh_sys();
        let opts = SetupOpts { login_user: "piuser".into(), with_cloudflared: false, dry_run: false };
        let report = setup(&sys, &opts).await;
        let calls = sys.calls();
        assert!(calls.iter().any(|c| c.starts_with("useradd --system")), "creates pi-agent");
        assert!(calls.iter().any(|c| c == "usermod -aG docker pi-agent"));
        assert!(calls.iter().any(|c| c == "usermod -aG pi-agent piuser"));
        assert!(calls.iter().any(|c| c.contains("install -d -o pi-agent -g pi-agent /var/lib/pi")));
        assert!(calls.iter().any(|c| c.contains("install -d -o pi-agent -g pi-agent /var/log/pi")));
        assert!(calls.iter().any(|c| c == "systemctl daemon-reload"));
        assert!(calls.iter().any(|c| c == "systemctl enable --now pi-agent"));
        assert!(report.warnings.is_empty(), "docker present -> no warnings");
    }

    #[tokio::test]
    async fn repairs_only_missing_var_log_pi_on_working_install() {
        let mut sys = FakeSys::default();
        // user exists and is in both groups; all dirs exist EXCEPT /var/log/pi; unit identical.
        sys.ok.insert(FakeSys::key("id", &["-u", "pi-agent"]), "999".into());
        sys.ok.insert(FakeSys::key("id", &["-nG", "pi-agent"]), "pi-agent docker".into());
        sys.ok.insert(FakeSys::key("id", &["-nG", "piuser"]), "piuser sudo docker pi-agent".into());
        sys.ok.insert(FakeSys::key("docker", &["version", "--format", "{{.Server.Version}}"]), "27.0".into());
        sys.ok.insert(FakeSys::key("docker", &["compose", "version"]), "v2".into());
        for p in ["/var/lib/pi", "/etc/pi", UNIT_PATH, AGENT_TOML_PATH] {
            sys.paths.insert(p.into());
        }
        sys.files.insert(UNIT_PATH.into(), UNIT.into());
        sys.files.insert(AGENT_TOML_PATH.into(), AGENT_TOML.into());
        let opts = SetupOpts { login_user: "piuser".into(), with_cloudflared: false, dry_run: false };
        let report = setup(&sys, &opts).await;
        let calls = sys.calls();
        assert!(!calls.iter().any(|c| c.starts_with("useradd")), "user not recreated");
        assert!(!calls.iter().any(|c| c.starts_with("usermod")), "groups untouched");
        assert!(calls.iter().any(|c| c.contains("install -d -o pi-agent -g pi-agent /var/log/pi")));
        assert!(report.repaired.iter().any(|r| r.contains("/var/log/pi")));
        assert!(sys.writes.lock().unwrap().is_empty(), "agent.toml/unit untouched");
    }

    #[tokio::test]
    async fn dry_run_makes_no_changes() {
        let sys = fresh_sys();
        let opts = SetupOpts { login_user: "piuser".into(), with_cloudflared: false, dry_run: true };
        let _ = setup(&sys, &opts).await;
        let calls = sys.calls();
        assert!(calls.iter().all(|c| c.starts_with("id ") || c.starts_with("docker ")), "only probes ran: {calls:?}");
        assert!(sys.writes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn missing_docker_warns_not_fails() {
        let mut sys = fresh_sys();
        sys.ok.remove(&FakeSys::key("docker", &["version", "--format", "{{.Server.Version}}"]));
        sys.err.insert(FakeSys::key("docker", &["version", "--format", "{{.Server.Version}}"]));
        let opts = SetupOpts { login_user: "piuser".into(), with_cloudflared: false, dry_run: false };
        let report = setup(&sys, &opts).await;
        assert!(report.warnings.iter().any(|w| w.contains("docker")));
    }

    #[tokio::test]
    async fn with_cloudflared_enables_linger_and_instructs() {
        let sys = fresh_sys();
        let opts = SetupOpts { login_user: "piuser".into(), with_cloudflared: true, dry_run: false };
        let report = setup(&sys, &opts).await;
        let calls = sys.calls();
        assert!(calls.iter().any(|c| c == "loginctl enable-linger pi-agent"));
        assert!(report.created.iter().any(|c| c.contains("linger")));
        assert!(report.warnings.iter().any(|w| w.contains("cloudflared tunnel login")), "prints manual login step");
    }

    #[tokio::test]
    async fn without_cloudflared_does_not_touch_linger() {
        let sys = fresh_sys();
        let opts = SetupOpts { login_user: "piuser".into(), with_cloudflared: false, dry_run: false };
        let _ = setup(&sys, &opts).await;
        assert!(!sys.calls().iter().any(|c| c.contains("enable-linger")));
    }

    #[tokio::test]
    async fn useradd_failure_records_error_not_created() {
        let mut sys = fresh_sys();
        sys.err.insert(FakeSys::key("useradd", &["--system", "--no-create-home", "--shell", "/usr/sbin/nologin", "pi-agent"]));
        let opts = SetupOpts { login_user: "piuser".into(), with_cloudflared: false, dry_run: false };
        let report = setup(&sys, &opts).await;
        assert!(report.errors.iter().any(|e| e.contains("useradd")), "error recorded");
        assert!(!report.created.iter().any(|c| c == "user pi-agent"), "no false created");
    }

    #[tokio::test]
    async fn mkdir_failure_records_error_not_created() {
        let mut sys = fresh_sys();
        sys.err.insert(FakeSys::key("install", &["-d", "-o", "pi-agent", "-g", "pi-agent", "/var/lib/pi"]));
        let opts = SetupOpts { login_user: "piuser".into(), with_cloudflared: false, dry_run: false };
        let report = setup(&sys, &opts).await;
        assert!(report.errors.iter().any(|e| e.contains("/var/lib/pi")), "mkdir error recorded");
        assert!(!report.created.iter().any(|c| c.contains("/var/lib/pi")));
    }

    #[tokio::test]
    async fn run_with_bails_on_errors() {
        let mut sys = fresh_sys();
        sys.err.insert(FakeSys::key("useradd", &["--system", "--no-create-home", "--shell", "/usr/sbin/nologin", "pi-agent"]));
        let opts = SetupOpts { login_user: "piuser".into(), with_cloudflared: false, dry_run: false };
        let res = run_with(&sys, &opts).await;
        assert!(res.is_err(), "non-zero exit when privileged step fails");
    }
}
