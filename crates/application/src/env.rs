use std::sync::Arc;

use pi_domain::contracts::{
    ContainerRuntime, EnvFileWriter, LogSink, OverrideStore, ProjectRepository, SecretStore, Source,
};
use pi_domain::entities::{ComposeStack, EnvBundle};
use pi_domain::error::DomainError;

use crate::mask::MaskingSink;

/// Result of `pi env send` (§10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvSaved {
    pub keys: usize,
    pub applied: bool,
}

/// Accept and store an EnvBundle; with `apply` re-injects `.env` and runs
/// `up -d` so compose recreates only the affected services (§7, §10).
pub struct SendEnv {
    secrets: Arc<dyn SecretStore>,
    projects: Arc<dyn ProjectRepository>,
    source: Arc<dyn Source>,
    env_files: Arc<dyn EnvFileWriter>,
    overrides: Arc<dyn OverrideStore>,
    runtime: Arc<dyn ContainerRuntime>,
}

impl SendEnv {
    pub fn new(
        secrets: Arc<dyn SecretStore>,
        projects: Arc<dyn ProjectRepository>,
        source: Arc<dyn Source>,
        env_files: Arc<dyn EnvFileWriter>,
        overrides: Arc<dyn OverrideStore>,
        runtime: Arc<dyn ContainerRuntime>,
    ) -> Arc<SendEnv> {
        Arc::new(SendEnv {
            secrets,
            projects,
            source,
            env_files,
            overrides,
            runtime,
        })
    }

    pub async fn execute(
        &self,
        project: &str,
        bundle: EnvBundle,
        apply: bool,
        log: Arc<dyn LogSink>,
    ) -> Result<EnvSaved, DomainError> {
        if bundle.is_empty() {
            return Err(DomainError::Invalid("env bundle is empty".into()));
        }
        self.secrets.save(project, &bundle).await?;
        let keys = bundle.vars.len();
        if !apply {
            return Ok(EnvSaved {
                keys,
                applied: false,
            });
        }

        let registered = self.projects.get(project).await?.ok_or_else(|| {
            DomainError::NotFound(format!(
                "project '{project}' is not deployed yet; run `pi deploy` first"
            ))
        })?;
        let config = &registered.config;

        // mask the freshly received values in the `up` output (§8.1)
        let masker = MaskingSink::new(log);
        masker.arm(&bundle);
        let log: Arc<dyn LogSink> = masker;

        let workdir = self.source.workdir(project);
        self.env_files.write(&workdir, &bundle).await?;
        let override_file = self
            .overrides
            .write(
                project,
                &config.service,
                registered.config.expose.bind_addr(),
                registered.host_port,
                config.container_port,
            )
            .await?;
        let stack = ComposeStack {
            project_name: config.name.clone(),
            workdir: workdir.clone(),
            compose_file: workdir.join(&config.compose_path),
            override_file,
        };
        self.runtime.up(&stack, log).await?;
        Ok(EnvSaved {
            keys,
            applied: true,
        })
    }
}

/// Key names only, never values (§10: `pi env ls`).
pub struct ListEnvKeys {
    secrets: Arc<dyn SecretStore>,
}

impl ListEnvKeys {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Arc<ListEnvKeys> {
        Arc::new(ListEnvKeys { secrets })
    }

    pub async fn execute(&self, project: &str) -> Result<Vec<String>, DomainError> {
        Ok(self.secrets.load(project).await?.keys())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CollectSink;
    use pi_domain::contracts::{
        MockContainerRuntime, MockEnvFileWriter, MockOverrideStore, MockProjectRepository,
        MockSecretStore, MockSource,
    };
    use pi_domain::entities::{
        ExposeMode, HealthcheckConfig, Project, ProjectConfig, StageTimeoutOverrides,
    };
    use std::path::{Path, PathBuf};

    fn bundle() -> EnvBundle {
        let mut b = EnvBundle::default();
        b.vars.insert("DB_PASSWORD".into(), "hunter2-long".into());
        b.vars.insert("PORT".into(), "3000".into());
        b
    }

    fn registered() -> Project {
        Project {
            config: ProjectConfig {
                name: "rateme".into(),
                repo: "git@github.com:x/rateme.git".into(),
                branch: "main".into(),
                compose_path: "docker-compose.yml".into(),
                service: "web".into(),
                container_port: 3000,
                hostname: None,
                expose: ExposeMode::default(),
                healthcheck: HealthcheckConfig::default(),
                timeouts: StageTimeoutOverrides::default(),
            },
            host_port: 8000,
            created_at: 1,
        }
    }

    struct Mocks {
        secrets: MockSecretStore,
        projects: MockProjectRepository,
        source: MockSource,
        env_files: MockEnvFileWriter,
        overrides: MockOverrideStore,
        runtime: MockContainerRuntime,
    }

    fn mocks() -> Mocks {
        Mocks {
            secrets: MockSecretStore::new(),
            projects: MockProjectRepository::new(),
            source: MockSource::new(),
            env_files: MockEnvFileWriter::new(),
            overrides: MockOverrideStore::new(),
            runtime: MockContainerRuntime::new(),
        }
    }

    fn build(m: Mocks) -> Arc<SendEnv> {
        SendEnv::new(
            Arc::new(m.secrets),
            Arc::new(m.projects),
            Arc::new(m.source),
            Arc::new(m.env_files),
            Arc::new(m.overrides),
            Arc::new(m.runtime),
        )
    }

    #[tokio::test]
    async fn save_without_apply_only_stores_bundle() {
        let mut m = mocks();
        m.secrets
            .expect_save()
            .withf(|p, b| p == "rateme" && b.vars.len() == 2)
            .times(1)
            .returning(|_, _| Ok(()));

        let saved = build(m)
            .execute("rateme", bundle(), false, CollectSink::new())
            .await
            .unwrap();
        assert_eq!(
            saved,
            EnvSaved {
                keys: 2,
                applied: false
            }
        );
    }

    #[tokio::test]
    async fn empty_bundle_is_invalid_and_not_saved() {
        let mut m = mocks();
        m.secrets.expect_save().times(0);
        let err = build(m)
            .execute("rateme", EnvBundle::default(), false, CollectSink::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Invalid(_)), "got: {err}");
    }

    #[tokio::test]
    async fn apply_reinjects_env_and_runs_up_with_masked_logs() {
        let mut m = mocks();
        m.secrets.expect_save().returning(|_, _| Ok(()));
        m.projects
            .expect_get()
            .withf(|n| n == "rateme")
            .returning(|_| Ok(Some(registered())));
        m.source
            .expect_workdir()
            .withf(|n| n == "rateme")
            .returning(|_| PathBuf::from("/wd/rateme"));
        m.env_files
            .expect_write()
            .withf(|wd, b| wd == Path::new("/wd/rateme") && b.vars.len() == 2)
            .times(1)
            .returning(|_, _| Ok(()));
        m.overrides
            .expect_write()
            .withf(|p, s, bind, hp, cp| {
                p == "rateme"
                    && s == "web"
                    && bind == "127.0.0.1"
                    && *hp == 8000
                    && *cp == 3000
            })
            .times(1)
            .returning(|_, _, _, _, _| Ok(PathBuf::from("/ov/rateme.yml")));
        m.runtime
            .expect_up()
            .withf(|stack, _| {
                stack.project_name == "rateme"
                    && stack.workdir == PathBuf::from("/wd/rateme")
                    && stack.compose_file == PathBuf::from("/wd/rateme/docker-compose.yml")
                    && stack.override_file == PathBuf::from("/ov/rateme.yml")
            })
            .times(1)
            .returning(|_, log| {
                log.line("recreating with hunter2-long");
                Ok(())
            });

        let sink = CollectSink::new();
        let saved = build(m)
            .execute("rateme", bundle(), true, sink.clone())
            .await
            .unwrap();

        assert_eq!(
            saved,
            EnvSaved {
                keys: 2,
                applied: true
            }
        );
        let lines = sink.lines.lock().unwrap();
        assert!(
            lines.iter().any(|l| l.contains("***DB_PASSWORD***")),
            "lines: {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("hunter2-long")),
            "secret leaked"
        );
    }

    #[tokio::test]
    async fn apply_for_unknown_project_is_not_found_after_save() {
        let mut m = mocks();
        m.secrets.expect_save().times(1).returning(|_, _| Ok(()));
        m.projects.expect_get().returning(|_| Ok(None));
        m.env_files.expect_write().times(0);
        m.runtime.expect_up().times(0);

        let err = build(m)
            .execute("rateme", bundle(), true, CollectSink::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::NotFound(_)), "got: {err}");
    }

    #[tokio::test]
    async fn list_env_keys_returns_names_only() {
        let mut secrets = MockSecretStore::new();
        secrets
            .expect_load()
            .withf(|p| p == "rateme")
            .returning(|_| Ok(bundle()));
        let keys = ListEnvKeys::new(Arc::new(secrets))
            .execute("rateme")
            .await
            .unwrap();
        assert_eq!(keys, vec!["DB_PASSWORD".to_string(), "PORT".to_string()]);
    }
}
