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

    async fn ensure_key(
        &self,
        project: &str,
        log: &Arc<dyn LogSink>,
    ) -> Result<PathBuf, DomainError> {
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
            let pubkey = tokio::fs::read_to_string(key.with_extension("pub"))
                .await
                .map_err(src_err)?;
            log.line("generated deploy key for this project; add it to GitHub -> repo Settings -> Deploy keys (read-only), then re-run deploy if fetch fails:");
            log.line(pubkey.trim());
        }
        Ok(key)
    }
}

#[async_trait]
impl Source for GitSource {
    fn workdir(&self, project_name: &str) -> PathBuf {
        self.workdirs.join(project_name)
    }

    async fn fetch(
        &self,
        project: &ProjectConfig,
        git_ref: &DeployRef,
        log: Arc<dyn LogSink>,
    ) -> Result<FetchedSource, DomainError> {
        let src_err = |e: std::io::Error| DomainError::Source(e.to_string());
        let workdir = self.workdir(&project.name);

        let key = if is_ssh_repo(&project.repo) {
            Some(self.ensure_key(&project.name, &log).await?)
        } else {
            None
        };
        let key = key.as_deref();

        if !workdir.join(".git").exists() {
            tokio::fs::create_dir_all(&self.workdirs)
                .await
                .map_err(src_err)?;
            log.line(&format!("cloning {} ...", project.repo));
            let mut cmd = self.git(key, None);
            cmd.arg("clone").arg(&project.repo).arg(&workdir);
            run_streamed(cmd, Arc::clone(&log))
                .await
                .map_err(DomainError::Source)?;
        }

        let mut cmd = self.git(key, Some(&workdir));
        cmd.args(["fetch", "origin", "--prune"]);
        run_streamed(cmd, Arc::clone(&log))
            .await
            .map_err(DomainError::Source)?;

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
        run_streamed(cmd, Arc::clone(&log))
            .await
            .map_err(DomainError::Source)?;

        Ok(FetchedSource {
            workdir,
            commit_sha: sha,
        })
    }

    async fn cleanup(&self, project_name: &str) -> Result<(), DomainError> {
        for path in [
            self.workdirs.join(project_name),
            self.keys.join(project_name),
        ] {
            remove_tree(&path).await?;
        }
        Ok(())
    }
}

/// Default cleanup fallback image for `force_remove_via_docker`, used unless
/// overridden by `CLEANUP_IMAGE_ENV_VAR`. Pinned (not `:latest`) so the
/// fallback container's behavior can't drift under us. `busybox` is a few
/// hundred KB and its `rm` applet is all this needs; the specific tag is
/// already pulled by every scenario in `tests/e2e` (both fixture apps build
/// `FROM busybox:1.37`), so on this codebase's own e2e harness the fallback
/// never needs a fresh network pull. On a real host that has never used this
/// image before, the first `rpi rm` that hits the fallback pays a small
/// one-time pull; every `rpi rm` after that reuses the locally cached image.
const DEFAULT_CLEANUP_IMAGE: &str = "busybox:1.37";

/// Env var that overrides the cleanup fallback image. Lets an operator on an
/// offline/air-gapped host (or one that just prefers a pre-mirrored image)
/// point the one-shot cleanup container at something already present
/// locally, instead of forcing a network pull of `DEFAULT_CLEANUP_IMAGE` the
/// first time `remove_tree` hits `EACCES`. Named after this crate's existing
/// `PI_*` env vars (`PI_SERVER`, `PI_AGENT_URL`, `PI_THEME`).
const CLEANUP_IMAGE_ENV_VAR: &str = "PI_RM_CLEANUP_IMAGE";

/// Resolves the image `force_remove_via_docker` runs: `CLEANUP_IMAGE_ENV_VAR`
/// if set to a non-blank value (surrounding whitespace trimmed), else
/// `DEFAULT_CLEANUP_IMAGE`. Kept as a small, pure, env-reading helper so the
/// resolution logic is unit-testable without touching Docker.
fn cleanup_image() -> String {
    std::env::var(CLEANUP_IMAGE_ENV_VAR)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_CLEANUP_IMAGE.to_string())
}

/// Removes `path` (file or directory tree), tolerating a tree the agent does
/// not fully own. Tries a plain removal first -- the common case, since the
/// agent created everything under its own data dir -- and only reaches for
/// Docker when that comes back `EACCES`: a deployed service bind-mounted a
/// path under the workdir and, running as root (Docker's default container
/// user), wrote root-owned files into it. The agent process itself is never
/// root and can't `chown`/`chmod` its way out of that.
///
/// The fallback runs a throwaway container, via the same `docker` CLI
/// invocation this crate already uses for every compose operation
/// (`docker.rs`), that bind-mounts the parent directory and force-removes
/// the target by name as root. This grants the *cleanup step*, not the
/// long-running `rpi-agent` process, root privilege: the agent already
/// talks to dockerd (which runs as root) for every build/up/down, so this
/// reuses privilege the system already grants the daemon instead of
/// escalating the agent itself (e.g. via a sudoers rule).
async fn remove_tree(path: &Path) -> Result<(), DomainError> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) if e.kind() != std::io::ErrorKind::PermissionDenied => {
            return Err(DomainError::Source(e.to_string()));
        }
        Err(_) => {} // EACCES: fall through to the root cleanup container below.
    }

    let image = cleanup_image();
    if let Err(docker_err) = force_remove_via_docker(path, &image).await {
        return Err(permission_denied_err(path, &image, &docker_err));
    }

    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(permission_denied_err(path, &image, &e.to_string())),
    }
}

/// Splits `path` into the parent to bind-mount and the child name to delete
/// inside it, the two pieces `force_remove_args` needs. A named error (not
/// just `None`) so callers can report *why* the fallback couldn't even be
/// attempted.
fn split_for_force_remove(path: &Path) -> Result<(&Path, &str), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{} has a non-UTF-8 file name", path.display()))?;
    Ok((parent, name))
}

/// `docker run` args that delete `name` (a child of `parent`) as root:
/// bind-mounts `parent` at `/target` and `rm -rf`s the child by name inside
/// the container. Mounting the parent (rather than the target itself) lets
/// the container delete the target outright rather than just its contents,
/// so a successful run leaves nothing behind for the caller to clean up.
fn force_remove_args(parent: &Path, name: &str, image: &str) -> Vec<String> {
    vec![
        "run".to_string(),
        "--rm".to_string(),
        "-v".to_string(),
        format!("{}:/target", parent.display()),
        image.to_string(),
        "rm".to_string(),
        "-rf".to_string(),
        "--".to_string(),
        format!("/target/{name}"),
    ]
}

/// Force-removes `path` as root by running a one-shot `docker` container
/// using `image` (see `force_remove_args`), the same `docker` CLI invocation
/// mechanism this crate already uses for every compose operation
/// (`docker.rs`).
async fn force_remove_via_docker(path: &Path, image: &str) -> Result<(), String> {
    let (parent, name) = split_for_force_remove(path)?;
    let mut cmd = Command::new("docker");
    cmd.args(force_remove_args(parent, name, image));
    run_capture(cmd).await.map(|_| ())
}

fn permission_denied_err(path: &Path, image: &str, detail: &str) -> DomainError {
    DomainError::Source(format!(
        "cannot remove {p}: it contains files owned by another user (typically \
         left behind by a container that ran as root and wrote into a \
         bind-mounted path under the workdir); automatic cleanup via a \
         one-shot root container ({image}) also failed ({detail}). If this \
         host cannot reach a registry to pull {image} (e.g. an air-gapped \
         host), set {var}=<image already present locally> and re-run `rpi \
         rm`. Otherwise fix manually on the agent host with `sudo rm -rf {p}` \
         and re-run `rpi rm`.",
        p = path.display(),
        var = CLEANUP_IMAGE_ENV_VAR,
    ))
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
            std::path::Path::new("/var/lib/rpi/keys/rateme/id_ed25519"),
            std::path::Path::new("/var/lib/rpi/known_hosts"),
        );
        assert_eq!(
            cmd,
            "ssh -i \"/var/lib/rpi/keys/rateme/id_ed25519\" -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=\"/var/lib/rpi/known_hosts\""
        );
    }

    #[test]
    fn workdir_is_under_data_dir_workdirs() {
        let source = GitSource::new(std::path::Path::new("/var/lib/rpi"));
        assert_eq!(
            source.workdir("rateme"),
            std::path::PathBuf::from("/var/lib/rpi/workdirs/rateme")
        );
    }

    #[test]
    fn split_for_force_remove_returns_parent_and_name() {
        let (parent, name) =
            split_for_force_remove(Path::new("/var/lib/rpi/workdirs/rateme")).unwrap();
        assert_eq!(parent, Path::new("/var/lib/rpi/workdirs"));
        assert_eq!(name, "rateme");
    }

    #[test]
    fn split_for_force_remove_rejects_a_path_with_no_parent() {
        let err = split_for_force_remove(Path::new("/")).unwrap_err();
        assert!(err.contains("no parent directory"), "{err}");
    }

    #[test]
    fn force_remove_args_bind_mounts_parent_and_removes_child_by_name() {
        let args = force_remove_args(
            Path::new("/var/lib/rpi/workdirs"),
            "rateme",
            DEFAULT_CLEANUP_IMAGE,
        );
        assert_eq!(
            args,
            vec![
                "run",
                "--rm",
                "-v",
                "/var/lib/rpi/workdirs:/target",
                DEFAULT_CLEANUP_IMAGE,
                "rm",
                "-rf",
                "--",
                "/target/rateme",
            ]
        );
    }

    #[test]
    fn force_remove_args_uses_the_image_it_is_given() {
        let args = force_remove_args(
            Path::new("/var/lib/rpi/workdirs"),
            "rateme",
            "registry.local/cleanup:offline",
        );
        assert_eq!(args[4], "registry.local/cleanup:offline");
    }

    #[test]
    fn cleanup_image_is_pinned_not_latest() {
        assert!(
            DEFAULT_CLEANUP_IMAGE.contains(':') && !DEFAULT_CLEANUP_IMAGE.ends_with(":latest"),
            "default fallback image must be pinned for reproducibility: {DEFAULT_CLEANUP_IMAGE}"
        );
    }

    // Serializes tests that mutate `CLEANUP_IMAGE_ENV_VAR` via `std::env`,
    // which is process-global state shared across the parallel threads
    // `cargo test` runs tests on by default. Without this, a test asserting
    // the unset/default case could observe an override left behind by a
    // concurrently-running override test, or vice versa.
    static CLEANUP_IMAGE_ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn cleanup_image_defaults_when_env_var_is_unset() {
        let _guard = CLEANUP_IMAGE_ENV_GUARD
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        std::env::remove_var(CLEANUP_IMAGE_ENV_VAR);
        assert_eq!(cleanup_image(), DEFAULT_CLEANUP_IMAGE);
    }

    #[test]
    fn cleanup_image_uses_the_env_var_override_when_set() {
        let _guard = CLEANUP_IMAGE_ENV_GUARD
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        std::env::set_var(CLEANUP_IMAGE_ENV_VAR, "registry.local/cleanup:offline");
        let resolved = cleanup_image();
        std::env::remove_var(CLEANUP_IMAGE_ENV_VAR);
        assert_eq!(resolved, "registry.local/cleanup:offline");
    }

    #[test]
    fn cleanup_image_falls_back_to_default_on_blank_override() {
        let _guard = CLEANUP_IMAGE_ENV_GUARD
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        std::env::set_var(CLEANUP_IMAGE_ENV_VAR, "   ");
        let resolved = cleanup_image();
        std::env::remove_var(CLEANUP_IMAGE_ENV_VAR);
        assert_eq!(resolved, DEFAULT_CLEANUP_IMAGE);
    }

    #[test]
    fn cleanup_image_falls_back_to_default_on_empty_override() {
        let _guard = CLEANUP_IMAGE_ENV_GUARD
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        std::env::set_var(CLEANUP_IMAGE_ENV_VAR, "");
        let resolved = cleanup_image();
        std::env::remove_var(CLEANUP_IMAGE_ENV_VAR);
        assert_eq!(resolved, DEFAULT_CLEANUP_IMAGE);
    }

    #[test]
    fn permission_denied_err_names_the_path_and_recovery_command() {
        let err = permission_denied_err(
            Path::new("/var/lib/rpi/workdirs/rateme"),
            DEFAULT_CLEANUP_IMAGE,
            "boom",
        );
        let msg = err.to_string();
        assert!(
            msg.contains("/var/lib/rpi/workdirs/rateme"),
            "message should name the path: {msg}"
        );
        assert!(
            msg.contains("sudo rm -rf /var/lib/rpi/workdirs/rateme"),
            "message should give a manual recovery command: {msg}"
        );
        assert!(
            msg.contains("boom"),
            "message should include the underlying error: {msg}"
        );
        assert!(
            msg.contains(DEFAULT_CLEANUP_IMAGE),
            "message should name the cleanup image: {msg}"
        );
        assert!(
            msg.contains(CLEANUP_IMAGE_ENV_VAR),
            "message should mention the override env var: {msg}"
        );
        assert!(msg.starts_with("source error:"), "{msg}");
    }

    #[tokio::test]
    async fn remove_tree_removes_an_ordinary_agent_owned_directory() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("project");
        std::fs::create_dir_all(target.join("nested")).unwrap();
        std::fs::write(target.join("nested").join("file.txt"), b"hi").unwrap();

        remove_tree(&target).await.unwrap();

        assert!(!target.exists());
    }

    #[tokio::test]
    async fn remove_tree_tolerates_an_already_absent_path() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("never-existed");

        remove_tree(&target).await.unwrap();
    }
}

#[cfg(test)]
mod integration {
    use super::*;
    use pi_domain::entities::{
        DeployRef, DeploymentStatus, ExposeMode, HealthcheckConfig, ProjectConfig,
        StageTimeoutOverrides,
    };
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
            expose: ExposeMode::default(),
            healthcheck: HealthcheckConfig::default(),
            timeouts: StageTimeoutOverrides::default(),
            commands: Default::default(),
            command_timeout_secs: None,
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
            .fetch(
                &cfg(origin.path()),
                &DeployRef::Branch("main".into()),
                log.clone(),
            )
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
            .fetch(
                &cfg(origin.path()),
                &DeployRef::Branch("main".into()),
                log.clone(),
            )
            .await
            .unwrap();
        assert_ne!(second.commit_sha, first.commit_sha);
        assert_eq!(
            std::fs::read_to_string(second.workdir.join("hello.txt")).unwrap(),
            "v2"
        );

        let pinned = source
            .fetch(
                &cfg(origin.path()),
                &DeployRef::Sha(first.commit_sha.clone()),
                log,
            )
            .await
            .unwrap();
        assert_eq!(pinned.commit_sha, first.commit_sha);
        assert_eq!(
            std::fs::read_to_string(pinned.workdir.join("hello.txt")).unwrap(),
            "v1"
        );
    }
}
