use std::sync::Arc;

use pi_domain::contracts::{
    ContainerRuntime, DeploymentHistory, LogSink, OverrideStore, ProjectRepository, Source,
};
use pi_domain::entities::{ComposeStack, Project};
use pi_domain::error::DomainError;

use crate::remove::RemoveProject;

/// `GET /v1/environments` (`rpi env ls`, environment-overlays spec): pure
/// pass-through of `ProjectRepository::list_environments`, kept as a
/// use-case so the HTTP layer never talks to the registry directly.
pub struct ListEnvironments {
    projects: Arc<dyn ProjectRepository>,
}

impl ListEnvironments {
    pub fn new(projects: Arc<dyn ProjectRepository>) -> Arc<ListEnvironments> {
        Arc::new(ListEnvironments { projects })
    }

    pub async fn execute(&self, base: Option<&str>) -> Result<Vec<Project>, DomainError> {
        self.projects.list_environments(base).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestroyOutcome {
    pub key: String,
    pub already_absent: bool,
}

/// `DELETE /v1/environments/{key}` (`rpi env rm`): tears an environment
/// overlay down completely. A missing key is not an error (idempotent
/// delete); a base-project key is rejected — `rpi rm` owns that path. The
/// active-deploy conflict check lives entirely in [`RemoveProject`], which
/// does the actual teardown — this use-case must not duplicate it.
pub struct DestroyEnvironment {
    projects: Arc<dyn ProjectRepository>,
    remove: Arc<RemoveProject>,
}

impl DestroyEnvironment {
    pub fn new(
        projects: Arc<dyn ProjectRepository>,
        remove: Arc<RemoveProject>,
    ) -> Arc<DestroyEnvironment> {
        Arc::new(DestroyEnvironment { projects, remove })
    }

    pub async fn execute(
        &self,
        key: &str,
        log: Arc<dyn LogSink>,
    ) -> Result<DestroyOutcome, DomainError> {
        let Some(existing) = self.projects.get(key).await? else {
            return Ok(DestroyOutcome {
                key: key.to_string(),
                already_absent: true,
            });
        };
        if existing.config.environment.is_none() {
            return Err(DomainError::Conflict(format!(
                "'{key}' is a base project, not an environment - use `rpi rm` for base projects"
            )));
        }
        self.remove.execute(key, true, log).await?;
        Ok(DestroyOutcome {
            key: key.to_string(),
            already_absent: false,
        })
    }
}

/// `POST /v1/environments/{key}/reset-data` (`rpi env reset-data`): drops
/// the overlay's containers and named volumes without deregistering the
/// project, and clears `on_create_done` so the next deploy re-runs the
/// overlay's `on_create` hook against a clean database. Unlike
/// [`DestroyEnvironment`], this use-case owns its own active-deploy guard —
/// there is no `RemoveProject` delegate here to do it for us.
pub struct ResetEnvironmentData {
    projects: Arc<dyn ProjectRepository>,
    history: Arc<dyn DeploymentHistory>,
    runtime: Arc<dyn ContainerRuntime>,
    source: Arc<dyn Source>,
    overrides: Arc<dyn OverrideStore>,
}

impl ResetEnvironmentData {
    pub fn new(
        projects: Arc<dyn ProjectRepository>,
        history: Arc<dyn DeploymentHistory>,
        runtime: Arc<dyn ContainerRuntime>,
        source: Arc<dyn Source>,
        overrides: Arc<dyn OverrideStore>,
    ) -> Arc<ResetEnvironmentData> {
        Arc::new(ResetEnvironmentData {
            projects,
            history,
            runtime,
            source,
            overrides,
        })
    }

    pub async fn execute(&self, key: &str, log: Arc<dyn LogSink>) -> Result<(), DomainError> {
        let Some(existing) = self.projects.get(key).await? else {
            return Err(DomainError::NotFound(format!("environment {key}")));
        };
        if existing.config.environment.is_none() {
            return Err(DomainError::Conflict(format!(
                "'{key}' is a base project, not an environment"
            )));
        }
        if !self.history.active(key).await?.is_empty() {
            return Err(DomainError::Conflict(format!(
                "environment {key} has an active deployment; wait for it or cancel it first"
            )));
        }
        let workdir = self.source.workdir(key);
        let compose_file = workdir.join(&existing.config.compose_path);
        if compose_file.exists() {
            let stack = ComposeStack {
                project_name: existing.config.name.clone(),
                workdir,
                compose_file,
                override_file: self.overrides.path(key),
            };
            self.runtime.down(&stack, true, Arc::clone(&log)).await?;
        }
        self.projects.set_on_create_done(key, false).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CollectSink;
    use pi_domain::contracts::{
        MockContainerRuntime, MockDeploymentHistory, MockIngress, MockOverrideStore,
        MockProjectRepository, MockSecretStore, MockSource,
    };
    use pi_domain::entities::{Deployment, DeploymentStatus, EnvironmentMeta, ProjectConfig};
    use std::path::PathBuf;

    fn env_meta() -> EnvironmentMeta {
        EnvironmentMeta {
            env: "test".into(),
            base: "myapp".into(),
            slug: None,
            ttl_secs: Some(3600),
            on_create: None,
        }
    }

    fn project(name: &str, environment: Option<EnvironmentMeta>) -> Project {
        Project {
            config: ProjectConfig {
                name: name.into(),
                repo: "https://github.com/x/y.git".into(),
                branch: "main".into(),
                compose_path: "docker-compose.yml".into(),
                service: "web".into(),
                container_port: 3000,
                hostname: Some("app.example.com".into()),
                expose: Default::default(),
                healthcheck: Default::default(),
                timeouts: Default::default(),
                commands: Default::default(),
                command_timeout_secs: None,
                environment,
            },
            host_port: 8000,
            created_at: 1,
            on_create_done: false,
            last_success_at: None,
        }
    }

    /// `RemoveProject` wired with mocks that carry no expectations — used
    /// where `DestroyEnvironment` must short-circuit before ever delegating
    /// to it (missing key / base-project key). Calling any of these mocks
    /// unexpectedly is a test failure (mockall panics), which is exactly the
    /// assertion we want: the delegate was never touched.
    fn untouched_remove(projects: Arc<dyn ProjectRepository>) -> Arc<RemoveProject> {
        RemoveProject::new(
            projects,
            Arc::new(MockDeploymentHistory::new()),
            Arc::new(MockContainerRuntime::new()),
            Arc::new(MockIngress::new()),
            Arc::new(MockSource::new()),
            Arc::new(MockSecretStore::new()),
            Arc::new(MockOverrideStore::new()),
        )
    }

    /// A real (unique) temp directory containing a `docker-compose.yml`, so
    /// `compose_file.exists()` is true for the stack-teardown path. Cleaned
    /// up best-effort on drop of the returned guard.
    struct ComposeDir(PathBuf);
    impl ComposeDir {
        fn new(label: &str) -> ComposeDir {
            let dir = std::env::temp_dir().join(format!(
                "pi-application-environments-test-{label}-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("docker-compose.yml"), "services: {}\n").unwrap();
            ComposeDir(dir)
        }
    }
    impl Drop for ComposeDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn list_delegates_to_project_repository_with_base_filter() {
        let expected = vec![project("myapp--test", Some(env_meta()))];
        let returned = expected.clone();
        let mut projects = MockProjectRepository::new();
        projects
            .expect_list_environments()
            .withf(|base: &Option<&str>| *base == Some("myapp"))
            .times(1)
            .returning(move |_| Ok(returned.clone()));

        let list = ListEnvironments::new(Arc::new(projects));
        let got = list.execute(Some("myapp")).await.unwrap();
        assert_eq!(got, expected);
    }

    #[tokio::test]
    async fn destroy_missing_key_is_ok_already_absent() {
        let mut projects = MockProjectRepository::new();
        projects.expect_get().times(1).returning(|_| Ok(None));
        let projects: Arc<dyn ProjectRepository> = Arc::new(projects);

        let remove = untouched_remove(Arc::clone(&projects));
        let destroy = DestroyEnvironment::new(Arc::clone(&projects), remove);

        let outcome = destroy
            .execute("ghost--test", CollectSink::new())
            .await
            .unwrap();
        assert_eq!(
            outcome,
            DestroyOutcome {
                key: "ghost--test".into(),
                already_absent: true,
            }
        );
    }

    #[tokio::test]
    async fn destroy_base_key_is_conflict() {
        let proj = project("myapp", None);
        let mut projects = MockProjectRepository::new();
        projects
            .expect_get()
            .times(1)
            .returning(move |_| Ok(Some(proj.clone())));
        let projects: Arc<dyn ProjectRepository> = Arc::new(projects);

        let remove = untouched_remove(Arc::clone(&projects));
        let destroy = DestroyEnvironment::new(Arc::clone(&projects), remove);

        let err = destroy
            .execute("myapp", CollectSink::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Conflict(_)), "got: {err}");
    }

    #[tokio::test]
    async fn destroy_environment_delegates_to_remove_with_volumes() {
        let dir = ComposeDir::new("destroy");
        let key = "myapp--test";
        let proj = project(key, Some(env_meta()));

        let mut projects = MockProjectRepository::new();
        let get_proj = proj.clone();
        // Called once by DestroyEnvironment's own kind guard, once more
        // inside RemoveProject::execute.
        projects
            .expect_get()
            .times(2)
            .returning(move |_| Ok(Some(get_proj.clone())));
        projects.expect_remove().times(1).returning(|_| Ok(()));
        let projects: Arc<dyn ProjectRepository> = Arc::new(projects);

        let mut history = MockDeploymentHistory::new();
        history.expect_active().times(1).returning(|_| Ok(vec![]));
        history
            .expect_remove_project()
            .times(1)
            .returning(|_| Ok(()));
        let history: Arc<dyn DeploymentHistory> = Arc::new(history);

        let mut runtime = MockContainerRuntime::new();
        runtime
            .expect_down()
            .withf(|_, remove_volumes, _| *remove_volumes)
            .times(1)
            .returning(|_, _, _| Ok(()));
        let runtime: Arc<dyn ContainerRuntime> = Arc::new(runtime);

        let mut ingress = MockIngress::new();
        ingress
            .expect_remove()
            .withf(|hostname, _| hostname == "app.example.com")
            .times(1)
            .returning(|_, _| Ok(()));
        let ingress: Arc<dyn pi_domain::contracts::Ingress> = Arc::new(ingress);

        let mut source = MockSource::new();
        let workdir = dir.0.clone();
        source.expect_workdir().returning(move |_| workdir.clone());
        source.expect_cleanup().times(1).returning(|_| Ok(()));
        let source: Arc<dyn Source> = Arc::new(source);

        let mut secrets = MockSecretStore::new();
        secrets.expect_remove().times(1).returning(|_| Ok(()));
        let secrets: Arc<dyn pi_domain::contracts::SecretStore> = Arc::new(secrets);

        let mut overrides = MockOverrideStore::new();
        overrides
            .expect_path()
            .returning(|name| PathBuf::from("/overrides").join(name));
        overrides.expect_remove().times(1).returning(|_| Ok(()));
        let overrides: Arc<dyn OverrideStore> = Arc::new(overrides);

        let remove = RemoveProject::new(
            Arc::clone(&projects),
            Arc::clone(&history),
            runtime,
            ingress,
            source,
            secrets,
            overrides,
        );
        let destroy = DestroyEnvironment::new(Arc::clone(&projects), remove);

        let outcome = destroy.execute(key, CollectSink::new()).await.unwrap();
        assert_eq!(
            outcome,
            DestroyOutcome {
                key: key.into(),
                already_absent: false,
            }
        );
    }

    #[tokio::test]
    async fn reset_data_downs_volumes_and_clears_flag() {
        let dir = ComposeDir::new("reset-ok");
        let key = "myapp--test";
        let proj = project(key, Some(env_meta()));

        let mut projects = MockProjectRepository::new();
        projects
            .expect_get()
            .times(1)
            .returning(move |_| Ok(Some(proj.clone())));
        projects
            .expect_set_on_create_done()
            .withf(|k, done| k == "myapp--test" && !*done)
            .times(1)
            .returning(|_, _| Ok(()));
        let projects: Arc<dyn ProjectRepository> = Arc::new(projects);

        let mut history = MockDeploymentHistory::new();
        history.expect_active().times(1).returning(|_| Ok(vec![]));
        let history: Arc<dyn DeploymentHistory> = Arc::new(history);

        let mut runtime = MockContainerRuntime::new();
        runtime
            .expect_down()
            .withf(|_, remove_volumes, _| *remove_volumes)
            .times(1)
            .returning(|_, _, _| Ok(()));
        let runtime: Arc<dyn ContainerRuntime> = Arc::new(runtime);

        let mut source = MockSource::new();
        let workdir = dir.0.clone();
        source.expect_workdir().returning(move |_| workdir.clone());
        let source: Arc<dyn Source> = Arc::new(source);

        let mut overrides = MockOverrideStore::new();
        overrides
            .expect_path()
            .returning(|name| PathBuf::from("/overrides").join(name));
        let overrides: Arc<dyn OverrideStore> = Arc::new(overrides);

        let reset = ResetEnvironmentData::new(projects, history, runtime, source, overrides);
        reset.execute(key, CollectSink::new()).await.unwrap();
    }

    #[tokio::test]
    async fn reset_data_with_active_deploy_is_conflict() {
        let key = "myapp--test";
        let proj = project(key, Some(env_meta()));
        let mut projects = MockProjectRepository::new();
        projects
            .expect_get()
            .times(1)
            .returning(move |_| Ok(Some(proj.clone())));
        projects.expect_set_on_create_done().times(0);
        let projects: Arc<dyn ProjectRepository> = Arc::new(projects);

        let mut history = MockDeploymentHistory::new();
        history.expect_active().times(1).returning(|_| {
            Ok(vec![Deployment {
                id: "dep-1".into(),
                project: "myapp--test".into(),
                git_ref: "main".into(),
                commit_sha: None,
                status: DeploymentStatus::Running,
                started_at: 1,
                finished_at: None,
                log_tail: String::new(),
            }])
        });
        let history: Arc<dyn DeploymentHistory> = Arc::new(history);

        let mut runtime = MockContainerRuntime::new();
        runtime.expect_down().times(0);
        let runtime: Arc<dyn ContainerRuntime> = Arc::new(runtime);
        let source: Arc<dyn Source> = Arc::new(MockSource::new());
        let overrides: Arc<dyn OverrideStore> = Arc::new(MockOverrideStore::new());

        let reset = ResetEnvironmentData::new(projects, history, runtime, source, overrides);
        let err = reset.execute(key, CollectSink::new()).await.unwrap_err();
        assert!(matches!(err, DomainError::Conflict(_)), "got: {err}");
    }

    #[tokio::test]
    async fn reset_data_on_base_key_is_conflict() {
        let proj = project("myapp", None);
        let mut projects = MockProjectRepository::new();
        projects
            .expect_get()
            .times(1)
            .returning(move |_| Ok(Some(proj.clone())));
        projects.expect_set_on_create_done().times(0);
        let projects: Arc<dyn ProjectRepository> = Arc::new(projects);

        let mut history = MockDeploymentHistory::new();
        history.expect_active().times(0);
        let history: Arc<dyn DeploymentHistory> = Arc::new(history);
        let runtime: Arc<dyn ContainerRuntime> = Arc::new(MockContainerRuntime::new());
        let source: Arc<dyn Source> = Arc::new(MockSource::new());
        let overrides: Arc<dyn OverrideStore> = Arc::new(MockOverrideStore::new());

        let reset = ResetEnvironmentData::new(projects, history, runtime, source, overrides);
        let err = reset
            .execute("myapp", CollectSink::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Conflict(_)), "got: {err}");
    }
}
