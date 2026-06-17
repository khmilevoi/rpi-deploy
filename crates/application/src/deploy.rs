use std::sync::Arc;

use pi_domain::contracts::{
    Clock, ContainerRuntime, DeploymentHistory, EnvFileWriter, HealthGate, Ingress, LogSink,
    OverrideStore, ProjectRepository, SecretStore, Source,
};
use pi_domain::entities::{
    ComposeStack, DeployRef, Deployment, DeploymentStatus, ProjectConfig, StageTimeouts,
};
use pi_domain::error::DomainError;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::gc::RunGc;
use crate::mask::MaskingSink;
use crate::tail::TailSink;

const LOG_TAIL_LINES: usize = 400;
const GC_TIMEOUT_SECS: u64 = 300;

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

/// Deploy use-case (§7, §8.2). Per-project serialization is owned by DeployScheduler.
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
    gc: Arc<RunGc>,
    timeouts: StageTimeouts,
    /// §8.1: global build semaphore — parallel builds OOM the Pi.
    build_sem: Semaphore,
}

/// Wraps a deploy stage with its timeout (§8.1). On expiry the stage future is
/// dropped — kill_on_drop in the process adapter kills the child — and the
/// deploy fails as `timeout: <stage>`.
async fn staged<T>(
    stage: &str,
    secs: u64,
    fut: impl std::future::Future<Output = Result<T, DomainError>>,
) -> Result<T, DomainError> {
    match tokio::time::timeout(std::time::Duration::from_secs(secs), fut).await {
        Ok(result) => result,
        Err(_) => Err(DomainError::Timeout {
            stage: stage.to_string(),
            secs,
        }),
    }
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
        gc: Arc<RunGc>,
        timeouts: StageTimeouts,
        build_slots: usize,
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
            gc,
            timeouts,
            build_sem: Semaphore::new(build_slots),
        })
    }

    pub async fn execute(
        &self,
        deployment_id: String,
        config: ProjectConfig,
        git_ref: DeployRef,
        sink: Arc<dyn LogSink>,
        cancel: CancellationToken,
    ) -> Result<Deployment, DomainError> {
        // chain: stages write to masker → masks secrets → tail stores masked lines → sink (SSE hub)
        let tail = TailSink::new(Arc::clone(&sink), LOG_TAIL_LINES);
        let masker = MaskingSink::new(tail.clone());
        let log: Arc<dyn LogSink> = masker.clone();
        let mut guard = FinishGuard::new(sink);

        let started_at = self.clock.now_unix();
        if let Err(err) = self.history.mark_running(&deployment_id, started_at).await {
            // DB record stays queued without this; write a Failed terminal row so
            // the record is consistent without waiting for the next startup sweep.
            let _ = self
                .history
                .record_finished(
                    &deployment_id,
                    DeploymentStatus::Failed,
                    None,
                    started_at,
                    &err.to_string(),
                )
                .await;
            return Err(err);
        }
        let mut deployment = Deployment {
            id: deployment_id,
            project: config.name.clone(),
            git_ref: git_ref.as_str().to_string(),
            commit_sha: None,
            status: DeploymentStatus::Running,
            started_at,
            finished_at: None,
            log_tail: String::new(),
        };

        let result = tokio::select! {
            _ = cancel.cancelled() => Err(DomainError::Canceled),
            r = self.run_stages(&config, &git_ref, log.clone(), &masker) => r,
        };
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
                let status = if matches!(err, DomainError::Canceled) {
                    DeploymentStatus::Canceled
                } else {
                    DeploymentStatus::Failed
                };
                log.line(&format!("deploy {}: {err}", status.as_str()));
                let log_tail = tail.tail();
                let record_result = self
                    .history
                    .record_finished(&deployment.id, status, None, finished_at, &log_tail)
                    .await;
                log.finished(status);
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
        let timeouts = self.timeouts.with_overrides(&config.timeouts);

        let project = self.projects.upsert(config).await?;
        log.line(&format!(
            "project '{}': host port {}",
            project.config.name, project.host_port
        ));

        let fetched = staged(
            "fetch",
            timeouts.fetch_secs,
            self.source.fetch(config, git_ref, log.clone()),
        )
        .await?;
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
        {
            let _build_slot = self
                .build_sem
                .acquire()
                .await
                .map_err(|_| DomainError::Runtime("build semaphore closed".into()))?;
            staged(
                "build",
                timeouts.build_secs,
                self.runtime.build(&stack, log.clone()),
            )
            .await?;
        }
        staged("up", timeouts.up_secs, self.runtime.up(&stack, log.clone())).await?;

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

        if let Err(err) = staged("gc", GC_TIMEOUT_SECS, self.gc.execute(log.clone())).await {
            log.line(&format!("gc skipped: {err}"));
        }

        Ok(fetched.commit_sha)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CollectSink;
    use pi_domain::contracts::{
        MockClock, MockContainerRuntime, MockDeploymentHistory, MockDiskProbe, MockEnvFileWriter,
        MockHealthGate, MockIngress, MockOverrideStore, MockProjectRepository, MockSecretStore,
        MockSource,
    };
    use pi_domain::entities::{
        DeployRef, DeploymentStatus, EnvBundle, FetchedSource, HealthcheckConfig, Project,
        ProjectConfig, StageTimeoutOverrides, StageTimeouts,
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
        pub gc_runtime: MockContainerRuntime,
        pub disk: MockDiskProbe,
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
        let mut gc_runtime = MockContainerRuntime::new();
        gc_runtime.expect_prune_images().returning(|_| Ok(()));
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(10));
        Mocks {
            source: MockSource::new(),
            runtime: MockContainerRuntime::new(),
            gc_runtime,
            disk,
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
        let gc = RunGc::new(Arc::new(m.gc_runtime), Arc::new(m.disk), 85);
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
            gc,
            StageTimeouts::default(),
            1,
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
        m.gc_runtime.checkpoint();
        let stage_order = Arc::clone(&order);
        m.gc_runtime
            .expect_prune_images()
            .times(1)
            .returning(move |_| {
                stage_order.lock().unwrap().push("gc");
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
        let result = deploy
            .execute(
                "dep-1".into(),
                cfg,
                DeployRef::Branch("main".into()),
                sink.clone(),
                CancellationToken::new(),
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
                "running", "upsert", "fetch", "secrets", "override", "build", "up", "health",
                "ingress", "gc", "finished"
            ]
        );
    }

    #[tokio::test]
    async fn mark_running_failure_writes_failed_to_db_and_emits_finished() {
        let mut m = mocks();
        m.history
            .expect_mark_running()
            .returning(|_, _| Err(DomainError::Storage("db locked".into())));
        m.history
            .expect_record_finished()
            .withf(|id, status, _sha, _at, _tail| {
                id == "dep-mr" && *status == DeploymentStatus::Failed
            })
            .times(1)
            .returning(|_, _, _, _, _| Ok(()));

        let deploy = build(m);
        let sink = CollectSink::new();
        let err = deploy
            .execute(
                "dep-mr".into(),
                sample_config(),
                DeployRef::Branch("main".into()),
                sink.clone(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, DomainError::Storage(_)));
        assert_eq!(
            *sink.finished.lock().unwrap(),
            vec![DeploymentStatus::Failed]
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
        let err = deploy
            .execute(
                "dep-2".into(),
                sample_config(),
                DeployRef::Branch("main".into()),
                sink.clone(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, DomainError::Runtime(_)));
        assert_eq!(
            *sink.finished.lock().unwrap(),
            vec![DeploymentStatus::Failed]
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
        m.history.expect_mark_running().returning(|_, _| Ok(()));
        m.history
            .expect_record_finished()
            .returning(|_, _, _, _, _| Ok(()));
    }

    struct HangingSource;

    #[async_trait::async_trait]
    impl pi_domain::contracts::Source for HangingSource {
        fn workdir(&self, project_name: &str) -> PathBuf {
            PathBuf::from("/wd").join(project_name)
        }

        async fn fetch(
            &self,
            _p: &ProjectConfig,
            _r: &DeployRef,
            _l: Arc<dyn LogSink>,
        ) -> Result<FetchedSource, DomainError> {
            std::future::pending().await
        }

        async fn cleanup(&self, _project_name: &str) -> Result<(), DomainError> {
            Ok(())
        }
    }

    fn build_with_source(
        m: Mocks,
        source: Arc<dyn pi_domain::contracts::Source>,
        timeouts: StageTimeouts,
    ) -> Arc<DeployProject> {
        let gc = RunGc::new(Arc::new(m.gc_runtime), Arc::new(m.disk), 85);
        DeployProject::new(
            source,
            Arc::new(m.runtime),
            Arc::new(m.projects),
            Arc::new(m.history),
            Arc::new(m.overrides),
            Arc::new(m.secrets),
            Arc::new(m.env_files),
            Arc::new(m.health),
            Arc::new(m.ingress),
            Arc::new(m.clock),
            gc,
            timeouts,
            1,
        )
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
        let result = deploy
            .execute(
                "dep-env".into(),
                sample_config(),
                DeployRef::Branch("main".into()),
                sink.clone(),
                CancellationToken::new(),
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
        let err = deploy
            .execute(
                "dep-hc".into(),
                sample_config(),
                DeployRef::Branch("main".into()),
                sink.clone(),
                CancellationToken::new(),
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
        let result = deploy
            .execute(
                "dep-nh".into(),
                config,
                DeployRef::Branch("main".into()),
                CollectSink::new(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.status, DeploymentStatus::Success);
    }

    #[tokio::test]
    async fn expired_fetch_stage_fails_with_timeout_and_stage_name() {
        let mut m = mocks();
        m.projects.expect_upsert().returning(|c| {
            Ok(Project {
                config: c.clone(),
                host_port: 8000,
                created_at: 1,
            })
        });
        m.history.expect_mark_running().returning(|_, _| Ok(()));
        m.history
            .expect_record_finished()
            .withf(|id, status, _sha, _at, tail| {
                id == "dep-t"
                    && *status == DeploymentStatus::Failed
                    && tail.contains("timeout: fetch")
            })
            .times(1)
            .returning(|_, _, _, _, _| Ok(()));

        let timeouts = StageTimeouts {
            fetch_secs: 0,
            ..StageTimeouts::default()
        };
        let deploy = build_with_source(m, Arc::new(HangingSource), timeouts);
        let sink = CollectSink::new();
        let err = deploy
            .execute(
                "dep-t".into(),
                sample_config(),
                DeployRef::Branch("main".into()),
                sink.clone(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();

        assert!(
            matches!(&err, DomainError::Timeout { stage, .. } if stage == "fetch"),
            "got: {err}"
        );
        assert_eq!(
            *sink.finished.lock().unwrap(),
            vec![DeploymentStatus::Failed]
        );
    }

    #[tokio::test]
    async fn cancel_token_marks_deployment_canceled() {
        let mut m = mocks();
        m.projects.expect_upsert().returning(|c| {
            Ok(Project {
                config: c.clone(),
                host_port: 8000,
                created_at: 1,
            })
        });
        m.history.expect_mark_running().returning(|_, _| Ok(()));
        m.history
            .expect_record_finished()
            .withf(|id, status, _sha, _at, _tail| {
                id == "dep-c" && *status == DeploymentStatus::Canceled
            })
            .times(1)
            .returning(|_, _, _, _, _| Ok(()));

        let deploy = build_with_source(m, Arc::new(HangingSource), StageTimeouts::default());
        let sink = CollectSink::new();
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let deploy = Arc::clone(&deploy);
            let sink = sink.clone();
            let cancel = cancel.clone();
            async move {
                deploy
                    .execute(
                        "dep-c".into(),
                        sample_config(),
                        DeployRef::Branch("main".into()),
                        sink,
                        cancel,
                    )
                    .await
            }
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel.cancel();
        let err = task.await.unwrap().unwrap_err();

        assert!(matches!(err, DomainError::Canceled), "got: {err}");
        assert_eq!(
            *sink.finished.lock().unwrap(),
            vec![DeploymentStatus::Canceled]
        );
    }

    struct CountingRuntime {
        active: std::sync::atomic::AtomicUsize,
        max_seen: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl pi_domain::contracts::ContainerRuntime for CountingRuntime {
        async fn build(
            &self,
            _stack: &pi_domain::entities::ComposeStack,
            _log: Arc<dyn LogSink>,
        ) -> Result<(), DomainError> {
            use std::sync::atomic::Ordering::SeqCst;
            let n = self.active.fetch_add(1, SeqCst) + 1;
            self.max_seen.fetch_max(n, SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            self.active.fetch_sub(1, SeqCst);
            Ok(())
        }

        async fn up(
            &self,
            _stack: &pi_domain::entities::ComposeStack,
            _log: Arc<dyn LogSink>,
        ) -> Result<(), DomainError> {
            Ok(())
        }

        async fn ps(
            &self,
            _project_name: &str,
        ) -> Result<Vec<pi_domain::entities::ServiceState>, DomainError> {
            Ok(vec![])
        }

        async fn prune_images(&self, _log: Arc<dyn LogSink>) -> Result<(), DomainError> {
            Ok(())
        }

        async fn prune_builder(&self, _log: Arc<dyn LogSink>) -> Result<(), DomainError> {
            Ok(())
        }

        async fn logs(
            &self,
            _project_name: &str,
            _tail: usize,
            _follow: bool,
            _log: Arc<dyn LogSink>,
        ) -> Result<(), DomainError> {
            Ok(())
        }

        async fn stats(
            &self,
            _project_name: &str,
        ) -> Result<Vec<pi_domain::entities::ServiceStats>, DomainError> {
            Ok(vec![])
        }

        async fn lifecycle(
            &self,
            _stack: &pi_domain::entities::ComposeStack,
            _action: pi_domain::entities::LifecycleAction,
            _log: Arc<dyn LogSink>,
        ) -> Result<(), DomainError> {
            Ok(())
        }

        async fn down(
            &self,
            _stack: &pi_domain::entities::ComposeStack,
            _remove_volumes: bool,
            _log: Arc<dyn LogSink>,
        ) -> Result<(), DomainError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn builds_of_different_projects_are_serialized_by_global_semaphore() {
        let mut m = mocks();
        m.projects.expect_upsert().returning(|c| {
            Ok(Project {
                config: c.clone(),
                host_port: if c.name == "a" { 8000 } else { 8001 },
                created_at: 1,
            })
        });
        m.source.expect_fetch().returning(|p, _, _| {
            Ok(FetchedSource {
                workdir: PathBuf::from("/wd").join(&p.name),
                commit_sha: SHA.into(),
            })
        });
        m.secrets
            .expect_load()
            .returning(|_| Ok(EnvBundle::default()));
        m.env_files.expect_write().times(0);
        m.overrides
            .expect_write()
            .returning(|p, _, _, _| Ok(PathBuf::from("/ov").join(p)));
        m.health.expect_check().returning(|_, _, _| Ok(()));
        m.ingress.expect_upsert().returning(|_, _, _| Ok(()));
        m.history.expect_mark_running().returning(|_, _| Ok(()));
        m.history
            .expect_record_finished()
            .returning(|_, _, _, _, _| Ok(()));

        let runtime = Arc::new(CountingRuntime {
            active: std::sync::atomic::AtomicUsize::new(0),
            max_seen: std::sync::atomic::AtomicUsize::new(0),
        });
        let gc = RunGc::new(Arc::clone(&runtime) as _, Arc::new(m.disk), 85);
        let deploy = DeployProject::new(
            Arc::new(m.source),
            Arc::clone(&runtime) as _,
            Arc::new(m.projects),
            Arc::new(m.history),
            Arc::new(m.overrides),
            Arc::new(m.secrets),
            Arc::new(m.env_files),
            Arc::new(m.health),
            Arc::new(m.ingress),
            Arc::new(m.clock),
            gc,
            StageTimeouts::default(),
            1,
        );

        let mut config_a = sample_config();
        config_a.name = "a".into();
        config_a.hostname = None;
        let mut config_b = sample_config();
        config_b.name = "b".into();
        config_b.hostname = None;

        let (ra, rb) = tokio::join!(
            deploy.execute(
                "dep-a".into(),
                config_a,
                DeployRef::Branch("main".into()),
                CollectSink::new(),
                CancellationToken::new(),
            ),
            deploy.execute(
                "dep-b".into(),
                config_b,
                DeployRef::Branch("main".into()),
                CollectSink::new(),
                CancellationToken::new(),
            ),
        );
        ra.unwrap();
        rb.unwrap();
        assert_eq!(
            runtime.max_seen.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "two builds must never run concurrently with a semaphore of 1"
        );
    }

    #[tokio::test]
    async fn gc_failure_does_not_fail_the_deploy() {
        let mut m = mocks();
        ok_pre_stages(&mut m);
        m.secrets
            .expect_load()
            .returning(|_| Ok(EnvBundle::default()));
        m.env_files.expect_write().times(0);
        m.runtime.expect_build().returning(|_, _| Ok(()));
        m.runtime.expect_up().returning(|_, _| Ok(()));
        m.health.expect_check().returning(|_, _, _| Ok(()));
        m.ingress.expect_upsert().returning(|_, _, _| Ok(()));
        m.gc_runtime.checkpoint();
        m.gc_runtime
            .expect_prune_images()
            .returning(|_| Err(DomainError::Runtime("docker daemon hiccup".into())));

        let deploy = build(m);
        let result = deploy
            .execute(
                "dep-gc".into(),
                sample_config(),
                DeployRef::Branch("main".into()),
                CollectSink::new(),
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.status, DeploymentStatus::Success);
        assert!(
            result.log_tail.contains("gc skipped"),
            "tail: {}",
            result.log_tail
        );
    }
}
