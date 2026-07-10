# cloudflared setup safety hardening — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the residual "tool silently overwrites/disrupts an existing cloudflared tunnel" failure class plus two adjacent robustness/security fixes surfaced by the same review.

**Architecture:** Four independent parts on different files/seams — (A) a foreign-tunnel guard on setup's fresh-install path, (B) leak-free API-token input via file/stdin/env, (C) two new `rpi doctor` half-state checks, (D) DBUS in the user-scoped `systemctl` env. No wire/protocol/schema changes; no new crate dependency.

**Tech Stack:** Rust, async-trait, tokio, serde_yaml (already a dep of both crates touched), mockall/FakeSys/FakeRunner test doubles.

**Source spec:** `docs/superpowers/specs/2026-07-10-rpi-cloudflared-setup-hardening-design.md`

## Global Constraints

- No new crate dependency — `serde_yaml` is already a workspace dep of `pi` (bin) and `pi-infrastructure`; use it, add nothing.
- No wire/protocol/schema changes.
- Never hardcode the uid — always compute it at runtime (`current_uid()` = `libc::getuid()` in infra; `id -u rpi-agent` in setup).
- Parts A–D are independent (different files/functions) and can be reviewed one at a time; keep each task's changes self-contained.
- CI parity — before a task is "done" its code must pass, on Linux:
  - `rtk cargo fmt --all -- --check`
  - `rtk cargo clippy --all-targets --locked -- -D warnings`
  - `rtk cargo test --locked`
  - If `cargo fmt --all -- --check` reports a diff, run `rtk cargo fmt --all` and commit the result — do not hand-edit formatting.
- Crate names for `-p`: bin crate is `pi`, infra crate is `pi-infrastructure`.

---

## File Structure

- `crates/bin/src/agent/setup.rs` — A (foreign-tunnel guard + `cloudflared_running`), B (token resolution + `run_cmd` wiring), D (enable-path DBUS). Also its `#[cfg(test)] mod fake` FakeSys helper (A) and `#[cfg(test)] mod tests` (A/B/D).
- `crates/bin/src/main.rs` — B (`--cf-token-file` arg + `--cf-token` help + call site + parse test).
- `crates/infrastructure/src/cloudflared.rs` — D (`restart_extra_env` → `Vec` + DBUS, `apply_restart_env` loop, unit-test update).
- `crates/infrastructure/src/probe.rs` — C (connector + route checks, `cloudflared_config` field/param, tests).
- `crates/bin/src/agent/state.rs` — C (pass `cloudflared_config` at the real probe call site).
- `crates/bin/src/agent/http.rs` — C (pass `None` at the test probe call site).

---

## Task 1 (Part A): Foreign-tunnel guard

Refuse setup's fresh-install path when a cloudflared the tool didn't create is already running. The fresh branch runs only when `config.yml` is absent, so any cloudflared running at that moment is foreign — no ownership bookkeeping needed.

**Files:**
- Modify: `crates/bin/src/agent/setup.rs` — add `CloudflaredState` + `cloudflared_running` before `cloudflared_bootstrap_full` (~684); add the guard inside `cloudflared_bootstrap_full` between the adoption `if` and the `if dry` (~703–705); extend `FakeSys` (`mod fake`, ~1167) with `err_msg`; seed `pgrep` in `fresh_sys()` (~1283); add tests in `mod tests`.

**Interfaces:**
- Produces: `enum CloudflaredState { Running, NotRunning, Undetermined }` and `async fn cloudflared_running(sys: &dyn Sys) -> CloudflaredState` (module-private, used only in setup.rs + its tests). `FakeSys` gains `pub err_msg: HashMap<String, String>` (key → custom error string, checked before `err`).
- Consumes: existing `Sys::run` contract — `Ok(stdout)` on exit 0, `Err(stderr)` on non-zero, `Err("spawn …")` when the program can't launch.

- [ ] **Step 1: Extend `FakeSys` with custom error strings**

The Undetermined unit test needs a `pgrep` error that starts with `"spawn "`, which the current `err` set can't produce (it yields `"fake error: …"`). Add an `err_msg` map checked first.

In `crates/bin/src/agent/setup.rs`, `mod fake`, add the field to the struct:

```rust
    #[derive(Default)]
    pub struct FakeSys {
        pub paths: HashSet<String>,
        pub files: HashMap<String, String>,
        pub ok: HashMap<String, String>, // "program a b" -> stdout
        pub err: HashSet<String>,        // "program a b" that fail with a generic error
        pub err_msg: HashMap<String, String>, // "program a b" -> exact error string (e.g. "spawn …")
        pub calls: Mutex<Vec<String>>,
        pub writes: Mutex<Vec<(String, String)>>,
    }
```

And check it first in `run`:

```rust
        async fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
            let k = FakeSys::key(program, args);
            self.calls.lock().unwrap().push(k.clone());
            if let Some(msg) = self.err_msg.get(&k) {
                return Err(msg.clone());
            }
            if self.err.contains(&k) {
                return Err(format!("fake error: {k}"));
            }
            Ok(self.ok.get(&k).cloned().unwrap_or_default())
        }
```

- [ ] **Step 2: Write the failing `cloudflared_running` unit test**

In `crates/bin/src/agent/setup.rs`, `mod tests`, add:

```rust
    #[tokio::test]
    async fn cloudflared_running_maps_run_outcomes() {
        // Ok(_) -> a cloudflared process exists
        let mut sys = FakeSys::default();
        sys.ok
            .insert(FakeSys::key("pgrep", &["-x", "cloudflared"]), "4321".into());
        assert_eq!(cloudflared_running(&sys).await, CloudflaredState::Running);

        // Err(non-spawn) -> pgrep ran, no match
        let mut sys = FakeSys::default();
        sys.err.insert(FakeSys::key("pgrep", &["-x", "cloudflared"]));
        assert_eq!(cloudflared_running(&sys).await, CloudflaredState::NotRunning);

        // Err("spawn …") -> pgrep not installed
        let mut sys = FakeSys::default();
        sys.err_msg.insert(
            FakeSys::key("pgrep", &["-x", "cloudflared"]),
            "spawn pgrep: No such file or directory".into(),
        );
        assert_eq!(
            cloudflared_running(&sys).await,
            CloudflaredState::Undetermined
        );
    }
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `rtk cargo test -p pi cloudflared_running_maps_run_outcomes`
Expected: FAIL — `cannot find … CloudflaredState` / `cloudflared_running` not defined.

- [ ] **Step 4: Implement `CloudflaredState` + `cloudflared_running`**

In `crates/bin/src/agent/setup.rs`, immediately before `pub(crate) async fn cloudflared_bootstrap_full` (~line 684):

```rust
/// Whether a cloudflared process is currently running, for the foreign-tunnel
/// guard (Part A). Detects a *process* (`pgrep -x cloudflared`), not the unit
/// file: the no-token fallback scaffolds the unit without starting it, so a
/// unit-file check would false-positive on that legitimate workflow.
#[derive(Debug, PartialEq, Eq)]
enum CloudflaredState {
    Running,
    NotRunning,
    Undetermined,
}

async fn cloudflared_running(sys: &dyn Sys) -> CloudflaredState {
    match sys.run("pgrep", &["-x", "cloudflared"]).await {
        Ok(_) => CloudflaredState::Running,
        Err(e) if e.starts_with("spawn ") => CloudflaredState::Undetermined,
        Err(_) => CloudflaredState::NotRunning,
    }
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `rtk cargo test -p pi cloudflared_running_maps_run_outcomes`
Expected: PASS.

- [ ] **Step 6: Seed `pgrep` = NotRunning in `fresh_sys()`**

The guard (Step 8) makes every fresh-install test refuse unless `pgrep` reports NotRunning, because default `FakeSys` returns `Ok("")` for unknown commands (→ `Running`). Make the clean-host fixture explicit.

In `fresh_sys()` (~1283), after the docker inserts and before `sys` is returned:

```rust
        // Clean host: no foreign cloudflared running (Part A guard sees NotRunning).
        sys.err
            .insert(FakeSys::key("pgrep", &["-x", "cloudflared"]));
        sys
```

- [ ] **Step 7: Write the failing guard tests**

In `crates/bin/src/agent/setup.rs`, `mod tests`, add:

```rust
    #[tokio::test]
    async fn bootstrap_refuses_when_foreign_cloudflared_running() {
        use pi_domain::contracts::MockCloudflareApi;
        let mut sys = fresh_sys();
        // cloudflared installed (binary step skips) and running (foreign).
        sys.ok
            .insert(FakeSys::key("cloudflared", &["--version"]), "2024.1.0".into());
        sys.err
            .remove(&FakeSys::key("pgrep", &["-x", "cloudflared"]));
        sys.ok
            .insert(FakeSys::key("pgrep", &["-x", "cloudflared"]), "4321".into());
        let cf = MockCloudflareApi::new(); // no expectations: tunnel API must not be called
        let mut rep = SetupReport::default();
        let opts = CloudflaredBootstrap {
            tunnel_name: "myboard".into(),
            zone: "example.com".into(),
        };
        let adopted = cloudflared_bootstrap_full(&sys, &cf, &opts, false, &mut rep).await;
        assert!(!adopted);
        assert!(
            rep.errors
                .iter()
                .any(|e| e.contains("already running") && e.contains("refusing")),
            "{:?}",
            rep.errors
        );
        assert!(
            sys.writes.lock().unwrap().is_empty(),
            "guard must write nothing"
        );
    }

    #[tokio::test]
    async fn bootstrap_dry_run_refuses_when_foreign_cloudflared_running() {
        use pi_domain::contracts::MockCloudflareApi;
        let mut sys = fresh_sys();
        sys.err
            .remove(&FakeSys::key("pgrep", &["-x", "cloudflared"]));
        sys.ok
            .insert(FakeSys::key("pgrep", &["-x", "cloudflared"]), "4321".into());
        let cf = MockCloudflareApi::new();
        let mut rep = SetupReport::default();
        let opts = CloudflaredBootstrap {
            tunnel_name: "myboard".into(),
            zone: "example.com".into(),
        };
        let adopted = cloudflared_bootstrap_full(&sys, &cf, &opts, true, &mut rep).await;
        assert!(!adopted);
        assert!(
            rep.skipped.iter().any(|s| s.starts_with("would refuse:")),
            "{:?}",
            rep.skipped
        );
        assert!(rep.errors.is_empty());
        assert!(sys.writes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn bootstrap_refuses_when_pgrep_unavailable() {
        use pi_domain::contracts::MockCloudflareApi;
        let mut sys = fresh_sys();
        sys.err
            .remove(&FakeSys::key("pgrep", &["-x", "cloudflared"]));
        sys.err_msg.insert(
            FakeSys::key("pgrep", &["-x", "cloudflared"]),
            "spawn pgrep: No such file or directory".into(),
        );
        let cf = MockCloudflareApi::new();
        let mut rep = SetupReport::default();
        let opts = CloudflaredBootstrap {
            tunnel_name: "myboard".into(),
            zone: "example.com".into(),
        };
        let adopted = cloudflared_bootstrap_full(&sys, &cf, &opts, false, &mut rep).await;
        assert!(!adopted);
        assert!(
            rep.errors.iter().any(|e| e.contains("pgrep unavailable")),
            "{:?}",
            rep.errors
        );
        assert!(sys.writes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn adoption_runs_even_with_a_cloudflared_already_running() {
        use pi_domain::contracts::MockCloudflareApi;
        let mut sys = adoption_sys(MANUAL_CONFIG_UUID);
        sys.paths
            .insert("/var/lib/rpi/cloudflared/creds.json".into());
        // A cloudflared is running, but config.yml is present -> the adoption
        // branch runs first and the fresh-path guard never fires.
        sys.err
            .remove(&FakeSys::key("pgrep", &["-x", "cloudflared"]));
        sys.ok
            .insert(FakeSys::key("pgrep", &["-x", "cloudflared"]), "4321".into());
        let cf = MockCloudflareApi::new();
        let mut rep = SetupReport::default();
        let adopted =
            cloudflared_bootstrap_full(&sys, &cf, &adoption_opts(), false, &mut rep).await;
        assert!(adopted, "config.yml present -> adoption");
        assert!(
            !rep.errors.iter().any(|e| e.contains("refusing")),
            "guard must not fire on adoption: {:?}",
            rep.errors
        );
    }
```

- [ ] **Step 8: Run the guard tests to verify they fail**

Run: `rtk cargo test -p pi bootstrap_refuses_when_foreign_cloudflared_running bootstrap_dry_run_refuses_when_foreign_cloudflared_running bootstrap_refuses_when_pgrep_unavailable adoption_runs_even_with_a_cloudflared_already_running`
Expected: FAIL — the three refuse tests panic (the un-mocked `find_or_create_tunnel` is reached / writes happen); the adoption test may already pass.

- [ ] **Step 9: Add the guard to `cloudflared_bootstrap_full`**

In `cloudflared_bootstrap_full`, insert the guard between the end of the adoption `if` block and the `if dry {` block (after the `}` closing `if sys.exists(Path::new(CLOUDFLARED_CONFIG_PATH))`, before `if dry {`):

```rust
    // Foreign-tunnel guard (Part A): config.yml is absent, so rpi has never
    // created a tunnel here — any cloudflared already running is foreign.
    // Refuse rather than overwrite its unit and restart it. Every destructive
    // action below happens after this point.
    match cloudflared_running(sys).await {
        CloudflaredState::NotRunning => {}
        CloudflaredState::Running => {
            let msg = format!(
                "a cloudflared tunnel is already running on this host, but rpi has no config.yml \
                 at {CLOUDFLARED_CONFIG_PATH} to adopt — refusing to overwrite it and restart the \
                 tunnel. Stop the running cloudflared (then re-run to create a fresh rpi-managed \
                 tunnel), or move its config to {CLOUDFLARED_CONFIG_PATH} so setup adopts it."
            );
            if dry {
                rep.skipped.push(format!("would refuse: {msg}"));
            } else {
                rep.errors.push(msg);
            }
            return false;
        }
        CloudflaredState::Undetermined => {
            let msg = "could not check for a running cloudflared (pgrep unavailable); refusing to \
                       proceed rather than risk overwriting an existing tunnel — verify manually \
                       that no cloudflared is running, or install pgrep, then re-run."
                .to_string();
            if dry {
                rep.skipped.push(format!("would refuse: {msg}"));
            } else {
                rep.errors.push(msg);
            }
            return false;
        }
    }

```

- [ ] **Step 10: Run the guard tests and the full setup suite to verify green**

Run: `rtk cargo test -p pi agent::setup`
Expected: PASS — the four new guard tests plus every pre-existing `agent::setup::*` test (the fresh-install and adoption tests stay green because `fresh_sys()` now seeds `pgrep` = NotRunning and adoption never consults it).

- [ ] **Step 11: Verify and commit**

Run:
```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
```
Expected: all PASS.

```bash
rtk git add crates/bin/src/agent/setup.rs
rtk git commit -m "fix(setup): refuse fresh cloudflared install when a foreign tunnel is running

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2 (Part B): Secure token input

Accept the Cloudflare API token from a file/stdin/env, not only argv (which leaks via `ps`/history/`journald`). Add `--cf-token-file`, deprecate inline `--cf-token` with a visible warning, keep the `CLOUDFLARE_API_TOKEN` env fallback.

**Files:**
- Modify: `crates/bin/src/agent/setup.rs` — add `CfTokenResolution` + `resolve_cf_token` near `run_cmd` (~1069); add `cf_token_file` param to `run_cmd` and wire resolution in (replacing the `~1087` env-fallback line); add unit tests in `mod tests`.
- Modify: `crates/bin/src/main.rs` — add `--cf-token-file`, update `--cf-token` help, thread it through the `Setup` match arm and `run_cmd` call (~415–421), add a parse test.

**Interfaces:**
- Produces: `fn resolve_cf_token(token_file: Option<String>, token_inline: Option<String>, env_token: Option<String>, read: impl FnOnce(&str) -> Result<String, String>) -> Result<CfTokenResolution, String>` where `struct CfTokenResolution { token: Option<String>, warning: Option<String> }` (module-private; the resolved token flows through the existing `cf_token: Option<String>` in `SetupOpts`, so no downstream change). `run_cmd` gains a `cf_token_file: Option<String>` parameter placed immediately after `cf_token`.
- Consumes: `crate::output::warn` (stderr) for the deprecation warning; `SetupOpts.cf_token` (unchanged).

- [ ] **Step 1: Write the failing `resolve_cf_token` unit tests**

In `crates/bin/src/agent/setup.rs`, `mod tests`, add:

```rust
    #[test]
    fn cf_token_file_is_read_and_trimmed() {
        let r = resolve_cf_token(Some("/tok".into()), None, None, |p| {
            assert_eq!(p, "/tok");
            Ok("secret-token\n".into())
        })
        .unwrap();
        assert_eq!(r.token.as_deref(), Some("secret-token"));
        assert!(r.warning.is_none());
    }

    #[test]
    fn cf_token_file_dash_reads_stdin() {
        // The real reader maps "-" to stdin; the helper just passes the path.
        let r = resolve_cf_token(Some("-".into()), None, None, |p| {
            assert_eq!(p, "-");
            Ok("from-stdin\n".into())
        })
        .unwrap();
        assert_eq!(r.token.as_deref(), Some("from-stdin"));
    }

    #[test]
    fn cf_token_file_unreadable_is_error() {
        let err = resolve_cf_token(Some("/nope".into()), None, None, |_| {
            Err("No such file".into())
        })
        .unwrap_err();
        assert!(err.contains("--cf-token-file /nope"), "{err}");
    }

    #[test]
    fn cf_token_file_empty_resolved_token_is_error() {
        let err =
            resolve_cf_token(Some("/tok".into()), None, None, |_| Ok("   \n".into())).unwrap_err();
        assert!(err.contains("empty token"), "{err}");
    }

    #[test]
    fn cf_token_inline_resolves_with_deprecation_warning() {
        let r = resolve_cf_token(None, Some("inline-tok".into()), None, |_| unreachable!()).unwrap();
        assert_eq!(r.token.as_deref(), Some("inline-tok"));
        assert!(r.warning.as_deref().unwrap().contains("--cf-token-file"));
    }

    #[test]
    fn cf_token_file_wins_over_inline_but_still_warns() {
        let r = resolve_cf_token(
            Some("/tok".into()),
            Some("inline-tok".into()),
            None,
            |_| Ok("file-tok".into()),
        )
        .unwrap();
        assert_eq!(r.token.as_deref(), Some("file-tok"));
        assert!(
            r.warning.is_some(),
            "inline token still triggers the deprecation warning"
        );
    }

    #[test]
    fn cf_token_falls_back_to_env() {
        let r = resolve_cf_token(None, None, Some("env-tok".into()), |_| unreachable!()).unwrap();
        assert_eq!(r.token.as_deref(), Some("env-tok"));
        assert!(r.warning.is_none());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `rtk cargo test -p pi cf_token`
Expected: FAIL — `resolve_cf_token` / `CfTokenResolution` not defined.

- [ ] **Step 3: Implement `resolve_cf_token`**

In `crates/bin/src/agent/setup.rs`, immediately before `pub async fn run_cmd` (~1069):

```rust
const CF_TOKEN_DEPRECATION: &str =
    "--cf-token passes the API token on the command line, where it leaks via `ps`, shell history, \
     and journald; prefer --cf-token-file <path> (or `-` for stdin) or the CLOUDFLARE_API_TOKEN \
     environment variable";

struct CfTokenResolution {
    token: Option<String>,
    warning: Option<String>,
}

/// Resolve the Cloudflare API token most-secure-first: `--cf-token-file`
/// (`-` = stdin) beats the deprecated inline `--cf-token`, which beats the
/// `CLOUDFLARE_API_TOKEN` env var. `read` performs the file/stdin read so this
/// stays unit-testable. An unreadable file, or an empty resolved token, is a
/// hard error — not a fall-through.
fn resolve_cf_token(
    token_file: Option<String>,
    token_inline: Option<String>,
    env_token: Option<String>,
    read: impl FnOnce(&str) -> Result<String, String>,
) -> Result<CfTokenResolution, String> {
    if let Some(path) = token_file {
        let raw = read(&path).map_err(|e| format!("read --cf-token-file {path}: {e}"))?;
        let token = raw.trim().to_string();
        if token.is_empty() {
            return Err(format!("--cf-token-file {path} resolved to an empty token"));
        }
        let warning = token_inline.map(|_| CF_TOKEN_DEPRECATION.to_string());
        return Ok(CfTokenResolution {
            token: Some(token),
            warning,
        });
    }
    if let Some(inline) = token_inline {
        return Ok(CfTokenResolution {
            token: Some(inline),
            warning: Some(CF_TOKEN_DEPRECATION.to_string()),
        });
    }
    Ok(CfTokenResolution {
        token: env_token,
        warning: None,
    })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `rtk cargo test -p pi cf_token`
Expected: PASS (all seven).

- [ ] **Step 5: Add `cf_token_file` to `run_cmd` and wire in resolution**

In `crates/bin/src/agent/setup.rs`, change the `run_cmd` signature to add `cf_token_file` right after `cf_token`:

```rust
pub async fn run_cmd(
    user: Option<String>,
    with_cloudflared: bool,
    cf_token: Option<String>,
    cf_token_file: Option<String>,
    domain: Option<String>,
    tunnel: Option<String>,
    dry_run: bool,
) -> anyhow::Result<()> {
```

Replace the existing env-fallback line:

```rust
    let cf_token = cf_token.or_else(|| std::env::var("CLOUDFLARE_API_TOKEN").ok());
```

with:

```rust
    let read_token = |path: &str| -> Result<String, String> {
        if path == "-" {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| e.to_string())?;
            Ok(buf)
        } else {
            std::fs::read_to_string(path).map_err(|e| e.to_string())
        }
    };
    let resolution = resolve_cf_token(
        cf_token_file,
        cf_token,
        std::env::var("CLOUDFLARE_API_TOKEN").ok(),
        read_token,
    )
    .map_err(|e| anyhow::anyhow!(e))?;
    if let Some(w) = &resolution.warning {
        crate::output::warn(w);
    }
    let cf_token = resolution.token;
```

- [ ] **Step 6: Add `--cf-token-file`, update `--cf-token` help, thread it through `main.rs`**

In `crates/bin/src/main.rs`, in the `Setup { … }` variant (~219–230), replace the `cf_token` arg and add `cf_token_file` right after it:

```rust
        /// DEPRECATED (leaks via ps/shell-history/journald): Cloudflare API token inline; prefer --cf-token-file or CLOUDFLARE_API_TOKEN
        #[arg(long)]
        cf_token: Option<String>,
        /// Read the Cloudflare API token from a file (path, or `-` for stdin); preferred over --cf-token
        #[arg(long)]
        cf_token_file: Option<String>,
```

Update the `Setup` match arm to destructure and pass it (~411–421):

```rust
        Cmd::Agent {
            cmd:
                AgentCmd::Setup {
                    user,
                    with_cloudflared,
                    cf_token,
                    cf_token_file,
                    domain,
                    tunnel,
                    dry_run,
                },
        } => {
            agent::setup::run_cmd(
                user,
                with_cloudflared,
                cf_token,
                cf_token_file,
                domain,
                tunnel,
                dry_run,
            )
            .await
        }
```

- [ ] **Step 7: Write the failing CLI parse test**

In `crates/bin/src/main.rs` tests module, add:

```rust
    #[test]
    fn parses_agent_setup_cf_token_file() {
        let cli = Cli::try_parse_from([
            "rpi",
            "agent",
            "setup",
            "--with-cloudflared",
            "--cf-token-file",
            "/run/secrets/cf",
            "--domain",
            "example.com",
        ])
        .unwrap();
        match cli.cmd.unwrap() {
            Cmd::Agent {
                cmd: AgentCmd::Setup { cf_token_file, .. },
            } => {
                assert_eq!(cf_token_file.as_deref(), Some("/run/secrets/cf"));
            }
            _ => panic!("expected agent setup"),
        }
    }
```

- [ ] **Step 8: Run the parse test to verify it passes (and the existing setup parse tests stay green)**

Run: `rtk cargo test -p pi parses_agent_setup_cf_token_file agent_setup_flags_parse parses_agent_setup_cloudflare_flags`
Expected: PASS — the two pre-existing tests destructure with `..` so the new field doesn't break them.

- [ ] **Step 9: Verify and commit**

Run:
```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
```
Expected: all PASS.

```bash
rtk git add crates/bin/src/agent/setup.rs crates/bin/src/main.rs
rtk git commit -m "feat(setup): accept Cloudflare token from file/stdin, deprecate inline --cf-token

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3 (Part C): Doctor half-state checks

Two new `HostSystemProbe::diagnostics()` checks that fire when ingress is *active* but the data plane is broken — the half-state a rolled-back deploy or dead connector produces: (a) connector-alive (cheap, pgrep) and (b) route-missing (parse `config.yml`). Requires threading the config path into the probe.

**Files:**
- Modify: `crates/infrastructure/src/probe.rs` — add `cloudflared_config: Option<String>` field + `new` param; add the two checks in `diagnostics()`; add a scripted runner + `probe_with` helper + tests; add `None` to the existing `probe()` helper.
- Modify: `crates/bin/src/agent/state.rs` — pass `config.cloudflared.as_ref().map(|c| c.config.to_string_lossy().into_owned())` at the real probe call site (~166–175).
- Modify: `crates/bin/src/agent/http.rs` — pass `None` at the test probe call site (~818–827).

**Interfaces:**
- Produces: `HostSystemProbe::new(... , ingress_active: bool, cloudflared_config: Option<String>, started_at: i64)` — one new param, placed **after `ingress_active`, before `started_at`**. New diagnostic check names: `"cloudflared connector"` and `"ingress route"`.
- Consumes: `ProbeRunner::run` (same `Ok/Err`/`"spawn "` contract as `Sys::run`); `serde_yaml` (already a dep of `pi-infrastructure`); `ProjectRepository::list` (already used in `diagnostics`); `CloudflaredSection.config: PathBuf` (from Task 2's untouched config schema) at the state.rs call site.

- [ ] **Step 1: Add the field + constructor param (compile-only change first)**

In `crates/infrastructure/src/probe.rs`, add the field to the struct (after `ingress_active`, before `started_at`):

```rust
pub struct HostSystemProbe {
    runner: Arc<dyn ProbeRunner>,
    disk: Arc<dyn DiskProbe>,
    projects: Arc<dyn ProjectRepository>,
    version: String,
    disk_threshold_percent: u8,
    cloudflared_enabled: bool,
    ingress_active: bool,
    cloudflared_config: Option<String>,
    started_at: i64,
}
```

Add the param to `new` (keep `#[allow(clippy::too_many_arguments)]`), after `ingress_active`, before `started_at`, and set it in the struct literal:

```rust
    pub fn new(
        runner: Arc<dyn ProbeRunner>,
        disk: Arc<dyn DiskProbe>,
        projects: Arc<dyn ProjectRepository>,
        version: String,
        disk_threshold_percent: u8,
        cloudflared_enabled: bool,
        ingress_active: bool,
        cloudflared_config: Option<String>,
        started_at: i64,
    ) -> Arc<HostSystemProbe> {
        Arc::new(HostSystemProbe {
            runner,
            disk,
            projects,
            version,
            disk_threshold_percent,
            cloudflared_enabled,
            ingress_active,
            cloudflared_config,
            started_at,
        })
    }
```

- [ ] **Step 2: Update the three call sites so the workspace compiles**

`crates/bin/src/agent/state.rs` (~166–175), insert the config path between `ingress_active` and `now`:

```rust
    let probe = HostSystemProbe::new(
        Arc::new(SystemRunner),
        disk,
        projects.clone(),
        env!("CARGO_PKG_VERSION").to_string(),
        config.gc.disk_threshold_percent,
        config.cloudflared.is_some(),
        ingress_active,
        config
            .cloudflared
            .as_ref()
            .map(|c| c.config.to_string_lossy().into_owned()),
        now,
    );
```

`crates/bin/src/agent/http.rs` (~818–827), insert `None` between the two `false`s and `100`:

```rust
        let probe = HostSystemProbe::new(
            Arc::new(SystemRunner),
            disk,
            projects.clone(),
            env!("CARGO_PKG_VERSION").to_string(),
            85,
            false,
            false,
            None,
            100,
        );
```

`crates/infrastructure/src/probe.rs`, the existing test helper `probe()` (~285), insert `None` between `ingress_active` and `0`:

```rust
        HostSystemProbe::new(
            Arc::new(FakeRunner),
            Arc::new(disk),
            Arc::new(repo),
            "0.0.0".into(),
            85,
            false, // cloudflared binary/service checks off — not under test
            ingress_active,
            None,
            0,
        )
```

Run: `rtk cargo build --locked`
Expected: builds clean (behavior unchanged so far).

- [ ] **Step 3: Add the scripted runner + `probe_with` helper to the probe tests**

In `crates/infrastructure/src/probe.rs`, `mod tests`, add below the existing `FakeRunner`:

```rust
    struct ScriptedRunner(std::collections::HashMap<String, Result<String, String>>);

    #[async_trait]
    impl ProbeRunner for ScriptedRunner {
        async fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
            let key = std::iter::once(program)
                .chain(args.iter().copied())
                .collect::<Vec<_>>()
                .join(" ");
            self.0.get(&key).cloned().unwrap_or_else(|| Ok("ok".into()))
        }
    }

    fn probe_with(
        runner: Arc<dyn ProbeRunner>,
        ingress_active: bool,
        cloudflared_config: Option<String>,
        projects: Vec<Project>,
    ) -> Arc<HostSystemProbe> {
        let mut repo = MockProjectRepository::new();
        repo.expect_list().returning(move || Ok(projects.clone()));
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(10));
        HostSystemProbe::new(
            runner,
            Arc::new(disk),
            Arc::new(repo),
            "0.0.0".into(),
            85,
            false,
            ingress_active,
            cloudflared_config,
            0,
        )
    }
```

- [ ] **Step 4: Write the failing connector + route tests**

In `crates/infrastructure/src/probe.rs`, `mod tests`, add:

```rust
    #[tokio::test]
    async fn active_ingress_without_running_connector_fails_the_connector_check() {
        let mut responses = std::collections::HashMap::new();
        // pgrep ran, no match (non-"spawn " error) -> connector is down.
        responses.insert("pgrep -x cloudflared".to_string(), Err(String::new()));
        let report = probe_with(Arc::new(ScriptedRunner(responses)), true, None, vec![])
            .diagnostics()
            .await;
        let check = report
            .checks
            .iter()
            .find(|c| c.name == "cloudflared connector")
            .expect("connector check present");
        assert!(!check.passed);
        assert!(check.detail.contains("no cloudflared process is running"));
    }

    #[tokio::test]
    async fn active_ingress_with_running_connector_adds_no_connector_check() {
        let mut responses = std::collections::HashMap::new();
        responses.insert("pgrep -x cloudflared".to_string(), Ok("4321".to_string()));
        let report = probe_with(Arc::new(ScriptedRunner(responses)), true, None, vec![])
            .diagnostics()
            .await;
        assert!(report.checks.iter().all(|c| c.name != "cloudflared connector"));
    }

    #[tokio::test]
    async fn connector_check_skipped_when_pgrep_unavailable() {
        let mut responses = std::collections::HashMap::new();
        responses.insert(
            "pgrep -x cloudflared".to_string(),
            Err("spawn pgrep: No such file or directory".to_string()),
        );
        let report = probe_with(Arc::new(ScriptedRunner(responses)), true, None, vec![])
            .diagnostics()
            .await;
        assert!(report.checks.iter().all(|c| c.name != "cloudflared connector"));
    }

    #[tokio::test]
    async fn active_ingress_missing_route_fails_the_route_check() {
        let config = "tunnel: t\ningress:\n  - hostname: a.example.com\n    service: http://127.0.0.1:8001\n  - service: http_status:404\n";
        let mut responses = std::collections::HashMap::new();
        // connector up, so the route check is what fires (isolate it).
        responses.insert("pgrep -x cloudflared".to_string(), Ok("4321".to_string()));
        responses.insert(
            "cat /etc/rpi/config.yml".to_string(),
            Ok(config.to_string()),
        );
        let report = probe_with(
            Arc::new(ScriptedRunner(responses)),
            true,
            Some("/etc/rpi/config.yml".into()),
            vec![project(Some("a.example.com")), project(Some("b.example.com"))],
        )
        .diagnostics()
        .await;
        let check = report
            .checks
            .iter()
            .find(|c| c.name == "ingress route")
            .expect("ingress route check present");
        assert!(!check.passed);
        assert!(check.detail.contains("b.example.com"));
        assert!(
            !check.detail.contains("a.example.com"),
            "routed hostname must not be listed"
        );
    }

    #[tokio::test]
    async fn route_check_skipped_when_config_absent_or_unreadable() {
        // config path is None -> route check skipped.
        let report = probe_with(
            Arc::new(ScriptedRunner(std::collections::HashMap::new())),
            true,
            None,
            vec![project(Some("a.example.com"))],
        )
        .diagnostics()
        .await;
        assert!(report.checks.iter().all(|c| c.name != "ingress route"));

        // config path set but `cat` fails -> route check skipped silently.
        let mut responses = std::collections::HashMap::new();
        responses.insert(
            "cat /etc/rpi/config.yml".to_string(),
            Err("No such file".to_string()),
        );
        let report = probe_with(
            Arc::new(ScriptedRunner(responses)),
            true,
            Some("/etc/rpi/config.yml".into()),
            vec![project(Some("a.example.com"))],
        )
        .diagnostics()
        .await;
        assert!(report.checks.iter().all(|c| c.name != "ingress route"));
    }
```

- [ ] **Step 5: Run the tests to verify they fail**

Run: `rtk cargo test -p pi-infrastructure connector route`
Expected: FAIL — no `"cloudflared connector"` / `"ingress route"` checks are produced yet (`.expect(...)` panics on the two failing-check tests).

- [ ] **Step 6: Implement the two checks in `diagnostics()`**

In `crates/infrastructure/src/probe.rs`, `diagnostics()`, insert immediately after the closing brace of the existing `if !self.ingress_active { … }` block and before the `checks.push(match self.disk.used_percent() … )` block:

```rust
        if self.ingress_active {
            // (a) connector-alive: ingress configured but no cloudflared process.
            match self.runner.run("pgrep", &["-x", "cloudflared"]).await {
                Ok(_) => {}                              // connector up — healthy
                Err(e) if e.starts_with("spawn ") => {} // pgrep unavailable — can't tell, skip
                Err(_) => checks.push(DiagnosticCheck {
                    name: "cloudflared connector".into(),
                    passed: false,
                    detail: "ingress is configured but no cloudflared process is running".into(),
                    hint: Some(
                        "start it: sudo -u rpi-agent XDG_RUNTIME_DIR=/run/user/<uid> \
                         systemctl --user start cloudflared (or check its logs)"
                            .into(),
                    ),
                }),
            }

            // (b) route-missing: declared hostname with no route in config.yml.
            if let Some(path) = &self.cloudflared_config {
                if let Ok(text) = self.runner.run("cat", &[path]).await {
                    if let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&text) {
                        let routed: std::collections::HashSet<String> = doc
                            .get("ingress")
                            .and_then(|v| v.as_sequence())
                            .map(|rules| {
                                rules
                                    .iter()
                                    .filter_map(|r| {
                                        r.get("hostname").and_then(|h| h.as_str()).map(String::from)
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        if let Ok(projects) = self.projects.list().await {
                            let missing: Vec<String> = projects
                                .iter()
                                .filter_map(|p| p.config.hostname.clone())
                                .filter(|h| !routed.contains(h))
                                .collect();
                            if !missing.is_empty() {
                                checks.push(DiagnosticCheck {
                                    name: "ingress route".into(),
                                    passed: false,
                                    detail: format!(
                                        "hostname(s) declared with a running ingress but no route in config.yml: {}",
                                        missing.join(", ")
                                    ),
                                    hint: Some(
                                        "re-deploy the project(s) to (re)create the route, or check config.yml"
                                            .into(),
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }

```

- [ ] **Step 7: Run the new tests + the existing probe tests to verify green**

Run: `rtk cargo test -p pi-infrastructure probe`
Expected: PASS — the five new tests plus the two pre-existing ones (`disabled_ingress_with_hostnames_fails_the_ingress_check`, `active_ingress_or_no_hostnames_add_no_check`). The latter stays green: with `FakeRunner` (`pgrep` → `Ok("ok")`) there's no connector failure, and `cloudflared_config = None` skips the route check.

- [ ] **Step 8: Verify and commit**

Run:
```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
```
Expected: all PASS.

```bash
rtk git add crates/infrastructure/src/probe.rs crates/bin/src/agent/state.rs crates/bin/src/agent/http.rs
rtk git commit -m "feat(doctor): flag ingress half-states (connector down, missing route)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4 (Part D): DBUS in the user-scoped systemctl env

Belt-and-suspenders alongside the existing `XDG_RUNTIME_DIR` injection: inject `DBUS_SESSION_BUS_ADDRESS` too, in both the in-process restart (`cloudflared.rs`) and the `enable --now` invocation (`setup.rs`), gated on the same "env lacks a non-empty `XDG_RUNTIME_DIR`" condition.

**Files:**
- Modify: `crates/infrastructure/src/cloudflared.rs` — `restart_extra_env` → `Vec<(&'static str, String)>` adding DBUS; `apply_restart_env` iterates; update `restart_env_added_only_when_missing` test (~466).
- Modify: `crates/bin/src/agent/setup.rs` — add `DBUS_SESSION_BUS_ADDRESS` to the `sudo -u rpi-agent … enable --now cloudflared` call in `cloudflared_user_service` (~962–978); add a test asserting the enable call carries it.

**Interfaces:**
- Produces: `fn restart_extra_env(current: Option<&str>, uid: u32) -> Vec<(&'static str, String)>` — empty when the env already has a non-empty `XDG_RUNTIME_DIR`; otherwise `[("XDG_RUNTIME_DIR", "/run/user/<uid>"), ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/<uid>/bus")]`.
- Consumes: `current_uid()` (unchanged, runtime uid).

- [ ] **Step 1: Update the `restart_extra_env` unit test to the new `Vec` contract**

In `crates/infrastructure/src/cloudflared.rs`, `mod tests`, replace `restart_env_added_only_when_missing` with:

```rust
    #[test]
    fn restart_env_added_only_when_missing() {
        let both = vec![
            ("XDG_RUNTIME_DIR", "/run/user/999".to_string()),
            (
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/run/user/999/bus".to_string(),
            ),
        ];
        assert_eq!(restart_extra_env(None, 999), both);
        assert!(restart_extra_env(Some("/run/user/1000"), 999).is_empty());
        assert_eq!(restart_extra_env(Some(""), 999), both);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `rtk cargo test -p pi-infrastructure restart_env_added_only_when_missing`
Expected: FAIL — `restart_extra_env` still returns `Option`, so the `Vec`/`is_empty()` assertions don't type-check / don't match.

- [ ] **Step 3: Broaden `restart_extra_env` and `apply_restart_env`**

In `crates/infrastructure/src/cloudflared.rs`, replace `restart_extra_env`:

```rust
/// `systemctl --user` needs XDG_RUNTIME_DIR (and, on non-standard setups,
/// DBUS_SESSION_BUS_ADDRESS) to reach the user manager; the rpi-agent unit sets
/// neither. Compute the variables to add to the restart command when the
/// agent's own environment lacks a non-empty XDG_RUNTIME_DIR.
fn restart_extra_env(current: Option<&str>, uid: u32) -> Vec<(&'static str, String)> {
    match current {
        Some(v) if !v.is_empty() => Vec::new(),
        _ => vec![
            ("XDG_RUNTIME_DIR", format!("/run/user/{uid}")),
            (
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path=/run/user/{uid}/bus"),
            ),
        ],
    }
}
```

And update `apply_restart_env` to iterate:

```rust
fn apply_restart_env(cmd: &mut Command) {
    let current = std::env::var("XDG_RUNTIME_DIR").ok();
    for (k, v) in restart_extra_env(current.as_deref(), current_uid()) {
        cmd.env(k, v);
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `rtk cargo test -p pi-infrastructure restart_env_added_only_when_missing`
Expected: PASS.

- [ ] **Step 5: Write the failing enable-path DBUS test**

In `crates/bin/src/agent/setup.rs`, `mod tests`, add:

```rust
    #[tokio::test]
    async fn cloudflared_user_service_enable_passes_dbus_address() {
        let mut sys = fresh_sys();
        // fresh_sys() seeds `id -u rpi-agent` to fail; this function assumes the
        // user exists, so make it succeed with a known uid.
        sys.err.remove(&FakeSys::key("id", &["-u", "rpi-agent"]));
        sys.ok
            .insert(FakeSys::key("id", &["-u", "rpi-agent"]), "999".into());
        let mut rep = SetupReport::default();
        cloudflared_user_service(&sys, false, &mut rep).await;
        assert!(
            sys.calls().iter().any(|c| c
                .contains("DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/999/bus")
                && c.contains("enable")
                && c.contains("--now")
                && c.contains("cloudflared")),
            "enable --now must pass the DBUS session bus address: {:?}",
            sys.calls()
        );
    }
```

- [ ] **Step 6: Run the test to verify it fails**

Run: `rtk cargo test -p pi cloudflared_user_service_enable_passes_dbus_address`
Expected: FAIL — the enable call carries only `XDG_RUNTIME_DIR` today.

- [ ] **Step 7: Add DBUS to the `enable --now` invocation**

In `crates/bin/src/agent/setup.rs`, `cloudflared_user_service`, replace the runtime binding + enable call (~962–978):

```rust
    let runtime = format!("XDG_RUNTIME_DIR=/run/user/{uid}");
    let dbus = format!("DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus");
    // enable+start the user unit as rpi-agent with the runtime dir + bus set
    let _ = sys
        .run(
            "sudo",
            &[
                "-u",
                "rpi-agent",
                &runtime,
                &dbus,
                "systemctl",
                "--user",
                "enable",
                "--now",
                "cloudflared",
            ],
        )
        .await;
```

- [ ] **Step 8: Run the test to verify it passes (and the existing user-service test stays green)**

Run: `rtk cargo test -p pi cloudflared_user_service`
Expected: PASS — the new test plus the pre-existing `cloudflared_user_service` test (which asserts on `daemon-reload`/`restart`/`enable-linger`, none affected by the added DBUS arg).

- [ ] **Step 9: Verify and commit**

Run:
```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
```
Expected: all PASS.

```bash
rtk git add crates/infrastructure/src/cloudflared.rs crates/bin/src/agent/setup.rs
rtk git commit -m "fix(cloudflared): inject DBUS_SESSION_BUS_ADDRESS alongside XDG_RUNTIME_DIR

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-Review (spec coverage)

- **A. Foreign-tunnel guard** — Task 1: `CloudflaredState`/`cloudflared_running` (A.3), guard at placement P1 before the `if dry`/tunnel creation (A.4), real/dry/undetermined messages (A.5), tests A(1)–A(6). No `--force`, no change to the no-token fallback (A.6). ✅
- **B. Secure token input** — Task 2: `--cf-token-file` with `-`=stdin, trim, unreadable/empty → hard error (B.1); resolution order file→inline(warn)→env (B.2); `--cf-token-file` arg + `--cf-token` deprecation help (B.3); token still flows via `SetupOpts.cf_token` (no downstream change). Tests B(1)–(6) + empty-file. On-disk storage untouched (B.4). ✅
- **C. Doctor half-state checks** — Task 3: connector-alive check, no constructor change to that logic (C.1); route-missing via threaded `cloudflared_config` + serde_yaml parse (C.2); constructor ripple with param after `ingress_active`, call sites in state.rs (Some when `[cloudflared]` present) + http.rs (None) + probe test helper (C.3); tests C(a)/(b). No Cloudflare-API DNS lookup (C.4). ✅
- **D. DBUS env** — Task 4: `restart_extra_env` → `Vec` with both vars, same gate, `apply_restart_env` iterates, runtime uid (D.1); DBUS added to the `enable --now` path (D.1); unit test both-present/empty/empty-string; enable-path assertion. Two execution contexts not unified (D.2). ✅
- **Files touched** match the spec's "Files touched" list exactly. ✅
- **No new dependency**: `serde_yaml` confirmed already a workspace dep of both `pi` and `pi-infrastructure`. ✅
