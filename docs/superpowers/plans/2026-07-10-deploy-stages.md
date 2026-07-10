# Deploy Pipeline View (Stage SSE Events) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The agent announces each deploy stage (`fetch`, `build`, `start`, `health`, `route`, `gc`) over SSE; the CLI renders each stage as a collapsing log pane with per-stage timing, and the final stamp gains a service count.

**Architecture:** `LogSink` (the deploy log chain `run_stages → MaskingSink → TailSink → HubSink → SSE`) gains two methods, `stage()` and `summary()`, so stage events ride the same ordered channel as log lines. The hub backlog stores events instead of lines, so reconnects/queued deploys replay pane structure. The CLI's `follow_logs` surfaces typed events to a new `output::Pipeline` orchestrator built on the existing `LogPane`. Everything is additive: `finished` stays a bare string, old CLIs ignore unknown SSE events, new CLIs fall back to today's single pane when no stage events arrive.

**Tech Stack:** Rust workspace (crates `pi-domain`, `pi-application`, `pi-infrastructure`, `pi` = the `rpi` bin), axum SSE, serde_json, console/LogPane rendering, mockall in tests.

**Spec:** `docs/superpowers/specs/2026-07-10-deploy-stages-design.md`

## Global Constraints

- Before finishing any task: `rtk cargo fmt --all -- --check`, `rtk cargo clippy --all-targets --locked -- -D warnings`, `rtk cargo test --locked` must pass (project CLAUDE.md; final task runs all three, per-task steps run the targeted tests).
- Always prefix commands with `rtk`.
- Wire stage names are exactly: `fetch`, `build`, `start`, `health`, `route`, `gc`. Statuses: `started`, `ok`, `failed`, `skipped`. `elapsed_ms` present on completions, absent on `started`.
- `event: finished` payload must remain a bare status string (old-CLI compatibility). No API version bump.
- Config/DTO field names (`up_secs` in rpi.toml / `TimeoutsDto`) must NOT change; only the displayed stage name changes `up` → `start`.
- Working directory: the `worktree-deploy-stages` worktree at `C:\Users\Khmil\RustProjects\pi\.claude\worktrees\deploy-stages`.

---

### Task 1: Domain — `StageEvent`, `StageStatus`, `LogSink::stage`/`summary`

**Files:**
- Modify: `crates/domain/src/entities.rs` (add after `DeploymentStatus`)
- Modify: `crates/domain/src/contracts.rs:17-20` (the `LogSink` trait)

**Interfaces:**
- Produces (later tasks rely on these exact items):
  - `pi_domain::entities::StageStatus { Started, Ok, Failed, Skipped }` with `pub fn as_str(&self) -> &'static str` returning `"started" | "ok" | "failed" | "skipped"`.
  - `pi_domain::entities::StageEvent { pub stage: String, pub status: StageStatus, pub elapsed_ms: Option<u64> }` with constructors `started(stage: &str)`, `ok(stage: &str, elapsed: std::time::Duration)`, `failed(stage: &str, elapsed: std::time::Duration)`, `skipped(stage: &str, elapsed: std::time::Duration)`.
  - `LogSink` methods `fn stage(&self, _ev: &StageEvent) {}` and `fn summary(&self, _services: usize) {}` — default no-ops, so the 9 existing implementors keep compiling; only sinks that care override.

- [ ] **Step 1: Write the failing test** — append to the `tests` module in `crates/domain/src/entities.rs`:

```rust
#[test]
fn stage_event_constructors_carry_status_and_elapsed_ms() {
    use std::time::Duration;
    let s = StageEvent::started("fetch");
    assert_eq!(s.stage, "fetch");
    assert_eq!(s.status, StageStatus::Started);
    assert_eq!(s.elapsed_ms, None);

    let ok = StageEvent::ok("build", Duration::from_millis(48_231));
    assert_eq!(ok.status, StageStatus::Ok);
    assert_eq!(ok.elapsed_ms, Some(48_231));

    assert_eq!(
        StageEvent::failed("start", Duration::from_secs(2)).status,
        StageStatus::Failed
    );
    assert_eq!(
        StageEvent::skipped("gc", Duration::from_millis(400)).status,
        StageStatus::Skipped
    );

    assert_eq!(StageStatus::Started.as_str(), "started");
    assert_eq!(StageStatus::Ok.as_str(), "ok");
    assert_eq!(StageStatus::Failed.as_str(), "failed");
    assert_eq!(StageStatus::Skipped.as_str(), "skipped");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `rtk cargo test --locked -p pi-domain stage_event`
Expected: compile error — `StageEvent` not found.

- [ ] **Step 3: Implement** — in `crates/domain/src/entities.rs`, right after the `DeploymentStatus` impl block:

```rust
/// Per-stage progress marker for the deploy pipeline view (deploy-stages
/// spec). Travels through `LogSink` alongside log lines so ordering with the
/// surrounding output is guaranteed by the channel itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageStatus {
    Started,
    Ok,
    Failed,
    Skipped,
}

impl StageStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            StageStatus::Started => "started",
            StageStatus::Ok => "ok",
            StageStatus::Failed => "failed",
            StageStatus::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageEvent {
    pub stage: String,
    pub status: StageStatus,
    /// Present on completions (`ok`/`failed`/`skipped`), absent on `started`.
    pub elapsed_ms: Option<u64>,
}

impl StageEvent {
    pub fn started(stage: &str) -> StageEvent {
        StageEvent {
            stage: stage.to_string(),
            status: StageStatus::Started,
            elapsed_ms: None,
        }
    }

    fn done(stage: &str, status: StageStatus, elapsed: std::time::Duration) -> StageEvent {
        StageEvent {
            stage: stage.to_string(),
            status,
            elapsed_ms: Some(elapsed.as_millis() as u64),
        }
    }

    pub fn ok(stage: &str, elapsed: std::time::Duration) -> StageEvent {
        StageEvent::done(stage, StageStatus::Ok, elapsed)
    }

    pub fn failed(stage: &str, elapsed: std::time::Duration) -> StageEvent {
        StageEvent::done(stage, StageStatus::Failed, elapsed)
    }

    pub fn skipped(stage: &str, elapsed: std::time::Duration) -> StageEvent {
        StageEvent::done(stage, StageStatus::Skipped, elapsed)
    }
}
```

In `crates/domain/src/contracts.rs`, extend the trait (add `StageEvent` to the existing `crate::entities::...` import list at the top of the file):

```rust
pub trait LogSink: Send + Sync {
    fn line(&self, line: &str);
    fn finished(&self, status: DeploymentStatus);
    /// Deploy pipeline stage marker (deploy-stages spec). Default no-op: only
    /// the deploy chain (masker → tail → hub) forwards these.
    fn stage(&self, _ev: &StageEvent) {}
    /// Service count after the health gate, for the CLI's result stamp.
    fn summary(&self, _services: usize) {}
}
```

- [ ] **Step 4: Run tests**

Run: `rtk cargo test --locked -p pi-domain`
Expected: PASS (all).

- [ ] **Step 5: Commit**

```bash
rtk git add crates/domain/src/entities.rs crates/domain/src/contracts.rs
rtk git commit -m "feat(domain): StageEvent + LogSink stage/summary methods"
```

---

### Task 2: Application sinks — forward stage/summary, tail boundary lines

**Files:**
- Modify: `crates/application/src/test_support.rs` (CollectSink)
- Modify: `crates/application/src/mask.rs:55-63` (MaskingSink LogSink impl)
- Modify: `crates/application/src/tail.rs` (TailSink)

**Interfaces:**
- Consumes: `StageEvent`, `StageStatus`, `LogSink::stage/summary` from Task 1.
- Produces:
  - `CollectSink { pub stages: Mutex<Vec<StageEvent>>, pub summaries: Mutex<Vec<usize>> }` — Task 3's tests assert on these fields.
  - `TailSink::stage` records a boundary line `▸ build ok (48.3s)` into the tail on completions (nothing on `started`), then forwards. `MaskingSink` forwards both methods verbatim.

- [ ] **Step 1: Write the failing tests** — append to `crates/application/src/tail.rs` tests:

```rust
#[test]
fn stage_completions_record_boundary_lines_and_forward() {
    use pi_domain::entities::StageEvent;
    use std::time::Duration;
    let inner = CollectSink::new();
    let tail = TailSink::new(inner.clone(), 10);

    tail.stage(&StageEvent::started("build")); // no boundary line for starts
    tail.line("step 1/9");
    tail.stage(&StageEvent::ok("build", Duration::from_millis(48_300)));
    tail.stage(&StageEvent::skipped("gc", Duration::from_millis(400)));

    assert_eq!(tail.tail(), "step 1/9\n▸ build ok (48.3s)\n▸ gc skipped (0.4s)");
    assert_eq!(inner.stages.lock().unwrap().len(), 3, "all events forwarded");
}

#[test]
fn summary_is_forwarded_without_a_tail_line() {
    let inner = CollectSink::new();
    let tail = TailSink::new(inner.clone(), 10);
    tail.summary(2);
    assert_eq!(tail.tail(), "");
    assert_eq!(*inner.summaries.lock().unwrap(), vec![2]);
}
```

And to `crates/application/src/mask.rs` tests:

```rust
#[test]
fn stage_and_summary_pass_through_unmasked() {
    use pi_domain::entities::{StageEvent, StageStatus};
    let inner = CollectSink::new();
    let sink = MaskingSink::new(inner.clone());
    sink.stage(&StageEvent::started("fetch"));
    sink.summary(3);
    let stages = inner.stages.lock().unwrap();
    assert_eq!(stages[0].stage, "fetch");
    assert_eq!(stages[0].status, StageStatus::Started);
    assert_eq!(*inner.summaries.lock().unwrap(), vec![3]);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test --locked -p pi-application stage`
Expected: compile error — `CollectSink` has no field `stages`.

- [ ] **Step 3: Implement.** `crates/application/src/test_support.rs` becomes:

```rust
use std::sync::{Arc, Mutex};

use pi_domain::contracts::LogSink;
use pi_domain::entities::{DeploymentStatus, StageEvent};

/// Test sink: collects everything written to it.
pub struct CollectSink {
    pub lines: Mutex<Vec<String>>,
    pub finished: Mutex<Vec<DeploymentStatus>>,
    pub stages: Mutex<Vec<StageEvent>>,
    pub summaries: Mutex<Vec<usize>>,
}

impl CollectSink {
    pub fn new() -> Arc<CollectSink> {
        Arc::new(CollectSink {
            lines: Mutex::new(vec![]),
            finished: Mutex::new(vec![]),
            stages: Mutex::new(vec![]),
            summaries: Mutex::new(vec![]),
        })
    }
}

impl LogSink for CollectSink {
    fn line(&self, line: &str) {
        self.lines.lock().unwrap().push(line.to_string());
    }

    fn finished(&self, status: DeploymentStatus) {
        self.finished.lock().unwrap().push(status);
    }

    fn stage(&self, ev: &StageEvent) {
        self.stages.lock().unwrap().push(ev.clone());
    }

    fn summary(&self, services: usize) {
        self.summaries.lock().unwrap().push(services);
    }
}
```

In `crates/application/src/mask.rs`, add to the `impl LogSink for MaskingSink` block (import `StageEvent` alongside the existing entity imports):

```rust
    fn stage(&self, ev: &StageEvent) {
        self.inner.stage(ev);
    }

    fn summary(&self, services: usize) {
        self.inner.summary(services);
    }
```

In `crates/application/src/tail.rs`: factor the buffer push out of `line()` and add the two methods (import `StageEvent` and `StageStatus`):

```rust
impl TailSink {
    // ... existing new()/tail() unchanged ...

    fn record(&self, line: &str) {
        if let Ok(mut lines) = self.lines.lock() {
            if self.cap > 0 {
                if lines.len() == self.cap {
                    lines.pop_front();
                }
                lines.push_back(line.to_string());
            }
        }
    }
}

impl LogSink for TailSink {
    fn line(&self, line: &str) {
        self.record(line);
        self.inner.line(line);
    }

    fn finished(&self, status: DeploymentStatus) {
        self.inner.finished(status);
    }

    fn stage(&self, ev: &StageEvent) {
        // Boundary line keeps the DB log_tail readable; `crate::duration`
        // lives in the bin crate, so plain seconds formatting here.
        if ev.status != StageStatus::Started {
            let elapsed = ev
                .elapsed_ms
                .map(|ms| format!(" ({:.1}s)", ms as f64 / 1000.0))
                .unwrap_or_default();
            self.record(&format!("▸ {} {}{elapsed}", ev.stage, ev.status.as_str()));
        }
        self.inner.stage(ev);
    }

    fn summary(&self, services: usize) {
        self.inner.summary(services);
    }
}
```

- [ ] **Step 4: Run tests**

Run: `rtk cargo test --locked -p pi-application`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/application/src/test_support.rs crates/application/src/mask.rs crates/application/src/tail.rs
rtk git commit -m "feat(application): forward stage/summary through sink chain, tail boundary lines"
```

---

### Task 3: `run_stages` emits stage events, `up`→`start` rename, summary after health

**Files:**
- Modify: `crates/application/src/deploy.rs` (run_stages + tests)

**Interfaces:**
- Consumes: `StageEvent` constructors (Task 1), `CollectSink.stages/summaries` (Task 2).
- Produces: the exact wire event sequence the CLI relies on. Happy path with hostname:
  `fetch started/ok, build started/ok, start started/ok, health started/ok, [summary], route started/ok, gc started/ok`. Failures: the failing stage emits `failed` (with `elapsed_ms`); gc failure emits `skipped`; `IngressOutcome::Skipped` emits `route skipped`. Cancellation emits nothing further (future dropped).

- [ ] **Step 1: Write the failing tests** — in `crates/application/src/deploy.rs` tests module. First extend the existing happy-path test `happy_path_runs_all_stages_and_records_success`: add before `let deploy = build(m);`:

```rust
        m.runtime.expect_ps().times(1).returning(|_| {
            Ok(vec![
                pi_domain::entities::ServiceState {
                    service: "web".into(),
                    state: "running".into(),
                    health: None,
                },
                pi_domain::entities::ServiceState {
                    service: "worker".into(),
                    state: "running".into(),
                    health: None,
                },
            ])
        });
```

and after the existing `order` assertion:

```rust
        use pi_domain::entities::StageStatus::{Ok as SOk, Started};
        let stages: Vec<(String, pi_domain::entities::StageStatus)> = sink
            .stages
            .lock()
            .unwrap()
            .iter()
            .map(|e| (e.stage.clone(), e.status))
            .collect();
        assert_eq!(
            stages,
            vec![
                ("fetch".into(), Started),
                ("fetch".into(), SOk),
                ("build".into(), Started),
                ("build".into(), SOk),
                ("start".into(), Started),
                ("start".into(), SOk),
                ("health".into(), Started),
                ("health".into(), SOk),
                ("route".into(), Started),
                ("route".into(), SOk),
                ("gc".into(), Started),
                ("gc".into(), SOk),
            ]
        );
        assert!(
            sink.stages
                .lock()
                .unwrap()
                .iter()
                .all(|e| (e.status == pi_domain::entities::StageStatus::Started)
                    == e.elapsed_ms.is_none()),
            "elapsed_ms present exactly on completions"
        );
        assert_eq!(*sink.summaries.lock().unwrap(), vec![2]);
```

Then add new tests (complete code; `ok_pre_stages`, `mocks`, `build`, `sample_config`, `SHA`, `CollectSink` already exist in this tests module):

```rust
    #[tokio::test]
    async fn build_failure_emits_build_failed_stage_event() {
        let mut m = mocks();
        ok_pre_stages(&mut m);
        m.secrets
            .expect_load()
            .returning(|_| Ok(SecretsBundle::default()));
        m.runtime
            .expect_build()
            .returning(|_, _| Err(DomainError::Runtime("compose build exited with 1".into())));

        let deploy = build(m);
        let sink = CollectSink::new();
        deploy
            .execute(
                "dep-bf".into(),
                sample_config(),
                DeployRef::Branch("main".into()),
                sink.clone(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();

        let stages = sink.stages.lock().unwrap();
        let last = stages.last().unwrap();
        assert_eq!(last.stage, "build");
        assert_eq!(last.status, pi_domain::entities::StageStatus::Failed);
        assert!(last.elapsed_ms.is_some());
    }

    #[tokio::test]
    async fn skipped_ingress_emits_route_skipped_event() {
        let mut m = mocks();
        ok_pre_stages(&mut m);
        m.secrets
            .expect_load()
            .returning(|_| Ok(SecretsBundle::default()));
        m.runtime.expect_build().returning(|_, _| Ok(()));
        m.runtime.expect_up().returning(|_, _| Ok(()));
        m.runtime.expect_ps().returning(|_| Ok(vec![]));
        m.health.expect_check().returning(|_, _, _| Ok(()));
        m.ingress
            .expect_upsert()
            .returning(|_, _, _| Ok(IngressOutcome::Skipped));

        let deploy = build(m);
        let sink = CollectSink::new();
        deploy
            .execute(
                "dep-rs".into(),
                sample_config(), // hostname = rateme.isskelo.com
                DeployRef::Branch("main".into()),
                sink.clone(),
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(sink
            .stages
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.stage == "route"
                && e.status == pi_domain::entities::StageStatus::Skipped));
    }

    #[tokio::test]
    async fn gc_failure_emits_gc_skipped_event() {
        let mut m = mocks();
        ok_pre_stages(&mut m);
        m.secrets
            .expect_load()
            .returning(|_| Ok(SecretsBundle::default()));
        m.runtime.expect_build().returning(|_, _| Ok(()));
        m.runtime.expect_up().returning(|_, _| Ok(()));
        m.runtime.expect_ps().returning(|_| Ok(vec![]));
        m.health.expect_check().returning(|_, _, _| Ok(()));
        m.ingress
            .expect_upsert()
            .returning(|_, _, _| Ok(IngressOutcome::Applied));
        m.gc_runtime.checkpoint();
        m.gc_runtime
            .expect_prune_images()
            .returning(|_| Err(DomainError::Runtime("docker daemon hiccup".into())));

        let deploy = build(m);
        let sink = CollectSink::new();
        let result = deploy
            .execute(
                "dep-gs".into(),
                sample_config(),
                DeployRef::Branch("main".into()),
                sink.clone(),
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.status, DeploymentStatus::Success);
        assert!(sink
            .stages
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.stage == "gc"
                && e.status == pi_domain::entities::StageStatus::Skipped));
    }

    #[tokio::test]
    async fn project_without_hostname_emits_no_route_stage() {
        let mut m = mocks();
        ok_pre_stages(&mut m);
        m.secrets
            .expect_load()
            .returning(|_| Ok(SecretsBundle::default()));
        m.runtime.expect_build().returning(|_, _| Ok(()));
        m.runtime.expect_up().returning(|_, _| Ok(()));
        m.runtime.expect_ps().returning(|_| Ok(vec![]));
        m.health.expect_check().returning(|_, _, _| Ok(()));
        m.ingress.expect_upsert().times(0);

        let mut config = sample_config();
        config.hostname = None;

        let deploy = build(m);
        let sink = CollectSink::new();
        deploy
            .execute(
                "dep-nr".into(),
                config,
                DeployRef::Branch("main".into()),
                sink.clone(),
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(!sink.stages.lock().unwrap().iter().any(|e| e.stage == "route"));
    }
```

(Note: `build_failure_emits_build_failed_stage_event` needs no `expect_ps` — the deploy never reaches the health gate. `secrets_writer.expect_write` is not set because the bundle is empty and the writer is never called.)

Also extend `expired_fetch_stage_fails_with_timeout_and_stage_name` with:

```rust
        let stages = sink.stages.lock().unwrap();
        assert_eq!(stages.last().unwrap().stage, "fetch");
        assert_eq!(
            stages.last().unwrap().status,
            pi_domain::entities::StageStatus::Failed
        );
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test --locked -p pi-application deploy`
Expected: new assertions FAIL (`stages` empty) — and several tests panic with mockall "expect_ps: no expectation" once the implementation lands; that comes next.

- [ ] **Step 3: Implement.** In `crates/application/src/deploy.rs` (import `StageEvent` in the entities use-list):

Add below `staged`:

```rust
/// Emits `started` / `ok` / `failed` stage events around a deploy stage,
/// measuring wall-clock elapsed. The stage name is the wire name the CLI
/// renders (deploy-stages spec).
async fn tracked<T>(
    log: &Arc<dyn LogSink>,
    stage: &str,
    fut: impl std::future::Future<Output = Result<T, DomainError>>,
) -> Result<T, DomainError> {
    log.stage(&StageEvent::started(stage));
    let t0 = std::time::Instant::now();
    match fut.await {
        Ok(v) => {
            log.stage(&StageEvent::ok(stage, t0.elapsed()));
            Ok(v)
        }
        Err(e) => {
            log.stage(&StageEvent::failed(stage, t0.elapsed()));
            Err(e)
        }
    }
}
```

In `run_stages`, replace the stage invocations:

```rust
        let fetched = tracked(
            &log,
            "fetch",
            staged(
                "fetch",
                timeouts.fetch_secs,
                self.source.fetch(config, git_ref, log.clone()),
            ),
        )
        .await?;
```

Build (inside the existing semaphore block):

```rust
            tracked(
                &log,
                "build",
                staged(
                    "build",
                    timeouts.build_secs,
                    self.runtime.build(&stack, log.clone()),
                ),
            )
            .await?;
```

Up → start (note the renamed timeout label):

```rust
        tracked(
            &log,
            "start",
            staged("start", timeouts.up_secs, self.runtime.up(&stack, log.clone())),
        )
        .await?;
```

Health, then summary:

```rust
        // §8: health gate — on failure the deploy is failed, stack stays up
        tracked(
            &log,
            "health",
            self.health.check(config, project.host_port, log.clone()),
        )
        .await?;

        // deploy-stages spec: service count for the CLI stamp; ps failure is
        // non-fatal — the summary is simply not sent.
        if let Ok(services) = self.runtime.ps(&config.name).await {
            log.summary(services.len());
        }
```

Route (replaces the `if let Some(hostname)` block; the warning text is unchanged):

```rust
        // §11: route hostname only when configured
        let mut ingress_warning: Option<String> = None;
        if let Some(hostname) = &config.hostname {
            log.stage(&StageEvent::started("route"));
            let t0 = std::time::Instant::now();
            match self
                .ingress
                .upsert(hostname, project.host_port, log.clone())
                .await
            {
                Ok(IngressOutcome::Applied) => {
                    log.stage(&StageEvent::ok("route", t0.elapsed()));
                }
                Ok(IngressOutcome::Skipped) => {
                    log.stage(&StageEvent::skipped("route", t0.elapsed()));
                    ingress_warning = Some(format!(
                        "warning: hostname {hostname} is declared but ingress is disabled \
                         on the agent; the app is not publicly reachable — enable it with: \
                         sudo rpi agent setup --with-cloudflared --cf-token <token> --domain <zone>"
                    ));
                }
                Err(e) => {
                    log.stage(&StageEvent::failed("route", t0.elapsed()));
                    return Err(e);
                }
            }
        }
```

GC (replaces the `if let Err(err) = staged("gc", ...)` block):

```rust
        log.stage(&StageEvent::started("gc"));
        let t0 = std::time::Instant::now();
        match staged("gc", GC_TIMEOUT_SECS, self.gc.execute(log.clone())).await {
            Ok(()) => log.stage(&StageEvent::ok("gc", t0.elapsed())),
            Err(err) => {
                log.stage(&StageEvent::skipped("gc", t0.elapsed()));
                log.line(&format!("gc skipped: {err}"));
            }
        }
```

- [ ] **Step 4: Fix the mockall `ps` fallout.** Every test whose deploy passes the health gate now needs a `ps` expectation. Add to each of: `lan_expose_writes_override_bound_to_all_interfaces`, `lan_deploy_logs_reachable_url`, `lan_deploy_logs_port_when_ip_not_detected`, `lan_deploy_warns_when_ip_is_public`, `stored_bundle_is_written_to_workdir_and_masked_in_logs`, `project_without_hostname_skips_ingress`, `skipped_ingress_emits_final_warning_line`, `gc_failure_does_not_fail_the_deploy` (and the new tests from Step 1 that succeed past health):

```rust
        m.runtime.expect_ps().returning(|_| Ok(vec![]));
```

`builds_of_different_projects_are_serialized_by_global_semaphore` uses `CountingRuntime`, whose `ps` already returns `Ok(vec![])` — no change. `health_gate_failure_fails_deploy_and_skips_ingress` and `build_failure_records_failed_and_emits_finished_failed` never reach `ps` — no change.

- [ ] **Step 5: Run tests**

Run: `rtk cargo test --locked -p pi-application`
Expected: PASS, including the extended happy-path stage-sequence assertion.

- [ ] **Step 6: Commit**

```bash
rtk git add crates/application/src/deploy.rs
rtk git commit -m "feat(agent): emit stage events and service summary from run_stages"
```

---

### Task 4: Hub event backlog + SSE `stage`/`summary` events

**Files:**
- Modify: `crates/infrastructure/src/events.rs`
- Modify: `crates/bin/src/agent/http.rs:381-393` (`sse_log` area) and `:501-541` (`deployment_logs`)

**Interfaces:**
- Consumes: `StageEvent`, `StageStatus` (Task 1); `HubSink` receives `stage`/`summary` calls from the deploy chain (Task 3).
- Produces:
  - `DeployEvent { Line(String), Stage(StageEvent), Summary(usize), Finished(DeploymentStatus) }`.
  - `Subscription { pub backlog: Vec<DeployEvent>, pub live: broadcast::Receiver<DeployEvent> }` (backlog type changes from `Vec<String>`).
  - SSE wire shapes: `event: stage` / `data: {"stage":"fetch","status":"ok","elapsed_ms":2100}` (field order fixed by a `#[derive(Serialize)]` struct; `elapsed_ms` omitted when `None`) and `event: summary` / `data: {"services":2}`.

- [ ] **Step 1: Write failing hub tests** — in `crates/infrastructure/src/events.rs`, update the existing tests to the event-typed backlog and add a mixed-order test:

```rust
    #[tokio::test]
    async fn backlog_replays_lines_stages_and_summary_in_order() {
        use pi_domain::entities::{StageEvent, StageStatus};
        let hub = DeployEventsHub::new();
        let sink = hub.register("d1");
        sink.stage(&StageEvent::started("fetch"));
        sink.line("cloning");
        sink.stage(&StageEvent::ok("fetch", std::time::Duration::from_millis(2100)));
        sink.summary(2);

        let sub = hub.subscribe("d1").unwrap();
        match &sub.backlog[..] {
            [DeployEvent::Stage(s0), DeployEvent::Line(l), DeployEvent::Stage(s1), DeployEvent::Summary(n)] =>
            {
                assert_eq!(s0.status, StageStatus::Started);
                assert_eq!(l, "cloning");
                assert_eq!(s1.elapsed_ms, Some(2100));
                assert_eq!(*n, 2);
            }
            other => panic!("unexpected backlog: {other:?}"),
        }
    }
```

Update `subscriber_gets_backlog_then_live_events` and `backlog_is_capped` to match `DeployEvent::Line(..)` variants instead of bare strings (e.g. `assert!(matches!(&sub.backlog[0], DeployEvent::Line(l) if l == "early-1"))`; the cap test's first-element check becomes `DeployEvent::Line(l) if l == "line-5"`).

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test --locked -p pi-infrastructure events`
Expected: compile error (`DeployEvent` has no `Stage`, backlog is `Vec<String>`).

- [ ] **Step 3: Implement the hub.** In `crates/infrastructure/src/events.rs` (import `StageEvent`):

```rust
#[derive(Debug, Clone)]
pub enum DeployEvent {
    Line(String),
    Stage(StageEvent),
    Summary(usize),
    Finished(DeploymentStatus),
}

struct StreamState {
    backlog: VecDeque<DeployEvent>,
    tx: broadcast::Sender<DeployEvent>,
}
```

`register` initialises `backlog: VecDeque::new()`. `subscribe` clones the event backlog. `HubSink` gets a private push helper and the trait methods:

```rust
impl HubSink {
    fn push(&self, ev: DeployEvent) {
        if let Ok(mut streams) = self.hub.streams.lock() {
            if let Some(s) = streams.get_mut(&self.id) {
                if s.backlog.len() == HUB_BACKLOG {
                    s.backlog.pop_front();
                }
                s.backlog.push_back(ev.clone());
                let _ = s.tx.send(ev);
            }
        }
    }
}

impl LogSink for HubSink {
    fn line(&self, line: &str) {
        self.push(DeployEvent::Line(line.to_string()));
    }

    fn stage(&self, ev: &StageEvent) {
        self.push(DeployEvent::Stage(ev.clone()));
    }

    fn summary(&self, services: usize) {
        self.push(DeployEvent::Summary(services));
    }

    fn finished(&self, status: DeploymentStatus) {
        // unchanged: remove the entry, notify live receivers
        if let Ok(mut streams) = self.hub.streams.lock() {
            if let Some(s) = streams.remove(&self.id) {
                let _ = s.tx.send(DeployEvent::Finished(status));
            }
        }
    }
}
```

- [ ] **Step 4: Write the failing HTTP test** — in `crates/bin/src/agent/http.rs` tests:

```rust
    #[tokio::test]
    async fn deployment_logs_carry_stage_and_summary_sse_events() {
        use pi_domain::entities::StageEvent;
        let dir = tempfile::tempdir().unwrap();
        let state = state_with(dir.path(), Arc::new(ok_source()), Arc::new(ok_runtime()));
        let app = router(state.clone());

        let sink = state.hub.register("dep-sse");
        sink.line("cloning");
        sink.stage(&StageEvent::started("fetch"));
        sink.stage(&StageEvent::ok("fetch", std::time::Duration::from_millis(2100)));
        sink.summary(2);
        let closer = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            sink.finished(DeploymentStatus::Success);
        });

        let (status, body) = request_text(app, get_req("/v1/deployments/dep-sse/logs")).await;
        closer.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("event: log"), "{body}");
        assert!(body.contains("event: stage"), "{body}");
        assert!(
            body.contains(r#"{"stage":"fetch","status":"started"}"#),
            "{body}"
        );
        assert!(
            body.contains(r#"{"stage":"fetch","status":"ok","elapsed_ms":2100}"#),
            "{body}"
        );
        assert!(body.contains("event: summary"), "{body}");
        assert!(body.contains(r#"{"services":2}"#), "{body}");
        assert!(body.contains("event: finished"), "{body}");
        assert!(body.contains("data: success"), "{body}");
    }
```

(If the tests use a different GET-request helper name than `get_req`, match the existing one in the file.)

- [ ] **Step 5: Implement the SSE mapping.** In `crates/bin/src/agent/http.rs`, next to `sse_log`:

```rust
fn sse_stage(ev: &pi_domain::entities::StageEvent) -> Result<Event, Infallible> {
    // Field order is part of the wire contract tests; a derive struct keeps
    // it stable regardless of serde_json map ordering.
    #[derive(serde::Serialize)]
    struct StageDto<'a> {
        stage: &'a str,
        status: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        elapsed_ms: Option<u64>,
    }
    let dto = StageDto {
        stage: &ev.stage,
        status: ev.status.as_str(),
        elapsed_ms: ev.elapsed_ms,
    };
    Ok(Event::default()
        .event("stage")
        .data(serde_json::to_string(&dto).unwrap_or_default()))
}

fn sse_summary(services: usize) -> Result<Event, Infallible> {
    Ok(Event::default()
        .event("summary")
        .data(format!("{{\"services\":{services}}}")))
}
```

In `deployment_logs`, the live-hub branch becomes:

```rust
    if let Some(sub) = state.hub.subscribe(&id) {
        let stream = async_stream::stream! {
            for ev in sub.backlog {
                match ev {
                    DeployEvent::Line(line) => yield sse_log(line),
                    DeployEvent::Stage(ev) => yield sse_stage(&ev),
                    DeployEvent::Summary(n) => yield sse_summary(n),
                    DeployEvent::Finished(status) => {
                        yield sse_finished(status.as_str());
                        return;
                    }
                }
            }
            let mut live = sub.live;
            loop {
                match live.recv().await {
                    Ok(DeployEvent::Line(line)) => yield sse_log(line),
                    Ok(DeployEvent::Stage(ev)) => yield sse_stage(&ev),
                    Ok(DeployEvent::Summary(n)) => yield sse_summary(n),
                    Ok(DeployEvent::Finished(status)) => {
                        yield sse_finished(status.as_str());
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        };
        return Ok(Sse::new(stream)
            .keep_alive(KeepAlive::default())
            .into_response());
    }
```

(The DB-fallback branch is untouched — it only has lines.)

- [ ] **Step 6: Run tests**

Run: `rtk cargo test --locked -p pi-infrastructure && rtk cargo test --locked -p pi deployment_logs`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
rtk git add crates/infrastructure/src/events.rs crates/bin/src/agent/http.rs
rtk git commit -m "feat(agent): stage/summary SSE events, event-typed hub backlog"
```

---

### Task 5: CLI — typed `follow_logs` events

**Files:**
- Modify: `crates/bin/src/cli/api.rs:152-187` (`follow_logs`)
- Modify: `crates/bin/src/cli/commands.rs:50-57` (deploy's `follow_logs` closure — minimal adaptation, full Pipeline wiring is Task 8)

**Interfaces:**
- Consumes: wire shapes from Task 4.
- Produces (Task 8 relies on these):

```rust
#[derive(Debug, serde::Deserialize)]
pub struct StageEventDto {
    pub stage: String,
    pub status: String,
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
}

pub enum DeployStreamEvent<'a> {
    Line(&'a str),
    Stage(StageEventDto),
    Summary { services: usize },
}

pub async fn follow_logs(
    &self,
    id: &str,
    mut on_event: impl FnMut(DeployStreamEvent<'_>),
) -> anyhow::Result<String>
```

- [ ] **Step 1: Write the failing tests** — in `crates/bin/src/cli/api.rs` tests (add a module if none exists), testing the extracted parse helpers:

```rust
    #[test]
    fn parse_stage_accepts_valid_and_rejects_malformed_payloads() {
        let ev = parse_stage(r#"{"stage":"build","status":"ok","elapsed_ms":48231}"#).unwrap();
        assert_eq!(ev.stage, "build");
        assert_eq!(ev.status, "ok");
        assert_eq!(ev.elapsed_ms, Some(48231));

        let started = parse_stage(r#"{"stage":"fetch","status":"started"}"#).unwrap();
        assert_eq!(started.elapsed_ms, None);

        assert!(parse_stage("not json").is_none());
        assert!(parse_stage(r#"{"status":"ok"}"#).is_none(), "missing stage field");
    }

    #[test]
    fn parse_summary_accepts_valid_and_rejects_malformed_payloads() {
        assert_eq!(parse_summary(r#"{"services":2}"#), Some(2));
        assert_eq!(parse_summary("nope"), None);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test --locked -p pi parse_stage`
Expected: compile error — `parse_stage` not found.

- [ ] **Step 3: Implement.** In `crates/bin/src/cli/api.rs` add the types from the Interfaces block above, plus:

```rust
fn parse_stage(data: &str) -> Option<StageEventDto> {
    serde_json::from_str(data).ok()
}

fn parse_summary(data: &str) -> Option<usize> {
    #[derive(serde::Deserialize)]
    struct SummaryDto {
        services: usize,
    }
    serde_json::from_str::<SummaryDto>(data).ok().map(|s| s.services)
}
```

and rewrite the event dispatch inside `follow_logs` (malformed payloads are ignored — forward compatibility):

```rust
            for ev in parser.push(&text) {
                match ev.event.as_str() {
                    "log" => on_event(DeployStreamEvent::Line(&ev.data)),
                    "stage" => {
                        if let Some(dto) = parse_stage(&ev.data) {
                            on_event(DeployStreamEvent::Stage(dto));
                        }
                    }
                    "summary" => {
                        if let Some(services) = parse_summary(&ev.data) {
                            on_event(DeployStreamEvent::Summary { services });
                        }
                    }
                    "finished" => return Ok(ev.data),
                    _ => {}
                }
            }
```

In `crates/bin/src/cli/commands.rs`, adapt deploy's closure so the crate compiles (behaviour unchanged for now; import `DeployStreamEvent` from `crate::cli::api`):

```rust
    let status = api
        .follow_logs(&accepted.deployment_id, |ev| {
            if let DeployStreamEvent::Line(line) = ev {
                if let Some(w) = deploy_warning(line) {
                    warnings.push(w.to_string());
                }
                pane.push_line(line)
            }
        })
        .await?;
```

- [ ] **Step 4: Run tests**

Run: `rtk cargo test --locked -p pi`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/bin/src/cli/api.rs crates/bin/src/cli/commands.rs
rtk git commit -m "feat(cli): typed stage/summary events in follow_logs"
```

---

### Task 6: `LogPane::clear` / `LogPane::abort`

**Files:**
- Modify: `crates/bin/src/output/logpane.rs`

**Interfaces:**
- Produces (Pipeline in Task 7 relies on):
  - `pub fn clear(self)` — interactive: erase the frame, print nothing; non-interactive: no output.
  - `pub fn abort(self, dump_label: &str)` — interactive: recolour the frame red in place, print `— {dump_label} —` (muted) and then every captured line; non-interactive: no output (lines already streamed). `finish_err(summary)` is re-expressed as `abort("full log")` + `crate::output::error(summary)` — behaviour identical to today.
  - `new_recording` visibility becomes `pub(crate)` (still `#[cfg(test)]`) so Pipeline tests can build recording panes.

- [ ] **Step 1: Write the failing tests** — append to `logpane.rs` tests:

```rust
    #[test]
    fn clear_prints_nothing_through_print_line() {
        let printed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut pane = LogPane::new_recording("test", 3, true, printed.clone());
        pane.push_line("one");
        pane.clear();
        assert!(printed.lock().unwrap().is_empty());
    }

    #[test]
    fn abort_dumps_history_under_a_custom_label() {
        let printed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut pane = LogPane::new_recording("build", 3, true, printed.clone());
        pane.push_line("step 1");
        pane.push_line("boom");
        pane.abort("build log");
        assert_eq!(
            *printed.lock().unwrap(),
            vec!["— build log —", "step 1", "boom"]
        );
    }

    #[test]
    fn abort_non_interactive_adds_nothing_lines_already_streamed() {
        let printed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut pane = LogPane::new_recording("build", 3, false, printed.clone());
        pane.push_line("one");
        pane.abort("build log");
        assert_eq!(*printed.lock().unwrap(), vec!["one"]);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test --locked -p pi logpane`
Expected: compile error — no method `clear`/`abort`.

- [ ] **Step 3: Implement** in `logpane.rs`:

```rust
    /// Erase the live frame without printing anything — used by the deploy
    /// pipeline to collapse a stage pane before printing its own summary line.
    pub fn clear(self) {
        if self.interactive {
            let _ = self.term.clear_last_lines(self.rendered);
        }
    }

    /// Failure treatment without a summary line: recolour the frame red in
    /// place and dump the full captured log under a `— {dump_label} —`
    /// separator. Non-interactive runs already streamed every line.
    pub fn abort(self, dump_label: &str) {
        if self.interactive {
            let (_, cols) = self.term.size();
            let width = (cols as usize).max(20);
            let frame = render_frame(
                &self.label,
                &self.visible,
                width,
                self.rendered,
                &err_frame(),
            );
            let _ = self.term.write_str(&frame);
            let _ = self.term.flush();
            (self.print_line)(
                &console_style(Sem::Muted)
                    .apply_to(format!("— {dump_label} —"))
                    .to_string(),
            );
            for l in &self.full {
                (self.print_line)(l);
            }
        }
    }

    pub fn finish_err(self, summary: &str) {
        self.abort("full log");
        crate::output::error(summary);
    }
```

(Delete the old `finish_err` body; the existing `finish_err_*` tests must still pass unchanged.) Change `fn new_recording` to `pub(crate) fn new_recording`.

- [ ] **Step 4: Run tests**

Run: `rtk cargo test --locked -p pi logpane`
Expected: PASS, including the pre-existing `finish_err_dumps_full_history_when_interactive`.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/bin/src/output/logpane.rs
rtk git commit -m "refactor(output): LogPane clear/abort primitives for the pipeline view"
```

---

### Task 7: `output::Pipeline` orchestrator

**Files:**
- Create: `crates/bin/src/output/pipeline.rs`
- Modify: `crates/bin/src/output/mod.rs` (add `mod pipeline; pub use pipeline::Pipeline;`)

**Interfaces:**
- Consumes: `LogPane` (`new`, `push_line`, `clear`, `abort`, `finish_ok`, `finish_neutral`, `finish_err`, and `pub(crate) new_recording` in tests), `console_style(Sem)` from `output/mod.rs`, `crate::duration::format_elapsed`.
- Produces (Task 8 relies on):

```rust
pub struct Pipeline { /* private */ }
impl Pipeline {
    pub fn new(project: &str) -> Pipeline;              // opens legacy pane "deploy '<project>'"
    pub fn push_line(&mut self, line: &str);
    pub fn stage(&mut self, stage: &str, status: &str, elapsed_ms: Option<u64>);
    pub fn summary(&mut self, services: usize);
    pub fn services(&self) -> Option<usize>;
    pub fn finish_ok(self, stamp: &str);
    pub fn finish_neutral(self, stamp: &str);
    pub fn finish_err(self, stamp: &str);
}
```

- [ ] **Step 1: Write the failing tests** — `crates/bin/src/output/pipeline.rs` is created test-first; put the tests module in the same file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn recording(interactive: bool) -> (Pipeline, Arc<Mutex<Vec<String>>>) {
        let printed = Arc::new(Mutex::new(Vec::new()));
        (
            Pipeline::new_recording("myboard", interactive, printed.clone()),
            printed,
        )
    }

    #[test]
    fn legacy_stream_without_stage_events_streams_lines_as_today() {
        let (mut p, printed) = recording(false);
        p.push_line("project 'myboard': host port 8000");
        p.push_line("fetched abc");
        assert_eq!(
            *printed.lock().unwrap(),
            vec!["project 'myboard': host port 8000", "fetched abc"]
        );
    }

    #[test]
    fn interactive_stage_ok_collapses_to_a_summary_line() {
        let (mut p, printed) = recording(true);
        p.stage("build", "started", None);
        p.push_line("step 1/9");
        p.stage("build", "ok", Some(48_300));
        // Interactive pane lines are drawn on the terminal, not print_line;
        // only the collapse summary goes through print_line.
        assert_eq!(*printed.lock().unwrap(), vec!["✓ build (48.3s)"]);
    }

    #[test]
    fn failed_stage_dumps_only_its_own_lines() {
        let (mut p, printed) = recording(true);
        p.stage("fetch", "started", None);
        p.push_line("cloning");
        p.stage("fetch", "ok", Some(2_100));
        p.stage("build", "started", None);
        p.push_line("compile error");
        p.stage("build", "failed", Some(12_100));
        assert_eq!(
            *printed.lock().unwrap(),
            vec![
                "✓ fetch (2.1s)",
                "— build log —",
                "compile error",
                "✗ build (12.1s)"
            ]
        );
    }

    #[test]
    fn skipped_stage_prints_a_dim_note() {
        let (mut p, printed) = recording(true);
        p.stage("route", "started", None);
        p.stage("route", "skipped", Some(400));
        assert_eq!(*printed.lock().unwrap(), vec!["· route skipped (0.4s)"]);
    }

    #[test]
    fn non_interactive_prints_boundary_lines_on_completion_only() {
        let (mut p, printed) = recording(false);
        p.stage("build", "started", None);
        p.push_line("step 1/9");
        p.stage("build", "ok", Some(48_300));
        assert_eq!(
            *printed.lock().unwrap(),
            vec!["step 1/9", "▸ build ok (48.3s)"]
        );
    }

    #[test]
    fn lines_between_stages_print_plain() {
        let (mut p, printed) = recording(true);
        p.stage("fetch", "started", None);
        p.stage("fetch", "ok", Some(2_100));
        p.push_line("secrets injected (2 keys, 0 files)");
        assert_eq!(
            *printed.lock().unwrap(),
            vec!["✓ fetch (2.1s)", "secrets injected (2 keys, 0 files)"]
        );
    }

    #[test]
    fn unknown_status_is_ignored() {
        let (mut p, printed) = recording(true);
        p.stage("build", "paused", Some(10));
        assert!(printed.lock().unwrap().is_empty());
    }

    #[test]
    fn summary_is_stored_for_the_stamp() {
        let (mut p, _) = recording(true);
        assert_eq!(p.services(), None);
        p.summary(2);
        assert_eq!(p.services(), Some(2));
    }
}
```

Notes for the implementer: colours are disabled in the test environment, so styled strings compare equal to their plain text; the glyphs `✓`/`✗`/`·`/`▸` are produced via `console::Emoji` and tests rely on the unicode branch (mirror how `logpane.rs` tests assert glyphs directly).

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test --locked -p pi pipeline`
Expected: compile error — module does not exist yet (add `mod pipeline;` + `pub use` to `output/mod.rs` first so failure is about the missing type, or together with Step 3).

- [ ] **Step 3: Implement** `crates/bin/src/output/pipeline.rs`:

```rust
use std::time::Duration;

use console::Emoji;

use super::{console_style, LogPane, Sem};

const MAX_VISIBLE: usize = 10;

static CHECK: Emoji<'_, '_> = Emoji("✓", "ok");
static CROSS: Emoji<'_, '_> = Emoji("✗", "x");
static DOT: Emoji<'_, '_> = Emoji("·", "-");
static MARKER: Emoji<'_, '_> = Emoji("▸", ">");

/// Deploy stream orchestrator (deploy-stages spec): starts as today's single
/// `deploy '<project>'` pane and, on the first stage event from the agent,
/// switches to pipeline mode — one collapsing pane per stage. Old agents never
/// send stage events, so legacy behaviour is preserved byte-for-byte.
pub struct Pipeline {
    pane: Option<LogPane>,
    /// Name of the currently open stage pane (None: legacy pane or between stages).
    current: Option<String>,
    staged_mode: bool,
    interactive: bool,
    services: Option<usize>,
    print_line: Box<dyn Fn(&str)>,
    #[cfg(test)]
    recording: Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>,
}

impl Pipeline {
    pub fn new(project: &str) -> Pipeline {
        let interactive = console::Term::stdout().features().is_attended();
        Pipeline {
            pane: Some(LogPane::new(format!("deploy '{project}'"), MAX_VISIBLE)),
            current: None,
            staged_mode: false,
            interactive,
            services: None,
            print_line: Box::new(|l: &str| println!("{l}")),
            #[cfg(test)]
            recording: None,
        }
    }

    #[cfg(test)]
    fn new_recording(
        project: &str,
        interactive: bool,
        printed: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> Pipeline {
        let sink = printed.clone();
        Pipeline {
            pane: Some(LogPane::new_recording(
                format!("deploy '{project}'"),
                MAX_VISIBLE,
                interactive,
                printed.clone(),
            )),
            current: None,
            staged_mode: false,
            interactive,
            services: None,
            print_line: Box::new(move |l: &str| sink.lock().unwrap().push(l.to_string())),
            #[cfg(test)]
            recording: Some(printed),
        }
    }

    fn open_pane(&self, label: &str) -> LogPane {
        #[cfg(test)]
        if let Some(printed) = &self.recording {
            return LogPane::new_recording(label, MAX_VISIBLE, self.interactive, printed.clone());
        }
        LogPane::new(label, MAX_VISIBLE)
    }

    pub fn push_line(&mut self, line: &str) {
        match &mut self.pane {
            Some(pane) => pane.push_line(line),
            None => (self.print_line)(line),
        }
    }

    pub fn summary(&mut self, services: usize) {
        self.services = Some(services);
    }

    pub fn services(&self) -> Option<usize> {
        self.services
    }

    fn elapsed_suffix(elapsed_ms: Option<u64>) -> String {
        elapsed_ms
            .map(|ms| {
                format!(
                    " ({})",
                    crate::duration::format_elapsed(Duration::from_millis(ms))
                )
            })
            .unwrap_or_default()
    }

    fn print_done(&self, stage: &str, status: &str, elapsed_ms: Option<u64>) {
        let elapsed = Self::elapsed_suffix(elapsed_ms);
        let line = if !self.interactive {
            format!("{MARKER} {stage} {status}{elapsed}")
        } else {
            match status {
                "ok" => format!(
                    "{} {stage}{}",
                    console_style(Sem::Success).apply_to(CHECK.to_string()),
                    console_style(Sem::Muted).apply_to(elapsed),
                ),
                "failed" => console_style(Sem::Error)
                    .apply_to(format!("{CROSS} {stage}{elapsed}"))
                    .to_string(),
                // skipped
                _ => console_style(Sem::Muted)
                    .apply_to(format!("{DOT} {stage} skipped{elapsed}"))
                    .to_string(),
            }
        };
        (self.print_line)(&line);
    }

    pub fn stage(&mut self, stage: &str, status: &str, elapsed_ms: Option<u64>) {
        match status {
            "started" => {
                // First stage event: silently collapse the legacy pane and
                // enter pipeline mode. Also collapses a stage pane whose
                // completion never arrived (defensive).
                self.staged_mode = true;
                if let Some(pane) = self.pane.take() {
                    pane.clear();
                }
                self.pane = Some(self.open_pane(stage));
                self.current = Some(stage.to_string());
            }
            "ok" | "skipped" => {
                self.staged_mode = true;
                if let Some(pane) = self.pane.take() {
                    pane.clear();
                }
                self.current = None;
                self.print_done(stage, status, elapsed_ms);
            }
            "failed" => {
                self.staged_mode = true;
                if let Some(pane) = self.pane.take() {
                    pane.abort(&format!("{stage} log"));
                }
                self.current = None;
                self.print_done(stage, status, elapsed_ms);
            }
            _ => {} // unknown status: forward compatibility, ignore
        }
    }

    pub fn finish_ok(self, stamp: &str) {
        match (self.staged_mode, self.pane) {
            (false, Some(pane)) => pane.finish_ok(stamp),
            (true, pane) => {
                if let Some(p) = pane {
                    p.clear();
                }
                crate::output::success(stamp);
            }
            (false, None) => crate::output::success(stamp),
        }
    }

    pub fn finish_neutral(self, stamp: &str) {
        match (self.staged_mode, self.pane) {
            (false, Some(pane)) => pane.finish_neutral(stamp),
            (true, pane) => {
                if let Some(p) = pane {
                    p.clear();
                }
                crate::output::note(stamp);
            }
            (false, None) => crate::output::note(stamp),
        }
    }

    pub fn finish_err(self, stamp: &str) {
        match (self.staged_mode, self.pane, self.current) {
            (false, Some(pane), _) => pane.finish_err(stamp),
            (true, Some(pane), current) => {
                let label = current.map(|s| format!("{s} log")).unwrap_or("log".into());
                pane.abort(&label);
                crate::output::error(stamp);
            }
            (_, None, _) => crate::output::error(stamp),
        }
    }
}
```

Wire it up in `crates/bin/src/output/mod.rs` next to the other modules:

```rust
mod pipeline;
pub use pipeline::Pipeline;
```

Adjust to reality where needed: if `console_style` / `Sem` are not already importable from `super`, mirror exactly how `logpane.rs` imports them (`use super::{console_style, Sem};`). The `unknown_status_is_ignored` test intentionally hits the `_ => {}` arm — note the legacy pane must NOT be collapsed there (move `self.staged_mode = true;` out of the `_` arm, as shown).

- [ ] **Step 4: Run tests**

Run: `rtk cargo test --locked -p pi pipeline`
Expected: PASS (8 tests). Watch the two subtle ones: `unknown_status_is_ignored` (nothing printed, legacy pane untouched) and `failed_stage_dumps_only_its_own_lines` (dump contains only the build lines).

- [ ] **Step 5: Commit**

```bash
rtk git add crates/bin/src/output/pipeline.rs crates/bin/src/output/mod.rs
rtk git commit -m "feat(cli): Pipeline orchestrator - collapsing stage panes"
```

---

### Task 8: Wire `deploy` to the Pipeline; service count in the stamp

**Files:**
- Modify: `crates/bin/src/output/banner.rs` (`deploy_stamp`, `deploy_stamp_inner` + tests)
- Modify: `crates/bin/src/cli/commands.rs:15-92` (`deploy`)

**Interfaces:**
- Consumes: `Pipeline` (Task 7), `DeployStreamEvent` (Task 5).
- Produces: `pub fn deploy_stamp(outcome: StampOutcome, project: &str, url: Option<&str>, services: Option<usize>, elapsed: Duration) -> String` — the only signature change visible outside.

- [ ] **Step 1: Write the failing test** — in `banner.rs` tests:

```rust
    #[test]
    fn stamp_success_includes_service_count_when_known() {
        let with = deploy_stamp_inner(
            true,
            StampOutcome::Success,
            "myboard",
            Some("rpi.iiskelo.com"),
            Some(2),
            Duration::from_secs(64),
        );
        assert!(with.contains("· 2 services"), "{with:?}");

        let one = deploy_stamp_inner(
            true,
            StampOutcome::Success,
            "myboard",
            None,
            Some(1),
            Duration::from_secs(5),
        );
        assert!(one.contains("· 1 service"), "{one:?}");
        assert!(!one.contains("1 services"), "{one:?}");

        let without = deploy_stamp_inner(
            true,
            StampOutcome::Success,
            "myboard",
            None,
            None,
            Duration::from_secs(5),
        );
        assert!(!without.contains("service"), "{without:?}");
    }
```

Update the two existing `deploy_stamp_inner` tests to pass `None` for the new `services` argument (between `url` and `elapsed`).

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test --locked -p pi stamp`
Expected: compile error — wrong number of arguments.

- [ ] **Step 3: Implement.** In `banner.rs`, add the `services: Option<usize>` parameter to both `deploy_stamp` and `deploy_stamp_inner` (between `url` and `elapsed`), and extend the Success arm only:

```rust
        StampOutcome::Success => {
            let check = if unicode { "✓" } else { "ok" };
            let arrow = if unicode { "→" } else { "->" };
            let dest = url.map(|u| format!("  {arrow}  {u}")).unwrap_or_default();
            let svc = services
                .map(|n| format!(" · {n} {}", if n == 1 { "service" } else { "services" }))
                .unwrap_or_default();
            format!("deployed {check} {project}{dest}{svc} ({elapsed})")
        }
```

In `commands.rs`, `deploy()` — replace the `LogPane` + closure + match block:

```rust
    let mut pipeline = output::Pipeline::new(&rpitoml.project.name);
    let mut warnings: Vec<String> = Vec::new();
    let status = api
        .follow_logs(&accepted.deployment_id, |ev| match ev {
            DeployStreamEvent::Line(line) => {
                if let Some(w) = deploy_warning(line) {
                    warnings.push(w.to_string());
                }
                pipeline.push_line(line)
            }
            DeployStreamEvent::Stage(dto) => {
                pipeline.stage(&dto.stage, &dto.status, dto.elapsed_ms)
            }
            DeployStreamEvent::Summary { services } => pipeline.summary(services),
        })
        .await?;
    let elapsed = started.elapsed();
    let name = &rpitoml.project.name;
    let url = rpitoml.ingress.hostname.as_deref();
    let services = pipeline.services();
    match status.as_str() {
        "success" => pipeline.finish_ok(&output::deploy_stamp(
            output::StampOutcome::Success,
            name,
            url,
            services,
            elapsed,
        )),
        "superseded" => pipeline.finish_neutral(&output::deploy_stamp(
            output::StampOutcome::Superseded,
            name,
            url,
            services,
            elapsed,
        )),
        _ => {
            pipeline.finish_err(&output::deploy_stamp(
                output::StampOutcome::Failed,
                name,
                url,
                services,
                elapsed,
            ));
            for w in &warnings {
                output::warn(w);
            }
            drop(tunnel);
            std::process::exit(1);
        }
    }
```

(`output::LogPane` may drop out of `deploy`'s imports; `rpi command` still uses it elsewhere — leave the re-export alone.)

- [ ] **Step 4: Run tests**

Run: `rtk cargo test --locked -p pi`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/bin/src/output/banner.rs crates/bin/src/cli/commands.rs
rtk git commit -m "feat(cli): deploy renders the stage pipeline, stamp shows service count"
```

---

### Task 9: Docs, full gate, end-to-end sanity

**Files:**
- Modify: `README.md` (the feature-list paragraph around lines 25-42 that describes the log pane)

- [ ] **Step 1: README.** Extend the log-pane sentence with the pipeline view, e.g.: `rpi deploy` now renders each stage (`fetch → build → start → health → route → gc`) as a collapsing timed pane with a `✓ build (48.3s)` summary per stage and a service count in the final stamp; older CLI/agent combinations keep the previous single-pane view.

- [ ] **Step 2: Full gate** (required by project CLAUDE.md):

Run:
```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
```
Expected: all three exit 0. If fmt reports a diff: `rtk cargo fmt --all` and include in the commit.

- [ ] **Step 3: Manual sanity (optional but recommended).** With a local dev agent (see the `rpi-cli` skill: `rpi agent run` + `PI_AGENT_URL`), run `rpi deploy` against a sample project and eyeball: panes collapse with timings, failure dumps only the failed stage, `rpi deploy | cat` shows `▸ build ok (…)` boundary lines.

- [ ] **Step 4: Commit**

```bash
rtk git add README.md
rtk git commit -m "docs: deploy pipeline view in README"
```

---

## Self-Review Notes (already applied)

- Spec coverage: protocol (T4/T5), agent internals incl. `up`→`start` rename and warning-order preservation (T3), sink chain + tail boundaries (T2), hub backlog (T4), CLI fallback/collapse/failure/non-interactive (T7), stamp service count (T8), compat matrix (T4 test keeps `finished` bare; T7 legacy test).
- Type registry: `StageEvent`/`StageStatus` (T1) → used in T2-T4; `StageEventDto`/`DeployStreamEvent` (T5) → consumed in T8; `Pipeline` API (T7) → consumed in T8; `deploy_stamp(.., services: Option<usize>, ..)` (T8).
- Known judgement calls left to the implementer: exact import paths inside `output/mod.rs` (`console_style`, `Sem` are module-private helpers — mirror `logpane.rs`), and the HTTP test's GET helper name (`get_req` in the current test module).
