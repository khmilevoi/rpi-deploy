# Cloudflared Adoption + Loud Manual-Ingress Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `sudo rpi agent setup --with-cloudflared --cf-token … --domain …` safely adopts a hand-built cloudflared install (never rewrites an existing config.yml, zero downtime), deploy-time cloudflared restarts stop failing on missing `XDG_RUNTIME_DIR`, and "hostname declared but ingress disabled" becomes loud in the deploy summary and `rpi doctor`.

**Spec:** `docs/superpowers/specs/2026-07-09-rpi-cloudflared-adoption-design.md`

**Architecture:** Four independent seams: (1) `Ingress::upsert` returns an `IngressOutcome` so `DeployProject` can emit a final `warning:` log line that the CLI re-surfaces next to the summary; (2) `HostSystemProbe` gains an `ingress_active` flag and a failing `ingress routing` doctor check; (3) `CloudflaredIngress` injects `XDG_RUNTIME_DIR=/run/user/<uid>` into the restart command when the agent's env lacks it; (4) `cloudflared_bootstrap_full` branches into an adoption path when `config.yml` already exists — parse it, resolve the tunnel id, verify creds, write agent.toml sections, touch nothing else.

**Tech Stack:** Rust workspace (`pi-domain`, `pi-application`, `pi-infrastructure`, bin crate `pi`), mockall (`MockIngress`, `MockCloudflareApi`, `MockProjectRepository`), `FakeSys` in `setup.rs` tests, serde_yaml, libc (new, unix-only).

## Global Constraints

- Prefix every shell command with `rtk` (`rtk cargo test`, `rtk git commit …`) — CLAUDE.md golden rule.
- Before declaring any task complete: `rtk cargo fmt --all -- --check`, `rtk cargo clippy --all-targets --locked -- -D warnings`, `rtk cargo test --locked` must all pass (project CLAUDE.md; CI runs these on Linux).
- Dev machine is Windows, CI is Linux: anything unix-only (`libc::getuid`) goes behind `#[cfg(unix)]` with a `#[cfg(not(unix))]` fallback so the workspace builds and tests on both.
- CI uses `--locked`: any `Cargo.toml` dependency change must be committed together with the updated `Cargo.lock` (run `rtk cargo build` first to refresh it).
- Invariant from the spec (§3): an existing `/var/lib/rpi/cloudflared/config.yml` is NEVER written to by setup — no task may add a write to that path when the file already exists.
- The operator-facing enable command is exactly this string everywhere (deploy warning, doctor hint, README): `sudo rpi agent setup --with-cloudflared --cf-token <token> --domain <zone>`.
- Commit messages: conventional prefix (`feat:`/`fix:`/`test:`/`docs:`), body optional, and end with the line `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

### Task 1: `IngressOutcome` return value for `Ingress::upsert`

**Files:**
- Modify: `crates/domain/src/contracts.rs:192-203` (Ingress trait; add enum above it)
- Modify: `crates/infrastructure/src/cloudflared.rs` (`CloudflaredIngress::upsert` ~114-155, `DisabledIngress::upsert` ~245-256, tests ~357-382)
- Modify: `crates/application/src/deploy.rs` (test mock returns, ~508-515)

**Interfaces:**
- Consumes: existing `Ingress` trait, `DomainError`, `LogSink`.
- Produces: `pub enum IngressOutcome { Applied, Skipped }` in `pi_domain::contracts`; `Ingress::upsert(&self, hostname: &str, host_port: u16, log: Arc<dyn LogSink>) -> Result<IngressOutcome, DomainError>`. Task 2 matches on this. `remove` is unchanged.

- [ ] **Step 1: Write the failing tests** in `crates/infrastructure/src/cloudflared.rs` tests module (after `no_diff_skips_dns_and_restart_entirely`):

```rust
#[tokio::test]
async fn disabled_ingress_upsert_reports_skipped() {
    let ingress = DisabledIngress::new();
    let outcome = ingress
        .upsert("a.example.com", 8000, CollectSink::new())
        .await
        .unwrap();
    assert_eq!(outcome, IngressOutcome::Skipped);
}
```

Also extend `no_diff_skips_dns_and_restart_entirely`: change the bare `.unwrap();` on the upsert call to

```rust
let outcome = ingress
    .upsert("old.example.com", 8001, CollectSink::new())
    .await
    .unwrap();
assert_eq!(outcome, IngressOutcome::Applied);
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p pi-infrastructure cloudflared`
Expected: COMPILE ERROR — `IngressOutcome` not found / mismatched types (upsert returns `()`).

- [ ] **Step 3: Implement.** In `crates/domain/src/contracts.rs`, directly above the `Ingress` trait:

```rust
/// Result of an ingress upsert: `Applied` when the edge is (or already was)
/// routing the hostname, `Skipped` when a disabled backend did nothing and
/// routing remains manual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngressOutcome {
    Applied,
    Skipped,
}
```

Change the trait method signature:

```rust
    async fn upsert(
        &self,
        hostname: &str,
        host_port: u16,
        log: Arc<dyn LogSink>,
    ) -> Result<IngressOutcome, DomainError>;
```

In `crates/infrastructure/src/cloudflared.rs`: add `IngressOutcome` to the existing `use pi_domain::contracts::{…}` import; in `CloudflaredIngress::upsert` change the no-diff early return (`return Ok(());` after "already routed") to `return Ok(IngressOutcome::Applied);` and the final `Ok(())` to `Ok(IngressOutcome::Applied)`; in `DisabledIngress::upsert` change `Ok(())` to `Ok(IngressOutcome::Skipped)` and the return type of both `upsert` impls to `Result<IngressOutcome, DomainError>`.

In `crates/application/src/deploy.rs`: the call site compiles as-is (the `?` value is dropped), but the tests need: add `IngressOutcome` to the tests module's `use pi_domain::contracts::{…}` import and change the mock return in `happy_path_runs_all_stages_and_records_success` (~line 512) from `Ok(())` to `Ok(IngressOutcome::Applied)`.

- [ ] **Step 4: Run tests**

Run: `rtk cargo test --locked`
Expected: PASS (whole workspace — this catches any other implementor/caller you missed).

- [ ] **Step 5: fmt + clippy + commit**

```bash
rtk cargo fmt --all
rtk cargo clippy --all-targets --locked -- -D warnings
rtk git add crates/domain/src/contracts.rs crates/infrastructure/src/cloudflared.rs crates/application/src/deploy.rs
rtk git commit -m "feat(domain): Ingress::upsert reports Applied vs Skipped

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: agent emits a final `warning:` log line when ingress was skipped

**Files:**
- Modify: `crates/application/src/deploy.rs:300-312` (`run_stages` ingress + gc tail) and its tests module

**Interfaces:**
- Consumes: `IngressOutcome` from Task 1.
- Produces: on `Skipped` with a declared hostname, the LAST line of the deploy log is `warning: hostname <h> is declared but ingress is disabled on the agent; the app is not publicly reachable — enable it with: sudo rpi agent setup --with-cloudflared --cf-token <token> --domain <zone>`. Task 3's CLI parser relies on the exact `warning: ` prefix.

- [ ] **Step 1: Write the failing test** in the deploy.rs tests module (near the other execute-path tests; reuses `mocks()`, `build()`, `sample_config()`, `SHA`, `CollectSink`):

```rust
#[tokio::test]
async fn skipped_ingress_emits_final_warning_line() {
    let mut m = mocks();
    let cfg = sample_config(); // hostname = rateme.isskelo.com

    m.projects.expect_upsert().returning(|c| {
        Ok(Project {
            config: c.clone(),
            host_port: 8000,
            created_at: 1,
        })
    });
    m.source.expect_fetch().returning(|_, _, _| {
        Ok(FetchedSource {
            workdir: PathBuf::from("/w"),
            commit_sha: SHA.into(),
        })
    });
    m.secrets.expect_load().returning(|_| Ok(SecretsBundle::default()));
    m.overrides
        .expect_write()
        .returning(|_, _, _, _, _| Ok(PathBuf::from("/o.yml")));
    m.runtime.expect_build().returning(|_, _| Ok(()));
    m.runtime.expect_up().returning(|_, _| Ok(()));
    m.health.expect_check().returning(|_, _, _| Ok(()));
    m.ingress
        .expect_upsert()
        .times(1)
        .returning(|_, _, _| Ok(IngressOutcome::Skipped));
    m.history.expect_mark_running().returning(|_, _| Ok(()));
    m.history
        .expect_record_finished()
        .returning(|_, _, _, _, _| Ok(()));

    let deploy = build(m);
    let result = deploy
        .execute(
            "dep-w".into(),
            cfg,
            DeployRef::Branch("main".into()),
            CollectSink::new(),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(result.status, DeploymentStatus::Success);
    let last = result.log_tail.lines().last().unwrap_or_default();
    assert!(
        last.starts_with("warning: hostname rateme.isskelo.com"),
        "warning must be the last log line, got: {last}"
    );
    assert!(last.contains("sudo rpi agent setup --with-cloudflared"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p pi-application skipped_ingress_emits_final_warning_line`
Expected: FAIL — last line is not the warning (no warning emitted yet).

- [ ] **Step 3: Implement.** In `run_stages`, add `IngressOutcome` to the file's `use pi_domain::contracts::{…}` import, then replace the ingress + gc tail (currently lines ~300-311) with:

```rust
        // §11: route hostname only when configured
        let mut ingress_warning: Option<String> = None;
        if let Some(hostname) = &config.hostname {
            match self
                .ingress
                .upsert(hostname, project.host_port, log.clone())
                .await?
            {
                IngressOutcome::Applied => {}
                IngressOutcome::Skipped => {
                    ingress_warning = Some(format!(
                        "warning: hostname {hostname} is declared but ingress is disabled \
                         on the agent; the app is not publicly reachable — enable it with: \
                         sudo rpi agent setup --with-cloudflared --cf-token <token> --domain <zone>"
                    ));
                }
            }
        }

        if let Err(err) = staged("gc", GC_TIMEOUT_SECS, self.gc.execute(log.clone())).await {
            log.line(&format!("gc skipped: {err}"));
        }

        // Emitted last so it sits next to the final summary, not mid-stream.
        if let Some(w) = &ingress_warning {
            log.line(w);
        }

        Ok(fetched.commit_sha)
```

- [ ] **Step 4: Run tests**

Run: `rtk cargo test -p pi-application`
Expected: PASS (including the untouched happy-path test).

- [ ] **Step 5: fmt + clippy + commit**

```bash
rtk cargo fmt --all
rtk cargo clippy --all-targets --locked -- -D warnings
rtk git add crates/application/src/deploy.rs
rtk git commit -m "feat(deploy): trailing warning line when ingress is disabled but hostname declared

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: CLI re-surfaces `warning:` lines next to the deploy summary

**Files:**
- Modify: `crates/bin/src/cli/commands.rs` (`deploy` ~15-62, helper near `version_mismatch_warning` ~750, tests module at 760)

**Interfaces:**
- Consumes: `warning: `-prefixed log lines from Task 2 (opaque strings over the existing SSE `log` events — no wire change).
- Produces: user-visible `output::warn` lines printed after the log pane finishes, on success AND on failure.

- [ ] **Step 1: Write the failing test** in the existing `mod tests` of `commands.rs`:

```rust
#[test]
fn deploy_warning_extracts_only_prefixed_lines() {
    assert_eq!(deploy_warning("warning: x y"), Some("x y"));
    assert_eq!(deploy_warning("ingress: routing a -> b"), None);
    assert_eq!(deploy_warning(" warning: not at start"), None);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p pi deploy_warning_extracts`
Expected: COMPILE ERROR — `deploy_warning` not defined.

- [ ] **Step 3: Implement.** Next to `version_mismatch_warning` (~line 750):

```rust
/// A deploy log line the agent marked as a warning — re-surfaced next to the
/// final summary so it cannot scroll away with the stream.
fn deploy_warning(line: &str) -> Option<&str> {
    line.strip_prefix("warning: ")
}
```

In `deploy()` replace the pane/follow/match block (lines ~46-61) with:

```rust
    let mut pane = output::LogPane::new(format!("deploy '{}'", rpitoml.project.name), 10);
    let mut warnings: Vec<String> = Vec::new();
    let status = api
        .follow_logs(&accepted.deployment_id, |line| {
            if let Some(w) = deploy_warning(line) {
                warnings.push(w.to_string());
            }
            pane.push_line(line)
        })
        .await?;
    match status.as_str() {
        "success" => pane.finish_ok(&format!("deploy finished: {status}")),
        "superseded" => pane.finish_neutral(
            "deploy finished: superseded (a newer deploy request replaced this one - not an error)",
        ),
        _ => {
            pane.finish_err(&format!("deploy finished: {status}"));
            for w in &warnings {
                output::warn(w);
            }
            drop(tunnel);
            std::process::exit(1);
        }
    }
    for w in &warnings {
        output::warn(w);
    }
    Ok(())
```

- [ ] **Step 4: Run tests**

Run: `rtk cargo test -p pi`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
rtk cargo fmt --all
rtk cargo clippy --all-targets --locked -- -D warnings
rtk git add crates/bin/src/cli/commands.rs
rtk git commit -m "feat(cli): repeat deploy warnings next to the final summary

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: `rpi doctor` check — hostnames declared while ingress is disabled

**Files:**
- Modify: `crates/infrastructure/src/probe.rs` (struct ~32-61, `diagnostics()` insert after cloudflared block ~176, new tests module at end of file)
- Modify: `crates/bin/src/agent/state.rs:77-110` (capture `ingress_active`) and `:164-172` (pass it)
- Modify: `crates/bin/src/agent/http.rs:818-826` (test call site — one added arg)

**Interfaces:**
- Consumes: `ProjectRepository::list()` (already a field on `HostSystemProbe`), `DiagnosticCheck` entity.
- Produces: `HostSystemProbe::new(runner, disk, projects, version, disk_threshold_percent, cloudflared_enabled, ingress_active: bool, started_at)` — new 7th parameter, before `started_at`. A failing check named `"ingress routing"` when `!ingress_active` and any registered project has a hostname. Diagnostics stay wire-compatible (generic `DiagnosticCheckDto`).

- [ ] **Step 1: Write the failing tests** — new module at the end of `crates/infrastructure/src/probe.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use pi_domain::contracts::{MockDiskProbe, MockProjectRepository, SystemProbe};
    use pi_domain::entities::{
        ExposeMode, HealthcheckConfig, Project, ProjectConfig, StageTimeoutOverrides,
    };

    struct FakeRunner;
    #[async_trait]
    impl ProbeRunner for FakeRunner {
        async fn run(&self, _program: &str, _args: &[&str]) -> Result<String, String> {
            Ok("ok".into())
        }
    }

    fn project(hostname: Option<&str>) -> Project {
        Project {
            config: ProjectConfig {
                name: "app".into(),
                repo: "r".into(),
                branch: "main".into(),
                compose_path: "docker-compose.yml".into(),
                service: "web".into(),
                container_port: 80,
                hostname: hostname.map(String::from),
                expose: ExposeMode::default(),
                healthcheck: HealthcheckConfig::default(),
                timeouts: StageTimeoutOverrides::default(),
                commands: Default::default(),
                command_timeout_secs: None,
            },
            host_port: 8002,
            created_at: 1,
        }
    }

    fn probe(ingress_active: bool, projects: Vec<Project>) -> Arc<HostSystemProbe> {
        let mut repo = MockProjectRepository::new();
        repo.expect_list().return_once(move || Ok(projects));
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(10));
        HostSystemProbe::new(
            Arc::new(FakeRunner),
            Arc::new(disk),
            Arc::new(repo),
            "0.0.0".into(),
            85,
            false, // cloudflared binary/service checks off — not under test
            ingress_active,
            0,
        )
    }

    #[tokio::test]
    async fn disabled_ingress_with_hostnames_fails_the_ingress_check() {
        let report = probe(false, vec![project(Some("rpi.example.com"))])
            .diagnostics()
            .await;
        let check = report
            .checks
            .iter()
            .find(|c| c.name == "ingress routing")
            .expect("ingress routing check present");
        assert!(!check.passed);
        assert!(check.detail.contains("rpi.example.com"));
        assert!(check
            .hint
            .as_deref()
            .unwrap_or_default()
            .contains("sudo rpi agent setup --with-cloudflared"));
    }

    #[tokio::test]
    async fn active_ingress_or_no_hostnames_add_no_check() {
        for (active, host) in [(true, Some("a.example.com")), (false, None)] {
            let report = probe(active, vec![project(host)]).diagnostics().await;
            assert!(
                report.checks.iter().all(|c| c.name != "ingress routing"),
                "unexpected check for active={active} host={host:?}"
            );
        }
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p pi-infrastructure probe`
Expected: COMPILE ERROR — `HostSystemProbe::new` takes 7 args, tests pass 8 (`ingress_active` field missing).

- [ ] **Step 3: Implement.** In `probe.rs`: add field `ingress_active: bool` to `HostSystemProbe` (after `cloudflared_enabled`), add the constructor parameter in the same position, store it. In `diagnostics()`, insert between the `if self.cloudflared_enabled { … }` block and the disk-space check:

```rust
        if !self.ingress_active {
            if let Ok(projects) = self.projects.list().await {
                let hostnames: Vec<String> = projects
                    .iter()
                    .filter_map(|p| p.config.hostname.clone())
                    .collect();
                if !hostnames.is_empty() {
                    checks.push(DiagnosticCheck {
                        name: "ingress routing".into(),
                        passed: false,
                        detail: format!(
                            "hostname(s) declared but automatic ingress is disabled: {}",
                            hostnames.join(", ")
                        ),
                        hint: Some(
                            "enable it: sudo rpi agent setup --with-cloudflared \
                             --cf-token <token> --domain <zone>"
                                .into(),
                        ),
                    });
                }
            }
        }
```

In `state.rs`, thread the flag out of the existing ingress match with minimal diff:

```rust
    let mut ingress_active = false;
    let ingress: Arc<dyn Ingress> = match (&config.cloudflared, &config.cloudflare) {
```

…and in the one arm that builds `CloudflaredIngress` (token read OK), set `ingress_active = true;` immediately before the `CloudflaredIngress::new(…)` expression. All `DisabledIngress` arms stay as they are. Then pass it to the probe (line ~164):

```rust
    let probe = HostSystemProbe::new(
        Arc::new(SystemRunner),
        disk,
        projects.clone(),
        env!("CARGO_PKG_VERSION").to_string(),
        config.gc.disk_threshold_percent,
        config.cloudflared.is_some(),
        ingress_active,
        now,
    );
```

In `http.rs` test call site (~818): add `false,` between the existing `false,` and `100,`.

- [ ] **Step 4: Run tests**

Run: `rtk cargo test --locked`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
rtk cargo fmt --all
rtk cargo clippy --all-targets --locked -- -D warnings
rtk git add crates/infrastructure/src/probe.rs crates/bin/src/agent/state.rs crates/bin/src/agent/http.rs
rtk git commit -m "feat(doctor): flag projects with hostnames while ingress is disabled

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: inject `XDG_RUNTIME_DIR` into the cloudflared restart command

**Files:**
- Modify: `crates/infrastructure/Cargo.toml` (unix-only libc dep) + `Cargo.lock`
- Modify: `crates/infrastructure/src/cloudflared.rs` (helpers + both restart spawn sites: `route_dns_and_restart` ~220-228 and `remove` ~185-199, plus tests)

**Interfaces:**
- Consumes: nothing new from other tasks.
- Produces: `fn restart_extra_env(current: Option<&str>, uid: u32) -> Option<(&'static str, String)>` (pure, unit-tested) and `fn apply_restart_env(cmd: &mut Command)` called before every restart spawn. Behavior: adds `XDG_RUNTIME_DIR=/run/user/<uid>` only when the agent's own environment lacks the variable.

- [ ] **Step 1: Write the failing test** in the cloudflared.rs tests module:

```rust
#[test]
fn restart_env_added_only_when_missing() {
    assert_eq!(
        restart_extra_env(None, 999),
        Some(("XDG_RUNTIME_DIR", "/run/user/999".into()))
    );
    assert_eq!(restart_extra_env(Some("/run/user/1000"), 999), None);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test -p pi-infrastructure restart_env`
Expected: COMPILE ERROR — `restart_extra_env` not defined.

- [ ] **Step 3: Implement.** In `crates/infrastructure/Cargo.toml` append:

```toml
[target.'cfg(unix)'.dependencies]
libc = "0.2"
```

In `cloudflared.rs`, above `CloudflaredIngress`:

```rust
/// `systemctl --user` needs XDG_RUNTIME_DIR to reach the user manager; the
/// rpi-agent unit does not set it. Compute the variable to add to the restart
/// command when the agent's own environment lacks it.
fn restart_extra_env(current: Option<&str>, uid: u32) -> Option<(&'static str, String)> {
    match current {
        Some(_) => None,
        None => Some(("XDG_RUNTIME_DIR", format!("/run/user/{uid}"))),
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

fn apply_restart_env(cmd: &mut Command) {
    let current = std::env::var("XDG_RUNTIME_DIR").ok();
    if let Some((k, v)) = restart_extra_env(current.as_deref(), current_uid()) {
        cmd.env(k, v);
    }
}
```

At BOTH restart spawn sites — in `remove` (after `restart_cmd.args(args);`, ~line 190) and in `route_dns_and_restart` (after `restart_cmd.args(args);`, ~line 225) — add:

```rust
        apply_restart_env(&mut restart_cmd);
```

Refresh the lock file: `rtk cargo build -p pi-infrastructure`.

- [ ] **Step 4: Run tests**

Run: `rtk cargo test --locked`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit (lock file included)**

```bash
rtk cargo fmt --all
rtk cargo clippy --all-targets --locked -- -D warnings
rtk git add crates/infrastructure/Cargo.toml Cargo.lock crates/infrastructure/src/cloudflared.rs
rtk git commit -m "fix(ingress): set XDG_RUNTIME_DIR for the cloudflared restart command

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: adoption path in `cloudflared_bootstrap_full`

**Files:**
- Modify: `crates/bin/Cargo.toml` (add serde_yaml) + `Cargo.lock`
- Modify: `crates/bin/src/agent/setup.rs` (helpers near `render_config_yml` ~552, branch in `cloudflared_bootstrap_full` ~573-585, tests module)

**Interfaces:**
- Consumes: `Sys` (read/write/exists/run), `CloudflareApi::find_or_create_tunnel`, `SetupReport` (has `created/skipped/repaired/warnings/errors`), constants `CLOUDFLARED_CONFIG_PATH`, `AGENT_TOML_PATH`, existing `upsert_cloudflared_agent_toml(sys, tunnel_id, zone, rep)`.
- Produces: `pub(crate) fn looks_like_tunnel_id(s: &str) -> bool`; `pub(crate) fn parse_existing_config(text: &str) -> Result<ExistingTunnelConfig, String>` with `pub(crate) struct ExistingTunnelConfig { pub tunnel: String, pub credentials_file: Option<String> }`; `async fn adopt_existing_cloudflared(sys, cf, opts, rep)`. Behavior per spec §3: existing config.yml → adopt (no writes to it, no restart); unparseable → error + zero writes; absent → unchanged fresh path. NOTE: the outer `cloudflared_bootstrap` wrapper already writes the token before this function and scaffolds/enables the user unit after it (`systemctl --user enable --now` does not restart an active unit) — do not duplicate either here.

- [ ] **Step 1: Add serde_yaml to the bin crate.** In `crates/bin/Cargo.toml` `[dependencies]`, after `toml = { workspace = true }`:

```toml
serde_yaml = { workspace = true }
```

Run: `rtk cargo build -p pi` (refreshes Cargo.lock).
Expected: builds.

- [ ] **Step 2: Write the failing tests** in the setup.rs tests module (they reuse `fresh_sys()`, `FakeSys`, `SetupReport`, `CloudflaredBootstrap`):

```rust
const MANUAL_CONFIG_UUID: &str = "tunnel: bc15c4e2-1111-2222-3333-444455556666\n\
credentials-file: /var/lib/rpi/cloudflared/creds.json\n\
ingress:\n  - hostname: board.example.com\n    service: http://127.0.0.1:8001\n  - service: http_status:404\n";

fn adoption_sys(config: &str) -> FakeSys {
    let mut sys = fresh_sys();
    // binary already installed -> ensure_cloudflared_binary skips
    sys.ok
        .insert(FakeSys::key("cloudflared", &["--version"]), "ok".into());
    sys.paths.insert(CLOUDFLARED_CONFIG_PATH.into());
    sys.files.insert(CLOUDFLARED_CONFIG_PATH.into(), config.into());
    sys
}

fn adoption_opts() -> CloudflaredBootstrap {
    CloudflaredBootstrap {
        tunnel_name: "ignored-on-adoption".into(),
        zone: "example.com".into(),
    }
}

#[tokio::test]
async fn adoption_never_rewrites_config_and_uses_uuid_without_tunnel_api() {
    use pi_domain::contracts::MockCloudflareApi;
    let mut sys = adoption_sys(MANUAL_CONFIG_UUID);
    sys.paths.insert("/var/lib/rpi/cloudflared/creds.json".into());
    let cf = MockCloudflareApi::new(); // no expectations: any tunnel-API call panics
    let mut rep = SetupReport::default();
    cloudflared_bootstrap_full(&sys, &cf, &adoption_opts(), false, &mut rep).await;

    let writes = sys.writes.lock().unwrap();
    assert!(
        writes.iter().all(|(p, _)| p != CLOUDFLARED_CONFIG_PATH),
        "config.yml must never be rewritten on adoption: {writes:?}"
    );
    let (_, toml) = writes
        .iter()
        .find(|(p, _)| p == AGENT_TOML_PATH)
        .expect("agent.toml sections written");
    assert!(toml.contains("tunnel_id = \"bc15c4e2-1111-2222-3333-444455556666\""));
    drop(writes);
    assert!(sys.calls().iter().any(|c| c.contains("ingress validate")));
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
}

#[tokio::test]
async fn adoption_resolves_tunnel_name_via_api() {
    use pi_domain::contracts::{MockCloudflareApi, TunnelCreds};
    let mut sys = adoption_sys(
        "tunnel: myboard\ncredentials-file: /var/lib/rpi/cloudflared/creds.json\ningress:\n  - service: http_status:404\n",
    );
    sys.paths.insert("/var/lib/rpi/cloudflared/creds.json".into());
    let mut cf = MockCloudflareApi::new();
    cf.expect_find_or_create_tunnel()
        .withf(|name| name == "myboard")
        .returning(|name| {
            Ok(TunnelCreds {
                account_tag: "acc".into(),
                tunnel_id: "resolved-tid".into(),
                tunnel_name: name.to_string(),
                tunnel_secret: String::new(), // adopted
            })
        });
    let mut rep = SetupReport::default();
    cloudflared_bootstrap_full(&sys, &cf, &adoption_opts(), false, &mut rep).await;
    let writes = sys.writes.lock().unwrap();
    let (_, toml) = writes
        .iter()
        .find(|(p, _)| p == AGENT_TOML_PATH)
        .expect("agent.toml written");
    assert!(toml.contains("tunnel_id = \"resolved-tid\""));
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
}

#[tokio::test]
async fn adoption_missing_credentials_is_an_error_with_zero_writes() {
    use pi_domain::contracts::MockCloudflareApi;
    let sys = adoption_sys(MANUAL_CONFIG_UUID); // creds path NOT in sys.paths
    let cf = MockCloudflareApi::new();
    let mut rep = SetupReport::default();
    cloudflared_bootstrap_full(&sys, &cf, &adoption_opts(), false, &mut rep).await;
    assert!(
        rep.errors.iter().any(|e| e.contains("creds.json")),
        "{:?}",
        rep.errors
    );
    assert!(sys.writes.lock().unwrap().is_empty(), "zero writes on error");
}

#[tokio::test]
async fn adoption_unparseable_config_is_an_error_with_zero_writes() {
    use pi_domain::contracts::MockCloudflareApi;
    let sys = adoption_sys("- just\n- a list\n"); // no `tunnel:` mapping
    let cf = MockCloudflareApi::new();
    let mut rep = SetupReport::default();
    cloudflared_bootstrap_full(&sys, &cf, &adoption_opts(), false, &mut rep).await;
    assert!(
        rep.errors
            .iter()
            .any(|e| e.contains("move it aside") || e.contains("not a usable")),
        "{:?}",
        rep.errors
    );
    assert!(sys.writes.lock().unwrap().is_empty(), "zero writes on error");
}

#[tokio::test]
async fn adoption_validate_failure_is_a_warning_not_an_error() {
    use pi_domain::contracts::MockCloudflareApi;
    let mut sys = adoption_sys(MANUAL_CONFIG_UUID);
    sys.paths.insert("/var/lib/rpi/cloudflared/creds.json".into());
    sys.err.insert(FakeSys::key(
        "cloudflared",
        &["tunnel", "--config", CLOUDFLARED_CONFIG_PATH, "ingress", "validate"],
    ));
    let cf = MockCloudflareApi::new();
    let mut rep = SetupReport::default();
    cloudflared_bootstrap_full(&sys, &cf, &adoption_opts(), false, &mut rep).await;
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert!(!rep.warnings.is_empty(), "validate failure surfaces as warning");
    assert!(
        sys.writes.lock().unwrap().iter().any(|(p, _)| p == AGENT_TOML_PATH),
        "agent.toml still written"
    );
}

#[tokio::test]
async fn adoption_default_creds_path_is_derived_from_tunnel_id() {
    use pi_domain::contracts::MockCloudflareApi;
    let mut sys = adoption_sys(
        "tunnel: bc15c4e2-1111-2222-3333-444455556666\ningress:\n  - service: http_status:404\n",
    );
    sys.paths
        .insert("/var/lib/rpi/cloudflared/bc15c4e2-1111-2222-3333-444455556666.json".into());
    let cf = MockCloudflareApi::new();
    let mut rep = SetupReport::default();
    cloudflared_bootstrap_full(&sys, &cf, &adoption_opts(), false, &mut rep).await;
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
}

#[tokio::test]
async fn adoption_dry_run_reports_would_adopt_and_writes_nothing() {
    use pi_domain::contracts::MockCloudflareApi;
    let sys = adoption_sys(MANUAL_CONFIG_UUID);
    let cf = MockCloudflareApi::new();
    let mut rep = SetupReport::default();
    cloudflared_bootstrap_full(&sys, &cf, &adoption_opts(), true, &mut rep).await;
    assert!(sys.writes.lock().unwrap().is_empty());
    assert!(
        rep.skipped.iter().any(|s| s.contains("would adopt")),
        "{:?}",
        rep.skipped
    );
}

#[tokio::test]
async fn repeated_adoption_is_a_pure_skip() {
    use pi_domain::contracts::MockCloudflareApi;
    let mut sys = adoption_sys(MANUAL_CONFIG_UUID);
    sys.paths.insert("/var/lib/rpi/cloudflared/creds.json".into());
    // agent.toml already carries the sections from the first run
    sys.files.insert(
        AGENT_TOML_PATH.into(),
        "[cloudflared]\nconfig = \"/var/lib/rpi/cloudflared/config.yml\"\n".into(),
    );
    let cf = MockCloudflareApi::new();
    let mut rep = SetupReport::default();
    cloudflared_bootstrap_full(&sys, &cf, &adoption_opts(), false, &mut rep).await;
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert!(
        sys.writes.lock().unwrap().is_empty(),
        "second run must not write anything"
    );
    assert!(
        rep.skipped.iter().any(|s| s.contains("agent.toml")),
        "{:?}",
        rep.skipped
    );
}

#[test]
fn tunnel_id_shape_detection() {
    assert!(looks_like_tunnel_id("bc15c4e2-1111-2222-3333-444455556666"));
    assert!(!looks_like_tunnel_id("myboard"));
    assert!(!looks_like_tunnel_id("bc15c4e2-1111-2222-3333-44445555666")); // 35 chars
    assert!(!looks_like_tunnel_id("gc15c4e2-1111-2222-3333-444455556666")); // non-hex
}
```

- [ ] **Step 3: Run to verify failure**

Run: `rtk cargo test -p pi adoption`
Expected: COMPILE ERROR — `looks_like_tunnel_id` / adoption branch not defined.

- [ ] **Step 4: Implement.** In `setup.rs`, next to `render_config_yml` (~552):

```rust
/// Existing locally-managed config.yml, as found on a host where the tunnel
/// was built by hand (adoption spec §3.1).
pub(crate) struct ExistingTunnelConfig {
    pub tunnel: String,
    pub credentials_file: Option<String>,
}

/// A 36-char hyphenated hex UUID — the shape of a cloudflared tunnel id.
pub(crate) fn looks_like_tunnel_id(s: &str) -> bool {
    s.len() == 36
        && s.chars().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

pub(crate) fn parse_existing_config(text: &str) -> Result<ExistingTunnelConfig, String> {
    let doc: serde_yaml::Value = serde_yaml::from_str(text).map_err(|e| format!("yaml: {e}"))?;
    if !doc.is_mapping() {
        return Err("top level must be a mapping".into());
    }
    let tunnel = doc
        .get("tunnel")
        .and_then(|v| v.as_str())
        .ok_or("missing `tunnel:` key")?
        .to_string();
    let credentials_file = doc
        .get("credentials-file")
        .and_then(|v| v.as_str())
        .map(String::from);
    Ok(ExistingTunnelConfig {
        tunnel,
        credentials_file,
    })
}

/// §3.1: a host with an existing config.yml is adopted, never rewritten —
/// the running tunnel and its hand-written routes stay untouched; deploys
/// take over route management from here.
async fn adopt_existing_cloudflared(
    sys: &dyn Sys,
    cf: &dyn CloudflareApi,
    opts: &CloudflaredBootstrap,
    rep: &mut SetupReport,
) {
    let Some(text) = sys.read(Path::new(CLOUDFLARED_CONFIG_PATH)) else {
        rep.errors
            .push(format!("cannot read {CLOUDFLARED_CONFIG_PATH}"));
        return;
    };
    let parsed = match parse_existing_config(&text) {
        Ok(p) => p,
        Err(e) => {
            rep.errors.push(format!(
                "{CLOUDFLARED_CONFIG_PATH} exists but is not a usable cloudflared config ({e}); \
                 fix it or move it aside and re-run setup"
            ));
            return;
        }
    };
    let tunnel_id = if looks_like_tunnel_id(&parsed.tunnel) {
        parsed.tunnel.clone()
    } else {
        match cf.find_or_create_tunnel(&parsed.tunnel).await {
            Ok(creds) => creds.tunnel_id,
            Err(e) => {
                rep.errors
                    .push(format!("resolve tunnel '{}': {e}", parsed.tunnel));
                return;
            }
        }
    };
    let creds_path = parsed
        .credentials_file
        .clone()
        .unwrap_or_else(|| format!("/var/lib/rpi/cloudflared/{tunnel_id}.json"));
    if !sys.exists(Path::new(&creds_path)) {
        rep.errors.push(format!(
            "{CLOUDFLARED_CONFIG_PATH} references credentials {creds_path}, which does not \
             exist; restore the credentials JSON and re-run setup"
        ));
        return;
    }
    match sys
        .run(
            "cloudflared",
            &[
                "tunnel",
                "--config",
                CLOUDFLARED_CONFIG_PATH,
                "ingress",
                "validate",
            ],
        )
        .await
    {
        Ok(_) => rep
            .skipped
            .push(format!("{CLOUDFLARED_CONFIG_PATH} (adopted, left untouched)")),
        Err(e) => rep.warnings.push(format!(
            "adopted {CLOUDFLARED_CONFIG_PATH}, but `cloudflared tunnel ingress validate` \
             failed: {e}"
        )),
    }
    upsert_cloudflared_agent_toml(sys, &tunnel_id, &opts.zone, rep);
}
```

In `cloudflared_bootstrap_full`, insert the branch between `ensure_cloudflared_binary(...)` and the existing `if dry { … }` block:

```rust
    ensure_cloudflared_binary(sys, dry, rep).await;

    // Adoption (§3.1): an existing config.yml is never rewritten.
    if sys.exists(Path::new(CLOUDFLARED_CONFIG_PATH)) {
        if dry {
            rep.skipped.push(format!(
                "{CLOUDFLARED_CONFIG_PATH} exists — would adopt (dry run)"
            ));
            return;
        }
        adopt_existing_cloudflared(sys, cf, opts, rep).await;
        return;
    }

    if dry {
        rep.created.push("cloudflared tunnel (dry run)".into());
        return;
    }
```

(`use pi_domain::contracts::CloudflareApi;` is already imported in setup.rs for `cloudflared_bootstrap_full` — reuse it.)

- [ ] **Step 5: Run tests**

Run: `rtk cargo test -p pi` then `rtk cargo test --locked`
Expected: PASS, including all pre-existing bootstrap tests (`fresh_sys()` has no config.yml, so they exercise the unchanged fresh path).

- [ ] **Step 6: fmt + clippy + commit**

```bash
rtk cargo fmt --all
rtk cargo clippy --all-targets --locked -- -D warnings
rtk git add crates/bin/Cargo.toml Cargo.lock crates/bin/src/agent/setup.rs
rtk git commit -m "feat(setup): adopt an existing cloudflared install without rewriting config.yml

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: README + spec sync

**Files:**
- Modify: `README.md` (section «Cloudflare Tunnel»)
- Modify: `docs/superpowers/specs/2026-07-09-rpi-cloudflared-adoption-design.md` (§5 — mechanics drifted during implementation)

**Interfaces:** none (docs only).

- [ ] **Step 1: README.** Locate the "Cloudflare Tunnel" section (search for `Cloudflare Tunnel`). Add a subsection after the full-bootstrap description:

```markdown
### Adopting an existing tunnel

If `/var/lib/rpi/cloudflared/config.yml` already exists (a hand-built tunnel),
`rpi agent setup --with-cloudflared --cf-token <token> --domain <zone>` adopts it
instead of recreating anything:

- the existing `config.yml` is **never rewritten** and cloudflared is **not
  restarted** — hand-written routes and uptime are preserved;
- the tunnel id is taken from the `tunnel:` key (a name is resolved via the
  Cloudflare API); credentials are checked at the `credentials-file:` path;
- `[cloudflare]`/`[cloudflared]` are appended to `/etc/rpi/agent.toml`, after
  which every deploy manages routes and DNS by itself.

Token scopes: `Zone:Zone:Read` + `Zone:DNS:Edit` are enough when `tunnel:` holds
the tunnel id (UUID). `Account:Cloudflare Tunnel:Edit` is additionally needed for
fresh installs and for adoption when `tunnel:` holds a name.

If a project declares `[ingress] hostname` while the agent has no ingress
configured, the deploy summary and `rpi doctor` now say so explicitly.
```

Also search README for manual `XDG_RUNTIME_DIR` workarounds around cloudflared restarts (`rg -n "XDG_RUNTIME_DIR" README.md`); if the manual-restart instructions tell the operator to prefix `env XDG_RUNTIME_DIR=…` for the agent's own restarts, note that the agent now sets it automatically (leave instructions for humans running `systemctl --user` from a shell untouched — those still need it).

- [ ] **Step 2: Spec sync.** In `docs/superpowers/specs/2026-07-09-rpi-cloudflared-adoption-design.md` §5 replace the two bullets with the as-built mechanics:

Replace the deploy bullet's DTO sentence — from:

```
`DeployProject` превращает `Skipped` при объявленном hostname в warning
  деплой-результата; итоговый DTO получает `#[serde(default)] warnings: Vec<String>`
  (старый агент поля не шлёт — CLI молчит, обратная совместимость). CLI печатает
  warnings в итоговой сводке жёлтым, с командой включения:
```

to:

```
`DeployProject` при `Skipped` пишет `warning: …`-строку последней в деплой-лог
  (SSE-событие `finished` несёт только статус-строку, менять его формат
  несовместимо); CLI собирает строки с префиксом `warning: ` из стрима и
  повторяет их через `output::warn` рядом с итоговой сводкой. Старый CLI просто
  видит строку в конце лога, старый агент warning-строк не шлёт — совместимость
  в обе стороны. Команда включения в тексте warning'а:
```

Replace the doctor bullet — from:

```
- **`rpi doctor`.** Агентский status-DTO получает `#[serde(default)]` поле
  `ingress: "cloudflared" | "disabled"` (значение фиксируется при сборке ingress в
  `agent/state.rs`). Doctor добавляет чек: есть зарегистрированные проекты с
  `hostname` при `ingress = "disabled"` → warn со списком hostname'ов и той же
  командой включения. Старый агент без поля → чек скипается молча.
```

to:

```
- **`rpi doctor`.** Диагностика выполняется на агенте (`/v1/doctor` →
  `RunDiagnostics` → `HostSystemProbe`), поэтому wire менять не нужно:
  `HostSystemProbe` получает флаг `ingress_active` (вычисляется при сборке
  ingress в `agent/state.rs`) и добавляет провальный чек `ingress routing`,
  когда есть зарегистрированные проекты с `hostname` при выключенном ingress —
  detail перечисляет hostname'ы, hint содержит команду включения. Generic
  `DiagnosticCheckDto` совместим со старым CLI.
```

Also in §6 (tests) replace the bullet

```
- **`deploy.rs`/`proto.rs`:** `Skipped` + hostname → warning в результате; roundtrip
  warnings через DTO; payload без поля → пусто. `Applied` → без warning.
```

with

```
- **`deploy.rs`/`commands.rs`:** `Skipped` + hostname → `warning: …` последней
  строкой `log_tail`; `Applied` → без warning. В CLI — юнит-тест выделения
  `warning: `-префикса (`deploy_warning`). proto.rs не меняется.
```

- [ ] **Step 3: Commit**

```bash
rtk git add README.md docs/superpowers/specs/2026-07-09-rpi-cloudflared-adoption-design.md
rtk git commit -m "docs: adoption of existing tunnels, loud manual-ingress signals

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 8: final verification

**Files:** none (verification only).

- [ ] **Step 1: Full gate (CLAUDE.md)**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
```

Expected: all three clean. If fmt reports a diff: `rtk cargo fmt --all`, re-run, commit the formatting.

- [ ] **Step 2: Manual integration checklist (reference — runs on the real Pi after release, spec §6/§8; not executable from this repo):**

1. Release + update the `rpi` binary on the Pi.
2. `sha256sum /var/lib/rpi/cloudflared/config.yml` (before), then `sudo rpi agent setup --with-cloudflared --cf-token <tok> --domain iiskelo.com`; re-hash — must be identical; `board.iiskelo.com` must answer throughout; `agent.toml` gains `[cloudflare]`/`[cloudflared]` with the real tunnel UUID.
3. `sudo systemctl restart rpi-agent`, then `rpi deploy` from `rpi-deploy-site`: route appears in config.yml, CNAME `rpi.iiskelo.com` exists, `https://rpi.iiskelo.com` answers.
4. Re-run setup: pure skip-report. `rpi doctor` before enabling showed the failing `ingress routing` check; after — no such check.
