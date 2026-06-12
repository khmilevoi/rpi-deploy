use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::{ContainerRuntime, LogSink};
use pi_domain::entities::{ComposeStack, ServiceState};
use pi_domain::error::DomainError;
use tokio::process::Command;

use crate::process::{run_capture, run_streamed};

/// Age filter for `docker builder prune` (§8.1): recent cache survives so
/// rebuilds stay fast; only the disk threshold is configurable (§22).
pub(crate) const BUILDER_PRUNE_MAX_AGE: &str = "24h";

pub(crate) fn prune_images_args() -> Vec<String> {
    vec!["image".to_string(), "prune".to_string(), "-f".to_string()]
}

pub(crate) fn prune_builder_args() -> Vec<String> {
    vec![
        "builder".to_string(),
        "prune".to_string(),
        "-f".to_string(),
        "--filter".to_string(),
        format!("until={BUILDER_PRUNE_MAX_AGE}"),
    ]
}

pub(crate) fn file_chain(stack: &ComposeStack) -> Vec<PathBuf> {
    let mut files = vec![stack.compose_file.clone()];
    let repo_override = stack.workdir.join("docker-compose.override.yml");
    if repo_override.exists() && repo_override != stack.compose_file {
        files.push(repo_override);
    }
    files.push(stack.override_file.clone());
    files
}

pub(crate) fn compose_args(project_name: &str, files: &[PathBuf], tail: &[&str]) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["compose".into(), "-p".into(), project_name.into()];
    for f in files {
        args.push("-f".into());
        args.push(f.clone().into());
    }
    args.extend(tail.iter().map(|s| OsString::from(*s)));
    args
}

pub(crate) fn parse_ps_json(output: &str) -> Vec<ServiceState> {
    let trimmed = output.trim_start();
    if trimmed.starts_with('[') {
        return serde_json::from_str::<Vec<serde_json::Value>>(trimmed)
            .map(|items| items.iter().filter_map(service_state).collect())
            .unwrap_or_default();
    }
    output
        .lines()
        .filter_map(|line| service_state(&serde_json::from_str(line).ok()?))
        .collect()
}

fn service_state(v: &serde_json::Value) -> Option<ServiceState> {
    let health = v
        .get("Health")
        .and_then(|h| h.as_str())
        .filter(|h| !h.is_empty())
        .map(str::to_string);
    Some(ServiceState {
        service: v.get("Service")?.as_str()?.to_string(),
        state: v.get("State")?.as_str()?.to_string(),
        health,
    })
}

pub struct DockerComposeRuntime;

impl DockerComposeRuntime {
    pub fn new() -> Arc<DockerComposeRuntime> {
        Arc::new(DockerComposeRuntime)
    }

    fn compose(&self, stack: &ComposeStack, tail: &[&str]) -> Command {
        let mut cmd = Command::new("docker");
        cmd.args(compose_args(&stack.project_name, &file_chain(stack), tail));
        cmd.current_dir(&stack.workdir);
        cmd
    }
}

#[async_trait]
impl ContainerRuntime for DockerComposeRuntime {
    async fn build(&self, stack: &ComposeStack, log: Arc<dyn LogSink>) -> Result<(), DomainError> {
        log.line("docker compose build ...");
        run_streamed(self.compose(stack, &["build"]), log)
            .await
            .map_err(DomainError::Runtime)
    }

    async fn up(&self, stack: &ComposeStack, log: Arc<dyn LogSink>) -> Result<(), DomainError> {
        log.line("docker compose up -d ...");
        run_streamed(self.compose(stack, &["up", "-d", "--remove-orphans"]), log)
            .await
            .map_err(DomainError::Runtime)
    }

    async fn ps(&self, project_name: &str) -> Result<Vec<ServiceState>, DomainError> {
        let mut cmd = Command::new("docker");
        cmd.args(["compose", "-p", project_name, "ps", "--format", "json"]);
        let out = run_capture(cmd).await.map_err(DomainError::Runtime)?;
        Ok(parse_ps_json(&out))
    }

    async fn prune_images(&self, log: Arc<dyn LogSink>) -> Result<(), DomainError> {
        log.line("docker image prune -f ...");
        let mut cmd = Command::new("docker");
        cmd.args(prune_images_args());
        run_streamed(cmd, log).await.map_err(DomainError::Runtime)
    }

    async fn prune_builder(&self, log: Arc<dyn LogSink>) -> Result<(), DomainError> {
        log.line(&format!(
            "docker builder prune -f --filter until={BUILDER_PRUNE_MAX_AGE} ..."
        ));
        let mut cmd = Command::new("docker");
        cmd.args(prune_builder_args());
        run_streamed(cmd, log).await.map_err(DomainError::Runtime)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn stack(workdir: &std::path::Path) -> ComposeStack {
        ComposeStack {
            project_name: "rateme".into(),
            workdir: workdir.to_path_buf(),
            compose_file: workdir.join("docker-compose.yml"),
            override_file: PathBuf::from("/var/lib/pi/overrides/rateme.yml"),
        }
    }

    #[test]
    fn file_chain_without_repo_override() {
        let dir = tempfile::tempdir().unwrap();
        let s = stack(dir.path());
        assert_eq!(
            file_chain(&s),
            vec![s.compose_file.clone(), s.override_file.clone()]
        );
    }

    #[test]
    fn file_chain_includes_repo_override_between_compose_and_pi_override() {
        let dir = tempfile::tempdir().unwrap();
        let repo_override = dir.path().join("docker-compose.override.yml");
        std::fs::write(&repo_override, "services: {}").unwrap();
        let s = stack(dir.path());
        assert_eq!(
            file_chain(&s),
            vec![
                s.compose_file.clone(),
                repo_override,
                s.override_file.clone()
            ]
        );
    }

    #[test]
    fn compose_args_shape() {
        let files = vec![PathBuf::from("a.yml"), PathBuf::from("b.yml")];
        let args = compose_args("rateme", &files, &["up", "-d"]);
        let expected: Vec<std::ffi::OsString> = [
            "compose", "-p", "rateme", "-f", "a.yml", "-f", "b.yml", "up", "-d",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        assert_eq!(args, expected);
    }

    #[test]
    fn prune_args_shapes() {
        assert_eq!(
            prune_images_args(),
            vec!["image".to_string(), "prune".to_string(), "-f".to_string()]
        );
        assert_eq!(
            prune_builder_args(),
            vec![
                "builder".to_string(),
                "prune".to_string(),
                "-f".to_string(),
                "--filter".to_string(),
                format!("until={BUILDER_PRUNE_MAX_AGE}"),
            ]
        );
    }

    #[test]
    fn parse_ps_json_reads_ndjson_lines() {
        let out = concat!(
            r#"{"Service":"web","State":"running","Name":"rateme-web-1"}"#,
            "\n",
            r#"{"Service":"db","State":"exited","Name":"rateme-db-1"}"#,
            "\n",
            "garbage-line\n"
        );
        assert_eq!(
            parse_ps_json(out),
            vec![
                ServiceState {
                    service: "web".into(),
                    state: "running".into(),
                    health: None
                },
                ServiceState {
                    service: "db".into(),
                    state: "exited".into(),
                    health: None
                },
            ]
        );
    }

    #[test]
    fn parse_ps_json_reads_health_field() {
        let out = concat!(
            r#"{"Service":"web","State":"running","Health":"healthy"}"#,
            "\n",
            r#"{"Service":"db","State":"running","Health":""}"#,
            "\n",
            r#"{"Service":"worker","State":"running"}"#,
            "\n",
        );
        let states = parse_ps_json(out);
        assert_eq!(states[0].health.as_deref(), Some("healthy"));
        assert_eq!(states[1].health, None, "empty Health means no healthcheck");
        assert_eq!(states[2].health, None);
    }

    #[test]
    fn parse_ps_json_reads_legacy_array_format() {
        let out = r#"[{"Service":"web","State":"running"},{"Service":"db","State":"exited"}]"#;
        assert_eq!(
            parse_ps_json(out),
            vec![
                ServiceState {
                    service: "web".into(),
                    state: "running".into(),
                    health: None
                },
                ServiceState {
                    service: "db".into(),
                    state: "exited".into(),
                    health: None
                },
            ]
        );
    }
}
