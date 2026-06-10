use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::{LogSink, Source};
use pi_domain::entities::{DeployRef, FetchedSource, ProjectConfig};
use pi_domain::error::DomainError;
use tokio::process::Command;

use crate::process::{run_capture, run_streamed};

pub(crate) fn is_ssh_repo(repo: &str) -> bool {
    repo.starts_with("git@") || repo.starts_with("ssh://")
}

pub(crate) fn git_ssh_command(key: &Path, known_hosts: &Path) -> String {
    let key = key.display().to_string().replace('\\', "/");
    let known_hosts = known_hosts.display().to_string().replace('\\', "/");
    format!(
        "ssh -i \"{key}\" -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=\"{known_hosts}\""
    )
}

pub struct GitSource {
    workdirs: PathBuf,
    keys: PathBuf,
    known_hosts: PathBuf,
}

impl GitSource {
    pub fn new(data_dir: &Path) -> Arc<GitSource> {
        Arc::new(GitSource {
            workdirs: data_dir.join("workdirs"),
            keys: data_dir.join("keys"),
            known_hosts: data_dir.join("known_hosts"),
        })
    }

    fn git(&self, key: Option<&Path>, cwd: Option<&Path>) -> Command {
        let mut cmd = Command::new("git");
        if let Some(key) = key {
            cmd.env("GIT_SSH_COMMAND", git_ssh_command(key, &self.known_hosts));
        }
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        cmd
    }

    async fn ensure_key(&self, project: &str, log: &Arc<dyn LogSink>) -> Result<PathBuf, DomainError> {
        let src_err = |e: std::io::Error| DomainError::Source(format!("deploy key: {e}"));
        let dir = self.keys.join(project);
        let key = dir.join("id_ed25519");
        if !key.exists() {
            tokio::fs::create_dir_all(&dir).await.map_err(src_err)?;
            let mut cmd = Command::new("ssh-keygen");
            cmd.args(["-t", "ed25519", "-N", "", "-C"])
                .arg(format!("pi-deploy-{project}"))
                .arg("-f")
                .arg(&key);
            run_capture(cmd).await.map_err(DomainError::Source)?;
            let pubkey = tokio::fs::read_to_string(key.with_extension("pub")).await.map_err(src_err)?;
            log.line("generated deploy key for this project; add it to GitHub -> repo Settings -> Deploy keys (read-only), then re-run deploy if fetch fails:");
            log.line(pubkey.trim());
        }
        Ok(key)
    }
}

#[async_trait]
impl Source for GitSource {
    async fn fetch(
        &self,
        project: &ProjectConfig,
        git_ref: &DeployRef,
        log: Arc<dyn LogSink>,
    ) -> Result<FetchedSource, DomainError> {
        let src_err = |e: std::io::Error| DomainError::Source(e.to_string());
        let workdir = self.workdirs.join(&project.name);

        let key = if is_ssh_repo(&project.repo) {
            Some(self.ensure_key(&project.name, &log).await?)
        } else {
            None
        };
        let key = key.as_deref();

        if !workdir.join(".git").exists() {
            tokio::fs::create_dir_all(&self.workdirs).await.map_err(src_err)?;
            log.line(&format!("cloning {} ...", project.repo));
            let mut cmd = self.git(key, None);
            cmd.arg("clone").arg(&project.repo).arg(&workdir);
            run_streamed(cmd, Arc::clone(&log)).await.map_err(DomainError::Source)?;
        }

        let mut cmd = self.git(key, Some(&workdir));
        cmd.args(["fetch", "origin", "--prune"]);
        run_streamed(cmd, Arc::clone(&log)).await.map_err(DomainError::Source)?;

        let sha = match git_ref {
            DeployRef::Sha(sha) => sha.clone(),
            DeployRef::Branch(branch) => {
                let mut cmd = self.git(None, Some(&workdir));
                cmd.args(["rev-parse", &format!("origin/{branch}")]);
                run_capture(cmd).await.map_err(DomainError::Source)?
            }
        };

        let mut cmd = self.git(None, Some(&workdir));
        cmd.args(["reset", "--hard", &sha]);
        run_streamed(cmd, Arc::clone(&log)).await.map_err(DomainError::Source)?;

        Ok(FetchedSource { workdir, commit_sha: sha })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_repo_detection() {
        assert!(is_ssh_repo("git@github.com:isskelo/rateme.git"));
        assert!(is_ssh_repo("ssh://git@github.com/isskelo/rateme.git"));
        assert!(!is_ssh_repo("https://github.com/isskelo/rateme.git"));
        assert!(!is_ssh_repo("C:/repos/local"));
    }

    #[test]
    fn git_ssh_command_uses_forward_slashes_and_pins_identity() {
        let cmd = git_ssh_command(
            std::path::Path::new("/var/lib/pi/keys/rateme/id_ed25519"),
            std::path::Path::new("/var/lib/pi/known_hosts"),
        );
        assert_eq!(
            cmd,
            "ssh -i \"/var/lib/pi/keys/rateme/id_ed25519\" -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=\"/var/lib/pi/known_hosts\""
        );
    }
}

#[cfg(test)]
mod integration {
    use super::*;
    use pi_domain::entities::{DeployRef, DeploymentStatus, ProjectConfig};
    use std::path::Path;

    struct NullSink;
    impl pi_domain::contracts::LogSink for NullSink {
        fn line(&self, _line: &str) {}
        fn finished(&self, _status: DeploymentStatus) {}
    }

    fn sh(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn make_origin(dir: &Path) {
        sh(dir, &["init", "-b", "main"]);
        sh(dir, &["config", "user.email", "test@test"]);
        sh(dir, &["config", "user.name", "test"]);
        std::fs::write(dir.join("hello.txt"), "v1").unwrap();
        sh(dir, &["add", "."]);
        sh(dir, &["commit", "-m", "v1"]);
    }

    fn cfg(repo: &Path) -> ProjectConfig {
        ProjectConfig {
            name: "demo".into(),
            repo: repo.display().to_string().replace('\\', "/"),
            branch: "main".into(),
            compose_path: "docker-compose.yml".into(),
            service: "web".into(),
            container_port: 3000,
            hostname: None,
        }
    }

    #[tokio::test]
    async fn fetch_clones_then_updates_idempotently() {
        let origin = tempfile::tempdir().unwrap();
        make_origin(origin.path());
        let data = tempfile::tempdir().unwrap();
        let source = GitSource::new(data.path());
        let log = std::sync::Arc::new(NullSink);

        let first = source
            .fetch(&cfg(origin.path()), &DeployRef::Branch("main".into()), log.clone())
            .await
            .unwrap();
        assert_eq!(first.commit_sha.len(), 40);
        assert_eq!(
            std::fs::read_to_string(first.workdir.join("hello.txt")).unwrap(),
            "v1"
        );

        std::fs::write(origin.path().join("hello.txt"), "v2").unwrap();
        sh(origin.path(), &["add", "."]);
        sh(origin.path(), &["commit", "-m", "v2"]);

        let second = source
            .fetch(&cfg(origin.path()), &DeployRef::Branch("main".into()), log.clone())
            .await
            .unwrap();
        assert_ne!(second.commit_sha, first.commit_sha);
        assert_eq!(
            std::fs::read_to_string(second.workdir.join("hello.txt")).unwrap(),
            "v2"
        );

        let pinned = source
            .fetch(&cfg(origin.path()), &DeployRef::Sha(first.commit_sha.clone()), log)
            .await
            .unwrap();
        assert_eq!(pinned.commit_sha, first.commit_sha);
        assert_eq!(
            std::fs::read_to_string(pinned.workdir.join("hello.txt")).unwrap(),
            "v1"
        );
    }
}
