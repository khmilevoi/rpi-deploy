use std::sync::Arc;

use pi_domain::contracts::{
    Clock, ContainerRuntime, DeploymentHistory, EnvFileWriter, HealthGate, Ingress, LogSink,
    OverrideStore, ProjectRepository, SecretStore, Source,
};
use pi_domain::entities::{ComposeStack, DeployRef, Deployment, DeploymentStatus, ProjectConfig};
use pi_domain::error::DomainError;

use crate::locks::{DeployLocks, DeployPermit};
use crate::mask::MaskingSink;
use crate::tail::TailSink;

const LOG_TAIL_LINES: usize = 400;

/// Guarantees sending `finished(Failed)` on drop if `disarm()` is not called.
/// Protects against panics and early returns via `?`.
struct FinishGuard {
    sink: Arc<dyn LogSink>,
    armed: bool,
}

impl FinishGuard {
    fn new(sink: Arc<dyn LogSink>) -> Self {
        Self { sink, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for FinishGuard {
    fn drop(&mut self) {
        if self.armed {
            self.sink.finished(DeploymentStatus::Failed);
        }
    }
}

/// Deploy use-case (§7, §8.2). Project lock is acquired via try_begin BEFORE
/// spawning the async task, so the HTTP handler can immediately respond with 409.
pub struct DeployProject {
    source: Arc<dyn Source>,
    runtime: Arc<dyn ContainerRuntime>,
    projects: Arc<dyn ProjectRepository>,
    history: Arc<dyn DeploymentHistory>,
    overrides: Arc<dyn OverrideStore>,
    secrets: Arc<dyn SecretStore>,
    env_files: Arc<dyn EnvFileWriter>,
    health: Arc<dyn HealthGate>,
    ingress: Arc<dyn Ingress>,
    clock: Arc<dyn Clock>,
    locks: Arc<DeployLocks>,
}

impl DeployProject {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source: Arc<dyn Source>,
        runtime: Arc<dyn ContainerRuntime>,
        projects: Arc<dyn ProjectRepository>,
        history: Arc<dyn DeploymentHistory>,
        overrides: Arc<dyn OverrideStore>,
        secrets: Arc<dyn SecretStore>,
        env_files: Arc<dyn EnvFileWriter>,
        health: Arc<dyn HealthGate>,
        ingress: Arc<dyn Ingress>,
        clock: Arc<dyn Clock>,
    ) -> Arc<DeployProject> {
        Arc::new(DeployProject {
            source,
            runtime,
            projects,
            history,
            overrides,
            secrets,
            env_files,
            health,
            ingress,
            clock,
            locks: DeployLocks::new(),
        })
    }

    /// Err(DeployInProgress) — deploy of this project is already in progress (MVP: no queue, §23 v0.1).
    pub fn try_begin(&self, project: &str) -> Result<DeployPermit, DomainError> {
        self.locks
            .try_acquire(project)
            .ok_or_else(|| DomainError::DeployInProgress(project.to_string()))
    }

    pub async fn execute(
        &self,
        permit: DeployPermit,
        deployment_id: String,
        config: ProjectConfig,
        git_ref: DeployRef,
        sink: Arc<dyn LogSink>,
    ) -> Result<Deployment, DomainError> {
        let _permit = permit; // keep lock until deploy finishes, released by Drop

        // chain: stages write to masker → masks secrets → tail stores masked lines → sink (SSE hub)
        let tail = TailSink::new(Arc::clone(&sink), LOG_TAIL_LINES);
        let masker = MaskingSink::new(tail.clone());
        let log: Arc<dyn LogSink> = masker.clone();
        let mut guard = FinishGuard::new(sink);

        let mut deployment = Deployment {
            id: deployment_id,
            project: config.name.clone(),
            git_ref: git_ref.as_str().to_string(),
            commit_sha: None,
            status: DeploymentStatus::Queued,
            started_at: self.clock.now_unix(),
            finished_at: None,
            log_tail: String::new(),
        };
        self.history.record_queued(&deployment).await?;
        deployment.started_at = self.clock.now_unix();
        deployment.status = DeploymentStatus::Running;
        self.history
            .mark_running(&deployment.id, deployment.started_at)
            .await?;

        let result = self
            .run_stages(&config, &git_ref, log.clone(), &masker)
            .await;
        let finished_at = self.clock.now_unix();

        match result {
            Ok(commit_sha) => {
                deployment.status = DeploymentStatus::Success;
                deployment.commit_sha = Some(commit_sha);
                deployment.finished_at = Some(finished_at);
                deployment.log_tail = tail.tail();
                let record_result = self
                    .history
                    .record_finished(
                        &deployment.id,
                        DeploymentStatus::Success,
                        deployment.commit_sha.as_deref(),
                        finished_at,
                        &deployment.log_tail,
                    )
                    .await;
                log.finished(DeploymentStatus::Success);
                guard.disarm();
                record_result?;
                Ok(deployment)
            }
            Err(err) => {
                log.line(&format!("deploy failed: {err}"));
                let log_tail = tail.tail();
                let record_result = self
                    .history
                    .record_finished(
                        &deployment.id,
                        DeploymentStatus::Failed,
                        None,
                        finished_at,
                        &log_tail,
                    )
                    .await;
                log.finished(DeploymentStatus::Failed);
                guard.disarm();
                record_result?;
                Err(err)
            }
        }
    }

    async fn run_stages(
        &self,
        config: &ProjectConfig,
        git_ref: &DeployRef,
        log: Arc<dyn LogSink>,
        masker: &MaskingSink,
    ) -> Result<String, DomainError> {
        let project = self.projects.upsert(config).await?;
        log.line(&format!(
            "project '{}': host port {}",
            project.config.name, project.host_port
        ));

        let fetched = self.source.fetch(config, git_ref, log.clone()).await?;
        log.line(&format!("fetched {}", fetched.commit_sha));

        // §10: decrypt -> arm masking -> inject .env (skip when nothing stored)
        let bundle = self.secrets.load(&config.name).await?;
        if !bundle.is_empty() {
            masker.arm(&bundle);
            self.env_files.write(&fetched.workdir, &bundle).await?;
            log.line(&format!(".env injected ({} keys)", bundle.vars.len()));
        }

        let override_file = self
            .overrides
            .write(
                &config.name,
                &config.service,
                project.host_port,
                config.container_port,
            )
            .await?;

        let stack = ComposeStack {
            project_name: config.name.clone(),
            workdir: fetched.workdir.clone(),
            compose_file: fetched.workdir.join(&config.compose_path),
            override_file,
        };
        self.runtime.build(&stack, log.clone()).await?;
        self.runtime.up(&stack, log.clone()).await?;

        // §8: health gate — on failure the deploy is failed, stack stays up
        self.health
            .check(config, project.host_port, log.clone())
            .await?;

        // §11: route hostname only when configured
        if let Some(hostname) = &config.hostname {
            self.ingress
                .upsert(hostname, project.host_port, log.clone())
                .await?;
        }

        Ok(fetched.commit_sha)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CollectSink;
    use pi_domain::contracts::{
        MockClock, MockContainerRuntime, MockDeploymentHistory, MockEnvFileWriter, MockHealthGate,
        MockIngress, MockOverrideStore, MockProjectRepository, MockSecretStore, MockSource,
    };
    use pi_domain::entities::{
        DeployRef, DeploymentStatus, EnvBundle, FetchedSource, HealthcheckConfig, Project,
        ProjectConfig, StageTimeoutOverrides,
    };
    use pi_domain::error::DomainError;
    use std::{
        path::{Path, PathBuf},
        sync::{Arc, Mutex},
    };

    pub fn sample_config() -> ProjectConfig {
        ProjectConfig {
            name: "rateme".into(),
            repo: "git@github.com:isskelo/rateme.git".into(),
            branch: "main".into(),
            compose_path: "docker-compose.yml".into(),
            service: "web".into(),
            container_port: 3000,
            hostname: Some("rateme.isskelo.com".into()),
            healthcheck: HealthcheckConfig::default(),
            timeouts: StageTimeoutOverrides::default(),
        }
    }

    pub struct Mocks {
        pub source: MockSource,
        pub runtime: MockContainerRuntime,
        pub projects: MockProjectRepository,
        pub history: MockDeploymentHistory,
        pub overrides: MockOverrideStore,
        pub secrets: MockSecretStore,
        pub env_files: MockEnvFileWriter,
        pub health: MockHealthGate,
        pub ingress: MockIngress,
        pub clock: MockClock,
    }

    pub fn mocks() -> Mocks {
        let mut clock = MockClock::new();
        clock.expect_now_unix().return_const(100i64);
        Mocks {
            source: MockSource::new(),
            runtime: MockContainerRuntime::new(),
            projects: MockProjectRepository::new(),
            history: MockDeploymentHistory::new(),
            overrides: MockOverrideStore::new(),
            secrets: MockSecretStore::new(),
            env_files: MockEnvFileWriter::new(),
            health: MockHealthGate::new(),
            ingress: MockIngress::new(),
            clock,
        }
    }

    pub fn build(m: Mocks) -> Arc<DeployProject> {
        DeployProject::new(
            Arc::new(m.source),
            Arc::new(m.runtime),
            Arc::new(m.projects),
            Arc::new(m.history),
            Arc::new(m.overrides),
            Arc::new(m.secrets),
            Arc::new(m.env_files),
            Arc::new(m.health),
            Arc::new(m.ingress),
            Arc::new(m.clock),
        )
    }

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    #[tokio::test]
    async fn happy_path_runs_all_stages_and_records_success() {
        let mut m = mocks();
        let cfg = sample_config();
        let order = Arc::new(Mutex::new(Vec::new()));

        let stage_order = Arc::clone(&order);
        m.projects.expect_upsert().times(1).returning(move |c| {
            stage_order.lock().unwrap().push("upsert");
            Ok(Project {
                config: c.clone(),
                host_port: 8000,
                created_at: 1,
            })
        });
        let stage_order = Arc::clone(&order);
        m.source.expect_fetch().times(1).returning(move |_, _, _| {
            stage_order.lock().unwrap().push("fetch");
            Ok(FetchedSource {
                workdir: PathBuf::from("/var/lib/pi/workdirs/rateme"),
                commit_sha: SHA.into(),
            })
        });
        let stage_order = Arc::clone(&order);
        m.secrets.expect_load().times(1).returning(move |_| {
            stage_order.lock().unwrap().push("secrets");
            Ok(EnvBundle::default())
        });
        // empty bundle -> .env must NOT be written
        m.env_files.expect_write().times(0);
        let stage_order = Arc::clone(&order);
        m.overrides
            .expect_write()
            .withf(|p, s, hp, cp| p == "rateme" && s == "web" && *hp == 8000 && *cp == 3000)
            .times(1)
            .returning(move |_, _, _, _| {
                stage_order.lock().unwrap().push("override");
                Ok(PathBuf::from("/var/lib/pi/overrides/rateme.yml"))
            });
        let stage_order = Arc::clone(&order);
        m.runtime
            .expect_build()
            .withf(|stack, _| {
                stack.project_name == "rateme"
                    && stack.compose_file
                        == PathBuf::from("/var/lib/pi/workdirs/rateme/docker-compose.yml")
                    && stack.override_file == PathBuf::from("/var/lib/pi/overrides/rateme.yml")
            })
            .times(1)
            .returning(move |_, _| {
                stage_order.lock().unwrap().push("build");
                Ok(())
            });
        let stage_order = Arc::clone(&order);
        m.runtime
            .expect_up()
            .withf(|stack, _| {
                stack.project_name == "rateme"
                    && stack.compose_file
                        == PathBuf::from("/var/lib/pi/workdirs/rateme/docker-compose.yml")
                    && stack.override_file == PathBuf::from("/var/lib/pi/overrides/rateme.yml")
            })
            .times(1)
            .returning(move |_, _| {
                stage_order.lock().unwrap().push("up");
                Ok(())
            });
        let stage_order = Arc::clone(&order);
        m.health
            .expect_check()
            .withf(|c, hp, _| c.name == "rateme" && *hp == 8000)
            .times(1)
            .returning(move |_, _, _| {
                stage_order.lock().unwrap().push("health");
                Ok(())
            });
        let stage_order = Arc::clone(&order);
        m.ingress
            .expect_upsert()
            .withf(|h, hp, _| h == "rateme.isskelo.com" && *hp == 8000)
            .times(1)
            .returning(move |_, _, _| {
                stage_order.lock().unwrap().push("ingress");
                Ok(())
            });
        let stage_order = Arc::clone(&order);
        m.history
            .expect_record_queued()
            .withf(|d| {
                d.id == "dep-1" && d.status == DeploymentStatus::Queued && d.git_ref == "main"
            })
            .times(1)
            .returning(move |_| {
                stage_order.lock().unwrap().push("queued");
                Ok(())
            });
        let stage_order = Arc::clone(&order);
        m.history
            .expect_mark_running()
            .withf(|id, started_at| id == "dep-1" && *started_at == 100)
            .times(1)
            .returning(move |_, _| {
                stage_order.lock().unwrap().push("running");
                Ok(())
            });
        let stage_order = Arc::clone(&order);
        m.history
            .expect_record_finished()
            .withf(|id, status, sha, finished_at, tail| {
                id == "dep-1"
                    && *status == DeploymentStatus::Success
                    && sha == &Some(SHA)
                    && *finished_at == 100
                    && tail.contains("project 'rateme': host port 8000")
                    && tail.contains(&format!("fetched {SHA}"))
            })
            .times(1)
            .returning(move |_, _, _, _, _| {
                stage_order.lock().unwrap().push("finished");
                Ok(())
            });

        let deploy = build(m);
        let sink = CollectSink::new();
        let permit = deploy.try_begin("rateme").unwrap();
        let result = deploy
            .execute(
                permit,
                "dep-1".into(),
                cfg,
                DeployRef::Branch("main".into()),
                sink.clone(),
            )
            .await
            .unwrap();

        assert_eq!(result.status, DeploymentStatus::Success);
        assert_eq!(result.commit_sha.as_deref(), Some(SHA));
        assert_eq!(result.finished_at, Some(100));
        assert_eq!(
            *sink.finished.lock().unwrap(),
            vec![DeploymentStatus::Success]
        );
        assert_eq!(
            *order.lock().unwrap(),
            vec![
                "queued", "running", "upsert", "fetch", "secrets", "override", "build", "up",
                "health", "ingress", "finished"
            ]
        );
    }

    #[tokio::test]
    async fn build_failure_records_failed_and_emits_finished_failed() {
        let mut m = mocks();
        m.projects.expect_upsert().returning(|c| {
            Ok(Project {
                config: c.clone(),
                host_port: 8000,
                created_at: 1,
            })
        });
        m.source.expect_fetch().returning(|_, _, _| {
            Ok(FetchedSource {
                workdir: PathBuf::from("/wd"),
                commit_sha: SHA.into(),
            })
        });
        m.secrets
            .expect_load()
            .returning(|_| Ok(EnvBundle::default()));
        m.env_files.expect_write().times(0);
        m.overrides
            .expect_write()
            .returning(|_, _, _, _| Ok(PathBuf::from("/ov.yml")));
        m.runtime
            .expect_build()
            .returning(|_, _| Err(DomainError::Runtime("compose build exited with 1".into())));
        m.runtime.expect_up().times(0);
        m.health.expect_check().times(0);
        m.ingress.expect_upsert().times(0);
        m.history.expect_record_queued().returning(|_| Ok(()));
        m.history.expect_mark_running().returning(|_, _| Ok(()));
        m.history
            .expect_record_finished()
            .withf(|id, status, sha, _at, tail| {
                id == "dep-2"
                    && *status == DeploymentStatus::Failed
                    && sha.is_none()
                    && tail.contains("compose build exited with 1")
            })
            .times(1)
            .returning(|_, _, _, _, _| Ok(()));

        let deploy = build(m);
        let sink = CollectSink::new();
        let permit = deploy.try_begin("rateme").unwrap();
        let err = deploy
            .execute(
                permit,
                "dep-2".into(),
                sample_config(),
                DeployRef::Branch("main".into()),
                sink.clone(),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, DomainError::Runtime(_)));
        assert_eq!(
            *sink.finished.lock().unwrap(),
            vec![DeploymentStatus::Failed]
        );
        assert!(
            deploy.try_begin("rateme").is_ok(),
            "lock must be free after failed deploy"
        );
    }

    #[tokio::test]
    async fn try_begin_twice_returns_deploy_in_progress() {
        let deploy = build(mocks());
        let _permit = deploy.try_begin("rateme").unwrap();
        let err = match deploy.try_begin("rateme") {
            Ok(_) => panic!("second try_begin must fail while deploy is in progress"),
            Err(err) => err,
        };
        assert!(matches!(err, DomainError::DeployInProgress(p) if p == "rateme"));
    }

    #[tokio::test]
    async fn lock_released_after_execute_finishes() {
        let mut m = mocks();
        m.projects.expect_upsert().returning(|c| {
            Ok(Project {
                config: c.clone(),
                host_port: 8000,
                created_at: 1,
            })
        });
        m.source.expect_fetch().returning(|_, _, _| {
            Ok(FetchedSource {
                workdir: PathBuf::from("/wd"),
                commit_sha: SHA.into(),
            })
        });
        m.secrets
            .expect_load()
            .returning(|_| Ok(EnvBundle::default()));
        m.env_files.expect_write().times(0);
        m.overrides
            .expect_write()
            .returning(|_, _, _, _| Ok(PathBuf::from("/ov.yml")));
        m.runtime.expect_build().returning(|_, _| Ok(()));
        m.runtime.expect_up().returning(|_, _| Ok(()));
        m.health.expect_check().returning(|_, _, _| Ok(()));
        m.ingress.expect_upsert().returning(|_, _, _| Ok(()));
        m.history.expect_record_queued().returning(|_| Ok(()));
        m.history.expect_mark_running().returning(|_, _| Ok(()));
        m.history
            .expect_record_finished()
            .returning(|_, _, _, _, _| Ok(()));

        let deploy = build(m);
        let permit = deploy.try_begin("rateme").unwrap();
        deploy
            .execute(
                permit,
                "dep-3".into(),
                sample_config(),
                DeployRef::Branch("main".into()),
                CollectSink::new(),
            )
            .await
            .unwrap();
        assert!(
            deploy.try_begin("rateme").is_ok(),
            "lock must be free after deploy"
        );
    }

    fn secret_bundle() -> EnvBundle {
        let mut b = EnvBundle::default();
        b.vars.insert("DB_PASSWORD".into(), "hunter2-long".into());
        b
    }

    fn ok_pre_stages(m: &mut Mocks) {
        m.projects.expect_upsert().returning(|c| {
            Ok(Project {
                config: c.clone(),
                host_port: 8000,
                created_at: 1,
            })
        });
        m.source.expect_fetch().returning(|_, _, _| {
            Ok(FetchedSource {
                workdir: PathBuf::from("/wd"),
                commit_sha: SHA.into(),
            })
        });
        m.overrides
            .expect_write()
            .returning(|_, _, _, _| Ok(PathBuf::from("/ov.yml")));
        m.history.expect_record_queued().returning(|_| Ok(()));
        m.history.expect_mark_running().returning(|_, _| Ok(()));
        m.history
            .expect_record_finished()
            .returning(|_, _, _, _, _| Ok(()));
    }

    #[tokio::test]
    async fn stored_bundle_is_written_to_workdir_and_masked_in_logs() {
        let mut m = mocks();
        ok_pre_stages(&mut m);
        m.secrets.expect_load().returning(|_| Ok(secret_bundle()));
        m.env_files
            .expect_write()
            .withf(|wd, b| wd == Path::new("/wd") && b.vars.contains_key("DB_PASSWORD"))
            .times(1)
            .returning(|_, _| Ok(()));
        m.runtime.expect_build().returning(|_, log| {
            log.line("connecting with hunter2-long");
            Ok(())
        });
        m.runtime.expect_up().returning(|_, _| Ok(()));
        m.health.expect_check().returning(|_, _, _| Ok(()));
        m.ingress.expect_upsert().returning(|_, _, _| Ok(()));

        let deploy = build(m);
        let sink = CollectSink::new();
        let permit = deploy.try_begin("rateme").unwrap();
        let result = deploy
            .execute(
                permit,
                "dep-env".into(),
                sample_config(),
                DeployRef::Branch("main".into()),
                sink.clone(),
            )
            .await
            .unwrap();

        assert_eq!(result.status, DeploymentStatus::Success);
        assert!(result.log_tail.contains(".env injected (1 keys)"));
        assert!(
            result.log_tail.contains("***DB_PASSWORD***"),
            "tail: {}",
            result.log_tail
        );
        assert!(
            !result.log_tail.contains("hunter2-long"),
            "secret leaked into tail"
        );
        let lines = sink.lines.lock().unwrap();
        assert!(lines.iter().any(|l| l.contains("***DB_PASSWORD***")));
        assert!(
            !lines.iter().any(|l| l.contains("hunter2-long")),
            "secret leaked into stream"
        );
    }

    #[tokio::test]
    async fn health_gate_failure_fails_deploy_and_skips_ingress() {
        let mut m = mocks();
        ok_pre_stages(&mut m);
        m.secrets
            .expect_load()
            .returning(|_| Ok(EnvBundle::default()));
        m.env_files.expect_write().times(0);
        m.runtime.expect_build().returning(|_, _| Ok(()));
        m.runtime.expect_up().returning(|_, _| Ok(()));
        m.health
            .expect_check()
            .returning(|_, _, _| Err(DomainError::HealthCheck("timed out after 60s".into())));
        m.ingress.expect_upsert().times(0);

        let deploy = build(m);
        let sink = CollectSink::new();
        let permit = deploy.try_begin("rateme").unwrap();
        let err = deploy
            .execute(
                permit,
                "dep-hc".into(),
                sample_config(),
                DeployRef::Branch("main".into()),
                sink.clone(),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, DomainError::HealthCheck(_)));
        assert_eq!(
            *sink.finished.lock().unwrap(),
            vec![DeploymentStatus::Failed]
        );
    }

    #[tokio::test]
    async fn project_without_hostname_skips_ingress() {
        let mut m = mocks();
        ok_pre_stages(&mut m);
        m.secrets
            .expect_load()
            .returning(|_| Ok(EnvBundle::default()));
        m.env_files.expect_write().times(0);
        m.runtime.expect_build().returning(|_, _| Ok(()));
        m.runtime.expect_up().returning(|_, _| Ok(()));
        m.health.expect_check().returning(|_, _, _| Ok(()));
        m.ingress.expect_upsert().times(0);

        let mut config = sample_config();
        config.hostname = None;

        let deploy = build(m);
        let permit = deploy.try_begin("rateme").unwrap();
        let result = deploy
            .execute(
                permit,
                "dep-nh".into(),
                config,
                DeployRef::Branch("main".into()),
                CollectSink::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.status, DeploymentStatus::Success);
    }
}
