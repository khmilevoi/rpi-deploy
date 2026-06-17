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
}
