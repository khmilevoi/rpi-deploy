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
}
