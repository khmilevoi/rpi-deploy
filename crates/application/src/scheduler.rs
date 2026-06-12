use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pi_domain::contracts::{Clock, DeploymentHistory, LogSink};
use pi_domain::entities::{DeployRef, Deployment, DeploymentStatus, ProjectConfig};
use pi_domain::error::DomainError;
use tokio_util::sync::CancellationToken;

use crate::deploy::DeployProject;

#[async_trait]
pub trait DeployRunner: Send + Sync {
    async fn run(
        &self,
        deployment_id: String,
        config: ProjectConfig,
        git_ref: DeployRef,
        sink: Arc<dyn LogSink>,
        cancel: CancellationToken,
    ) -> Result<(), DomainError>;
}

#[async_trait]
impl DeployRunner for DeployProject {
    async fn run(
        &self,
        deployment_id: String,
        config: ProjectConfig,
        git_ref: DeployRef,
        sink: Arc<dyn LogSink>,
        cancel: CancellationToken,
    ) -> Result<(), DomainError> {
        self.execute(deployment_id, config, git_ref, sink, cancel)
            .await
            .map(|_| ())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    Started,
    Queued,
    QueuedReplacing { superseded_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelOutcome {
    CanceledQueued,
    CancelRequested,
    NotActive,
}

struct Pending {
    id: String,
    config: ProjectConfig,
    git_ref: DeployRef,
    sink: Arc<dyn LogSink>,
    cancel: CancellationToken,
}

struct Running {
    id: String,
    cancel: CancellationToken,
}

#[derive(Default)]
struct Slot {
    running: Option<Running>,
    pending: Option<Pending>,
}

pub struct DeployScheduler {
    runner: Arc<dyn DeployRunner>,
    history: Arc<dyn DeploymentHistory>,
    clock: Arc<dyn Clock>,
    slots: Mutex<HashMap<String, Slot>>,
}

impl DeployScheduler {
    pub fn new(
        runner: Arc<dyn DeployRunner>,
        history: Arc<dyn DeploymentHistory>,
        clock: Arc<dyn Clock>,
    ) -> Arc<DeployScheduler> {
        Arc::new(DeployScheduler {
            runner,
            history,
            clock,
            slots: Mutex::new(HashMap::new()),
        })
    }

    pub async fn submit(
        self: &Arc<Self>,
        deployment_id: String,
        config: ProjectConfig,
        git_ref: DeployRef,
        sink: Arc<dyn LogSink>,
    ) -> Result<SubmitOutcome, DomainError> {
        let queued = Deployment {
            id: deployment_id.clone(),
            project: config.name.clone(),
            git_ref: git_ref.as_str().to_string(),
            commit_sha: None,
            status: DeploymentStatus::Queued,
            started_at: self.clock.now_unix(),
            finished_at: None,
            log_tail: String::new(),
        };
        self.history.record_queued(&queued).await?;

        let project = config.name.clone();
        let entry = Pending {
            id: deployment_id,
            config,
            git_ref,
            sink,
            cancel: CancellationToken::new(),
        };

        let (to_start, superseded) = {
            let mut slots = self
                .slots
                .lock()
                .map_err(|_| DomainError::Storage("scheduler lock poisoned".into()))?;
            let slot = slots.entry(project).or_default();
            if slot.running.is_none() {
                slot.running = Some(Running {
                    id: entry.id.clone(),
                    cancel: entry.cancel.clone(),
                });
                (Some(entry), None)
            } else {
                entry
                    .sink
                    .line("queued behind the active deploy of this project (latest wins)");
                (None, slot.pending.replace(entry))
            }
        };

        let outcome = match (&to_start, &superseded) {
            (Some(_), _) => SubmitOutcome::Started,
            (None, None) => SubmitOutcome::Queued,
            (None, Some(old)) => SubmitOutcome::QueuedReplacing {
                superseded_id: old.id.clone(),
            },
        };

        if let Some(old) = superseded {
            let now = self.clock.now_unix();
            let note = "superseded by a newer deploy request";
            old.sink.line(note);
            let record = self
                .history
                .record_finished(&old.id, DeploymentStatus::Superseded, None, now, note)
                .await;
            old.sink.finished(DeploymentStatus::Superseded);
            record?;
        }
        if let Some(first) = to_start {
            let scheduler = Arc::clone(self);
            tokio::spawn(async move { scheduler.run_project(first).await });
        }
        Ok(outcome)
    }

    async fn run_project(self: Arc<Self>, mut current: Pending) {
        loop {
            let project = current.config.name.clone();
            let Pending {
                id,
                config,
                git_ref,
                sink,
                cancel,
            } = current;
            let _ = self.runner.run(id, config, git_ref, sink, cancel).await;

            let next = {
                let Ok(mut slots) = self.slots.lock() else {
                    return;
                };
                let Some(slot) = slots.get_mut(&project) else {
                    return;
                };
                match slot.pending.take() {
                    Some(p) => {
                        slot.running = Some(Running {
                            id: p.id.clone(),
                            cancel: p.cancel.clone(),
                        });
                        Some(p)
                    }
                    None => {
                        slots.remove(&project);
                        None
                    }
                }
            };
            match next {
                Some(p) => current = p,
                None => return,
            }
        }
    }

    pub async fn cancel(&self, deployment_id: &str) -> Result<CancelOutcome, DomainError> {
        enum Found {
            Pending(Pending),
            Running,
            No,
        }
        let found = {
            let mut slots = self
                .slots
                .lock()
                .map_err(|_| DomainError::Storage("scheduler lock poisoned".into()))?;
            let mut found = Found::No;
            for slot in slots.values_mut() {
                if slot
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.id == deployment_id)
                {
                    if let Some(p) = slot.pending.take() {
                        found = Found::Pending(p);
                    }
                    break;
                }
                if let Some(running) = &slot.running {
                    if running.id == deployment_id {
                        running.cancel.cancel();
                        found = Found::Running;
                        break;
                    }
                }
            }
            found
        };
        match found {
            Found::Pending(p) => {
                let now = self.clock.now_unix();
                let note = "canceled while queued";
                p.sink.line(note);
                let record = self
                    .history
                    .record_finished(&p.id, DeploymentStatus::Canceled, None, now, note)
                    .await;
                p.sink.finished(DeploymentStatus::Canceled);
                record?;
                Ok(CancelOutcome::CanceledQueued)
            }
            Found::Running => Ok(CancelOutcome::CancelRequested),
            Found::No => Ok(CancelOutcome::NotActive),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CollectSink;
    use pi_domain::contracts::{MockClock, MockDeploymentHistory};
    use pi_domain::entities::{HealthcheckConfig, StageTimeoutOverrides};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn config(name: &str) -> ProjectConfig {
        ProjectConfig {
            name: name.into(),
            repo: "r".into(),
            branch: "main".into(),
            compose_path: "docker-compose.yml".into(),
            service: "web".into(),
            container_port: 3000,
            hostname: None,
            healthcheck: HealthcheckConfig::default(),
            timeouts: StageTimeoutOverrides::default(),
        }
    }

    struct FakeRunner {
        started: Mutex<Vec<String>>,
        gate: tokio::sync::Semaphore,
        finished_count: AtomicUsize,
    }

    impl FakeRunner {
        fn new() -> Arc<FakeRunner> {
            Arc::new(FakeRunner {
                started: Mutex::new(vec![]),
                gate: tokio::sync::Semaphore::new(0),
                finished_count: AtomicUsize::new(0),
            })
        }

        fn started_ids(&self) -> Vec<String> {
            self.started.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl DeployRunner for FakeRunner {
        async fn run(
            &self,
            deployment_id: String,
            _config: ProjectConfig,
            _git_ref: DeployRef,
            sink: Arc<dyn LogSink>,
            cancel: CancellationToken,
        ) -> Result<(), DomainError> {
            self.started.lock().unwrap().push(deployment_id);
            let result = tokio::select! {
                _ = cancel.cancelled() => {
                    sink.finished(DeploymentStatus::Canceled);
                    Err(DomainError::Canceled)
                }
                permit = self.gate.acquire() => {
                    permit.map_err(|_| DomainError::Runtime("gate closed".into())).map(|p| {
                        p.forget();
                        sink.finished(DeploymentStatus::Success);
                    })
                }
            };
            self.finished_count.fetch_add(1, Ordering::SeqCst);
            result
        }
    }

    fn history_ok() -> MockDeploymentHistory {
        let mut history = MockDeploymentHistory::new();
        history.expect_record_queued().returning(|_| Ok(()));
        history
            .expect_record_finished()
            .returning(|_, _, _, _, _| Ok(()));
        history
    }

    fn clock() -> MockClock {
        let mut clock = MockClock::new();
        clock.expect_now_unix().return_const(100i64);
        clock
    }

    async fn wait_until(deadline_what: &str, f: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while !f() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for: {deadline_what}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    fn scheduler_with(
        runner: &Arc<FakeRunner>,
        history: MockDeploymentHistory,
    ) -> Arc<DeployScheduler> {
        DeployScheduler::new(
            Arc::clone(runner) as Arc<dyn DeployRunner>,
            Arc::new(history),
            Arc::new(clock()),
        )
    }

    #[tokio::test]
    async fn idle_project_starts_immediately() {
        let runner = FakeRunner::new();
        let scheduler = scheduler_with(&runner, history_ok());
        let outcome = scheduler
            .submit(
                "d1".into(),
                config("a"),
                DeployRef::Branch("main".into()),
                CollectSink::new(),
            )
            .await
            .unwrap();
        assert_eq!(outcome, SubmitOutcome::Started);
        wait_until("d1 started", || runner.started_ids() == vec!["d1"]).await;
        runner.gate.add_permits(1);
        wait_until("d1 finished", || {
            runner.finished_count.load(Ordering::SeqCst) == 1
        })
        .await;
    }

    #[tokio::test]
    async fn second_submit_queues_and_runs_after_active_finishes() {
        let runner = FakeRunner::new();
        let scheduler = scheduler_with(&runner, history_ok());
        scheduler
            .submit(
                "d1".into(),
                config("a"),
                DeployRef::Branch("main".into()),
                CollectSink::new(),
            )
            .await
            .unwrap();
        wait_until("d1 started", || runner.started_ids() == vec!["d1"]).await;

        let outcome = scheduler
            .submit(
                "d2".into(),
                config("a"),
                DeployRef::Branch("main".into()),
                CollectSink::new(),
            )
            .await
            .unwrap();
        assert_eq!(outcome, SubmitOutcome::Queued);
        assert_eq!(runner.started_ids(), vec!["d1"], "d2 must wait");

        runner.gate.add_permits(2);
        wait_until("d2 ran after d1", || {
            runner.started_ids() == vec!["d1", "d2"]
                && runner.finished_count.load(Ordering::SeqCst) == 2
        })
        .await;
    }

    #[tokio::test]
    async fn third_submit_supersedes_the_pending_one() {
        let runner = FakeRunner::new();
        let mut history = MockDeploymentHistory::new();
        history.expect_record_queued().returning(|_| Ok(()));
        history
            .expect_record_finished()
            .withf(|id, status, _sha, _at, tail| {
                id == "d2"
                    && *status == DeploymentStatus::Superseded
                    && tail.contains("superseded")
            })
            .times(1)
            .returning(|_, _, _, _, _| Ok(()));
        let scheduler = scheduler_with(&runner, history);

        scheduler
            .submit(
                "d1".into(),
                config("a"),
                DeployRef::Branch("main".into()),
                CollectSink::new(),
            )
            .await
            .unwrap();
        wait_until("d1 started", || runner.started_ids() == vec!["d1"]).await;
        let d2_sink = CollectSink::new();
        scheduler
            .submit(
                "d2".into(),
                config("a"),
                DeployRef::Branch("main".into()),
                d2_sink.clone(),
            )
            .await
            .unwrap();

        let outcome = scheduler
            .submit(
                "d3".into(),
                config("a"),
                DeployRef::Branch("main".into()),
                CollectSink::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            outcome,
            SubmitOutcome::QueuedReplacing {
                superseded_id: "d2".into()
            }
        );
        assert_eq!(
            *d2_sink.finished.lock().unwrap(),
            vec![DeploymentStatus::Superseded]
        );

        runner.gate.add_permits(2);
        wait_until("d3 ran after d1, skipping d2", || {
            runner.started_ids() == vec!["d1", "d3"]
        })
        .await;
    }

    #[tokio::test]
    async fn cancel_queued_removes_it_from_the_slot() {
        let runner = FakeRunner::new();
        let mut history = MockDeploymentHistory::new();
        history.expect_record_queued().returning(|_| Ok(()));
        history
            .expect_record_finished()
            .withf(|id, status, _sha, _at, _tail| {
                id == "d2" && *status == DeploymentStatus::Canceled
            })
            .times(1)
            .returning(|_, _, _, _, _| Ok(()));
        let scheduler = scheduler_with(&runner, history);

        scheduler
            .submit(
                "d1".into(),
                config("a"),
                DeployRef::Branch("main".into()),
                CollectSink::new(),
            )
            .await
            .unwrap();
        wait_until("d1 started", || runner.started_ids() == vec!["d1"]).await;
        let d2_sink = CollectSink::new();
        scheduler
            .submit(
                "d2".into(),
                config("a"),
                DeployRef::Branch("main".into()),
                d2_sink.clone(),
            )
            .await
            .unwrap();

        let outcome = scheduler.cancel("d2").await.unwrap();
        assert_eq!(outcome, CancelOutcome::CanceledQueued);
        assert_eq!(
            *d2_sink.finished.lock().unwrap(),
            vec![DeploymentStatus::Canceled]
        );

        runner.gate.add_permits(1);
        wait_until("d1 finished alone", || {
            runner.finished_count.load(Ordering::SeqCst) == 1
        })
        .await;
        assert_eq!(runner.started_ids(), vec!["d1"], "d2 must never start");
    }

    #[tokio::test]
    async fn cancel_running_signals_token_and_promotes_pending() {
        let runner = FakeRunner::new();
        let scheduler = scheduler_with(&runner, history_ok());
        scheduler
            .submit(
                "d1".into(),
                config("a"),
                DeployRef::Branch("main".into()),
                CollectSink::new(),
            )
            .await
            .unwrap();
        wait_until("d1 started", || runner.started_ids() == vec!["d1"]).await;
        scheduler
            .submit(
                "d2".into(),
                config("a"),
                DeployRef::Branch("main".into()),
                CollectSink::new(),
            )
            .await
            .unwrap();

        let outcome = scheduler.cancel("d1").await.unwrap();
        assert_eq!(outcome, CancelOutcome::CancelRequested);
        wait_until("d2 promoted after d1 canceled", || {
            runner.started_ids() == vec!["d1", "d2"]
        })
        .await;
        runner.gate.add_permits(1);
        wait_until("d2 finished", || {
            runner.finished_count.load(Ordering::SeqCst) == 2
        })
        .await;
    }

    #[tokio::test]
    async fn cancel_unknown_id_is_not_active() {
        let runner = FakeRunner::new();
        let scheduler = scheduler_with(&runner, history_ok());
        assert_eq!(
            scheduler.cancel("ghost").await.unwrap(),
            CancelOutcome::NotActive
        );
    }

    #[tokio::test]
    async fn after_slot_drains_a_new_submit_starts_fresh() {
        let runner = FakeRunner::new();
        let scheduler = scheduler_with(&runner, history_ok());
        scheduler
            .submit(
                "d1".into(),
                config("a"),
                DeployRef::Branch("main".into()),
                CollectSink::new(),
            )
            .await
            .unwrap();
        runner.gate.add_permits(1);
        wait_until("d1 finished", || {
            runner.finished_count.load(Ordering::SeqCst) == 1
        })
        .await;

        let outcome = scheduler
            .submit(
                "d2".into(),
                config("a"),
                DeployRef::Branch("main".into()),
                CollectSink::new(),
            )
            .await
            .unwrap();
        assert_eq!(outcome, SubmitOutcome::Started, "slot must be drained");
        runner.gate.add_permits(1);
        wait_until("d2 finished", || {
            runner.finished_count.load(Ordering::SeqCst) == 2
        })
        .await;
    }
}
