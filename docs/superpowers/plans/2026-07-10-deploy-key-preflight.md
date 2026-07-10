# Deploy Key Preflight Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `rpi deploy` verifies repo access before creating a deployment; when the agent can't read an SSH repo it auto-registers a read-only deploy key via local `gh`, or shows the key with instructions and waits — the first deploy never fails on a missing key.

**Architecture:** CLI-orchestrated preflight (spec `docs/superpowers/specs/2026-07-10-deploy-key-preflight-design.md`). New stateless agent route `POST /v1/projects/{name}/source/check` backed by a new `Source::check_access` contract method (`GitSource`: ensure key + `git ls-remote`). New CLI module `cli/sourcekey.rs` orchestrates: check → gh auto-register → manual box + 5s polling. Old agents (404) degrade silently to today's behaviour.

**Tech Stack:** Rust workspace (crates: `pi-domain`, `pi-infrastructure`, `pi-application`, bin crate `pi`), axum agent, reqwest CLI, mockall mocks, `console`/LogPane output primitives, GitHub CLI (`gh`) as an optional external tool.

## Global Constraints

- Every shell command is prefixed with `rtk` (repo rule, `C:\Users\Khmil\CLAUDE.md`).
- CI gate before finishing any task's commit (Stop hook enforces it too): `rtk cargo fmt --all -- --check && rtk cargo clippy --all-targets --locked -- -D warnings && rtk cargo test --locked`. If fmt reports a diff: `rtk cargo fmt --all`, include in the commit.
- Wire contract (spec, verbatim): route `POST /v1/projects/{name}/source/check`; request `{ "repo": string }`; response `{ "ok": bool, "pubkey": string?, "error": string? }`. Additive — **no API version bump** (`"api": "v1"` unchanged).
- Copy/values (spec, verbatim): key title/comment `pi-deploy-{project}`; `read_only=true`; poll every **5 s**, cap **600 s**; ls-remote timeout **30 s**; ed25519 keys at `keys/{name}/id_ed25519`.
- **Deliberate deviation from the add-cli-command 404 pattern:** bare 404 from the new route means old agent → *skip the preflight silently* (spec decision), NOT "bail: update the agent".
- Deviation from the spec's box mockup (approved during planning): the pubkey prints as a plain full-width line **above** the pane, because `LogPane` truncates content to terminal width and a clipped key can't be copied. Task 6 amends the spec.
- Commit messages: conventional style used by this repo (`feat(agent):`, `feat(cli):`, `docs:`, `refactor(...):`), each ending with `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Work happens in this worktree (`.worktrees/deploy-stages`, branch `worktree-deploy-stages`).

## File Structure

- `crates/domain/src/contracts.rs` — `SourceAccess` enum + `Source::check_access` (Task 1).
- `crates/infrastructure/src/git.rs` — `is_ssh_repo` goes `pub`; `ensure_key` loses its `LogSink`; `check_access` impl (Task 1).
- `crates/bin/src/proto.rs` — `SourceCheckRequest` / `SourceCheckResponse` DTOs (Task 2).
- `crates/bin/src/agent/state.rs` + `crates/bin/src/agent/http.rs` — `AppState.source` (both construction sites) + route + handler (Task 2).
- `crates/bin/src/cli/api.rs` — `ApiClient::source_check` with silent-404 (Task 3).
- `crates/bin/src/cli/sourcekey.rs` (new) + `crates/bin/src/cli/mod.rs` — pure helpers (Task 4), orchestration (Task 5).
- `crates/bin/src/cli/commands.rs`, `crates/bin/src/main.rs`, root `Cargo.toml` (tokio `signal` feature) — deploy wiring + `--no-gh-key` (Task 5).
- `README.md`, `docs/superpowers/specs/2026-07-10-deploy-key-preflight-design.md` — docs (Task 6).

---

### Task 1: Domain contract `SourceAccess` + `GitSource::check_access`

**Files:**
- Modify: `crates/domain/src/contracts.rs` (Source trait, ~line 30; new enum near `IngressOutcome`)
- Modify: `crates/infrastructure/src/git.rs`
- Test: `crates/infrastructure/src/git.rs` (`mod integration` at the bottom)

**Interfaces:**
- Consumes: existing `GitSource::git()` command builder, `run_capture`, `DomainError::Source`.
- Produces (later tasks rely on these exact shapes):
  - `pi_domain::contracts::SourceAccess` — `enum SourceAccess { Ok, Denied { pubkey: String, error: String } }`, derives `Debug, Clone, PartialEq, Eq`.
  - Trait method `async fn check_access(&self, project_name: &str, repo: &str) -> Result<SourceAccess, DomainError>` on `Source` (mockall auto-generates `MockSource::expect_check_access`).
  - `pub fn is_ssh_repo(repo: &str) -> bool` in `pi_infrastructure::git` (was `pub(crate)`).

- [ ] **Step 1: Write the failing tests**

In `crates/infrastructure/src/git.rs`, inside `mod integration`, add a recording sink next to `NullSink` and four tests. Add `use pi_domain::contracts::SourceAccess;` and `use std::sync::Mutex;` to the integration mod's imports.

```rust
    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<String>>);
    impl pi_domain::contracts::LogSink for RecordingSink {
        fn line(&self, line: &str) {
            self.0.lock().unwrap().push(line.to_string());
        }
        fn finished(&self, _status: DeploymentStatus) {}
    }

    #[tokio::test]
    async fn check_access_ok_for_non_ssh_repo_without_probing() {
        let data = tempfile::tempdir().unwrap();
        let source = GitSource::new(data.path());
        // .invalid TLD can't resolve — proves non-SSH repos short-circuit.
        let access = source
            .check_access("demo", "https://example.invalid/x/y.git")
            .await
            .unwrap();
        assert_eq!(access, SourceAccess::Ok);
        assert!(!data.path().join("keys/demo").exists(), "no key generated");
    }

    #[tokio::test]
    async fn check_access_denied_generates_key_and_reports_error() {
        let data = tempfile::tempdir().unwrap();
        let source = GitSource::new(data.path());
        // port 1 refuses instantly — a deterministic, offline auth-ish failure
        let access = source
            .check_access("demo", "ssh://git@127.0.0.1:1/x/y.git")
            .await
            .unwrap();
        let SourceAccess::Denied { pubkey, error } = access else {
            panic!("expected Denied");
        };
        assert!(pubkey.starts_with("ssh-ed25519"), "{pubkey}");
        assert!(pubkey.contains("pi-deploy-demo"), "{pubkey}");
        assert!(!error.is_empty());
        assert!(data.path().join("keys").join("demo").join("id_ed25519").exists());

        // second call reuses the key, does not regenerate
        let again = source
            .check_access("demo", "ssh://git@127.0.0.1:1/x/y.git")
            .await
            .unwrap();
        let SourceAccess::Denied { pubkey: pk2, .. } = again else {
            panic!("expected Denied");
        };
        assert_eq!(pk2, pubkey);
    }

    #[tokio::test]
    async fn fetch_still_logs_deploy_key_hint_when_generating_key() {
        // Safety net for old-CLI/old-agent mixes (spec): the fetch stage keeps
        // printing the pubkey hint when it generates a key.
        let data = tempfile::tempdir().unwrap();
        let source = GitSource::new(data.path());
        let sink = std::sync::Arc::new(RecordingSink::default());
        let mut config = cfg(std::path::Path::new("unused"));
        config.repo = "ssh://git@127.0.0.1:1/x/y.git".into();
        let result = source
            .fetch(&config, &DeployRef::Branch("main".into()), sink.clone())
            .await;
        assert!(result.is_err(), "clone against port 1 must fail");
        let all = sink.0.lock().unwrap().join("\n");
        assert!(all.contains("generated deploy key"), "{all}");
        assert!(all.contains("ssh-ed25519"), "{all}");
    }
```

And in the unit `mod tests` (next to `ssh_repo_detection`):

```rust
    #[test]
    fn is_ssh_repo_is_public_for_the_cli() {
        // the CLI (bin crate) gates its preflight on this exact function
        assert!(crate::git::is_ssh_repo("git@github.com:o/r.git"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk cargo test --locked -p pi-infrastructure check_access`
Expected: COMPILE ERROR — `no method named check_access found` (and `SourceAccess` unresolved). That is the failing state for a compiled language.

- [ ] **Step 3: Add the domain contract**

In `crates/domain/src/contracts.rs`, directly above `pub enum IngressOutcome` add:

```rust
/// Deploy-key preflight verdict (spec 2026-07-10): `Ok` — the agent can read
/// the repo; `Denied` — it cannot, and the project's public deploy key plus
/// the probe's error text travel back for the CLI to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceAccess {
    Ok,
    Denied { pubkey: String, error: String },
}
```

In the `Source` trait, after `fetch`, add:

```rust
    /// Deploy-key preflight: verify the agent can read `repo`, generating the
    /// project's deploy key first when missing (SSH repos only; anything else
    /// is `Ok` without probing). Probe failures are data (`Denied`), not
    /// errors — `Err` is reserved for local failures (keygen, fs).
    async fn check_access(
        &self,
        project_name: &str,
        repo: &str,
    ) -> Result<SourceAccess, DomainError>;
```

- [ ] **Step 4: Implement in `GitSource`**

In `crates/infrastructure/src/git.rs`:

a) Make the repo predicate public — change `pub(crate) fn is_ssh_repo` to:

```rust
pub fn is_ssh_repo(repo: &str) -> bool {
    repo.starts_with("git@") || repo.starts_with("ssh://")
}
```

b) Import the new type — extend the contracts import:

```rust
use pi_domain::contracts::{LogSink, Source, SourceAccess};
```

c) Replace `ensure_key` (drops the `LogSink` param; hint-printing moves to `fetch`):

```rust
    /// Ensures the project's deploy key exists. Returns the private key path
    /// and, when the key was generated by this call, the public key text —
    /// callers that stream logs print the add-to-GitHub hint from it.
    async fn ensure_key(&self, project: &str) -> Result<(PathBuf, Option<String>), DomainError> {
        let src_err = |e: std::io::Error| DomainError::Source(format!("deploy key: {e}"));
        let dir = self.keys.join(project);
        let key = dir.join("id_ed25519");
        if key.exists() {
            return Ok((key, None));
        }
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
        Ok((key, Some(pubkey)))
    }
```

d) In `fetch`, replace the key block with:

```rust
        let key = if is_ssh_repo(&project.repo) {
            let (key, generated) = self.ensure_key(&project.name).await?;
            if let Some(pubkey) = generated {
                log.line("generated deploy key for this project; add it to GitHub -> repo Settings -> Deploy keys (read-only), then re-run deploy if fetch fails:");
                log.line(pubkey.trim());
            }
            Some(key)
        } else {
            None
        };
```

(The `let key = key.as_deref();` line below it stays.)

e) Add the probe timeout constant near the top of the file and `check_access` inside `impl Source for GitSource` (after `fetch`):

```rust
/// ls-remote probe timeout — long enough for slow DNS + ssh handshake from a
/// Pi, short enough that the CLI preflight never feels hung (spec: 30 s).
const LS_REMOTE_TIMEOUT_SECS: u64 = 30;
```

```rust
    async fn check_access(
        &self,
        project_name: &str,
        repo: &str,
    ) -> Result<SourceAccess, DomainError> {
        if !is_ssh_repo(repo) {
            return Ok(SourceAccess::Ok);
        }
        let (key, _) = self.ensure_key(project_name).await?;
        let pubkey = tokio::fs::read_to_string(key.with_extension("pub"))
            .await
            .map_err(|e| DomainError::Source(format!("deploy key: {e}")))?;
        let pubkey = pubkey.trim().to_string();

        let mut cmd = self.git(Some(&key), None);
        cmd.args(["ls-remote", repo]);
        cmd.stdin(std::process::Stdio::null());
        cmd.kill_on_drop(true);
        let out = match tokio::time::timeout(
            std::time::Duration::from_secs(LS_REMOTE_TIMEOUT_SECS),
            cmd.output(),
        )
        .await
        {
            Err(_) => {
                return Ok(SourceAccess::Denied {
                    pubkey,
                    error: format!("git ls-remote timed out after {LS_REMOTE_TIMEOUT_SECS}s"),
                })
            }
            Ok(Err(e)) => return Err(DomainError::Source(format!("spawn git ls-remote: {e}"))),
            Ok(Ok(out)) => out,
        };
        if out.status.success() {
            return Ok(SourceAccess::Ok);
        }
        // First meaningful stderr line: skip the StrictHostKeyChecking
        // accept-new notice ("Warning: Permanently added ...") so real causes
        // ("Permission denied (publickey)", "Repository not found") surface.
        let stderr = String::from_utf8_lossy(&out.stderr);
        let error = stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("Warning:"))
            .unwrap_or("git ls-remote failed")
            .to_string();
        Ok(SourceAccess::Denied { pubkey, error })
    }
```

Note: plain `ls-remote` (no `--exit-code`, no ref filter) — an empty-but-accessible repo exits 0 and must count as `Ok`.

- [ ] **Step 5: Run the tests, then the workspace**

Run: `rtk cargo test --locked -p pi-infrastructure`
Expected: PASS, including the three new tests and existing `fetch_clones_then_updates_idempotently`.

Run: `rtk cargo test --locked`
Expected: PASS — `MockSource` regenerates with `expect_check_access`; nothing else calls it yet.

- [ ] **Step 6: Gate + commit**

Run: `rtk cargo fmt --all -- --check && rtk cargo clippy --all-targets --locked -- -D warnings`

```bash
rtk git add crates/domain/src/contracts.rs crates/infrastructure/src/git.rs
rtk git commit -m "feat(domain): Source::check_access deploy-key preflight probe

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Agent route `POST /v1/projects/{name}/source/check`

**Files:**
- Modify: `crates/bin/src/proto.rs` (append DTOs before `#[cfg(test)]`)
- Modify: `crates/bin/src/agent/state.rs` (`AppState` + `build_state`)
- Modify: `crates/bin/src/agent/http.rs` (route, handler, test fixture, tests)
- Test: `crates/bin/src/agent/http.rs` `mod tests`

**Interfaces:**
- Consumes: `Source::check_access` + `SourceAccess` from Task 1; `MockSource::expect_check_access`.
- Produces:
  - `crate::proto::SourceCheckRequest { repo: String }` and `SourceCheckResponse { ok: bool, pubkey: Option<String>, error: Option<String> }` (both `Debug, Clone, Serialize, Deserialize`; Options `#[serde(default)]`).
  - `AppState.source: Arc<dyn Source>` — set in BOTH `build_state` (state.rs) and `state_with` (http.rs tests).
  - Wire: `{"ok":true}` serializes with `"pubkey":null,"error":null` present — the CLI DTO tolerates both null and absent.

Per the add-cli-command decision rule this is a single contract call with no orchestration → **no application-layer use case**; the handler uses `state.source` directly (precedent: `active_deployments`).

- [ ] **Step 1: Write the failing handler tests**

In `crates/bin/src/agent/http.rs` `mod tests`, after `ok_source()` add:

```rust
    fn checked_source(access: pi_domain::contracts::SourceAccess) -> MockSource {
        let mut source = MockSource::new();
        source
            .expect_check_access()
            .returning(move |_, _| Ok(access.clone()));
        source
    }
```

And the tests (next to `version_handshake`):

```rust
    #[tokio::test]
    async fn source_check_ok_shape() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(checked_source(pi_domain::contracts::SourceAccess::Ok)),
            Arc::new(ok_runtime()),
        ));
        let (status, json) = request(
            app,
            post_json(
                "/v1/projects/demo/source/check",
                &serde_json::json!({ "repo": "git@github.com:x/y.git" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["ok"], true);
        assert!(json["pubkey"].is_null());
        assert!(json["error"].is_null());
    }

    #[tokio::test]
    async fn source_check_denied_carries_pubkey_and_error() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(checked_source(pi_domain::contracts::SourceAccess::Denied {
                pubkey: "ssh-ed25519 AAAA pi-deploy-demo".into(),
                error: "Permission denied (publickey)".into(),
            })),
            Arc::new(ok_runtime()),
        ));
        let (status, json) = request(
            app,
            post_json(
                "/v1/projects/demo/source/check",
                &serde_json::json!({ "repo": "git@github.com:x/y.git" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["ok"], false);
        assert_eq!(json["pubkey"], "ssh-ed25519 AAAA pi-deploy-demo");
        assert_eq!(json["error"], "Permission denied (publickey)");
    }

    #[tokio::test]
    async fn source_check_invalid_name_is_400() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let (status, _) = request(
            app,
            post_json(
                "/v1/projects/UPPER/source/check",
                &serde_json::json!({ "repo": "git@github.com:x/y.git" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test --locked -p pi source_check`
Expected: COMPILE ERROR — `AppState` has no field `source` / route missing.

- [ ] **Step 3: DTOs in proto.rs**

Append before the `#[cfg(test)]` module in `crates/bin/src/proto.rs`:

```rust
/// POST /v1/projects/{name}/source/check — deploy-key preflight (spec
/// 2026-07-10). A failed probe is data (`ok: false` + key to register),
/// never an HTTP error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceCheckRequest {
    pub repo: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceCheckResponse {
    pub ok: bool,
    #[serde(default)]
    pub pubkey: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}
```

- [ ] **Step 4: `AppState.source` in both construction sites**

`crates/bin/src/agent/state.rs`:
- Extend the contracts import: `use pi_domain::contracts::{DeploymentHistory, HostNetwork, IdGen, Ingress, Source};`
- Add the field after `pub ids`: `pub source: Arc<dyn Source>,`
- In `build_state`, `source` is currently moved into `SendSecrets::new(secrets.clone(), projects, source, ...)` — change that argument to `source.clone()`, and add to the `AppState { ... }` literal (next to `ids`):

```rust
        source: source as Arc<dyn Source>,
```

`crates/bin/src/agent/http.rs` test fixture `state_with`: the `source: Arc<dyn Source>` param is moved into `SendSecrets::new(secrets.clone(), projects, source, ...)` — change that argument to `source.clone()`, and add `source,` to the `AppState { ... }` literal (next to `ids: UuidGen::new(),`).

- [ ] **Step 5: Route + handler**

In `router()` after the `/v1/projects/{name}/secrets` route:

```rust
        .route("/v1/projects/{name}/source/check", post(source_check))
```

Handler (place near `active_deployments`); extend the proto import list with `SourceCheckRequest, SourceCheckResponse`:

```rust
/// POST /v1/projects/{name}/source/check — deploy-key preflight (spec
/// 2026-07-10). Stateless: ensures the project deploy key exists and probes
/// repo access; a failed probe is `ok: false`, not an HTTP error.
async fn source_check(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<SourceCheckRequest>,
) -> Result<Json<SourceCheckResponse>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    let access = state
        .source
        .check_access(&name, &req.repo)
        .await
        .map_err(ApiError)?;
    Ok(Json(match access {
        pi_domain::contracts::SourceAccess::Ok => SourceCheckResponse {
            ok: true,
            pubkey: None,
            error: None,
        },
        pi_domain::contracts::SourceAccess::Denied { pubkey, error } => SourceCheckResponse {
            ok: false,
            pubkey: Some(pubkey),
            error: Some(error),
        },
    }))
}
```

- [ ] **Step 6: Run tests**

Run: `rtk cargo test --locked -p pi source_check`
Expected: PASS (3 new tests).

Run: `rtk cargo test --locked`
Expected: PASS.

- [ ] **Step 7: Gate + commit**

Run: `rtk cargo fmt --all -- --check && rtk cargo clippy --all-targets --locked -- -D warnings`

```bash
rtk git add crates/bin/src/proto.rs crates/bin/src/agent/state.rs crates/bin/src/agent/http.rs
rtk git commit -m "feat(agent): POST /v1/projects/{name}/source/check deploy-key preflight route

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: `ApiClient::source_check` with silent-404 degradation

**Files:**
- Modify: `crates/bin/src/cli/api.rs`
- Test: `crates/bin/src/cli/api.rs` `mod tests`

**Interfaces:**
- Consumes: `SourceCheckRequest` / `SourceCheckResponse` (Task 2), existing `extract_error`, test helpers `spawn_app` / `not_found_plain`.
- Produces: `pub async fn source_check(&self, project: &str, repo: &str) -> anyhow::Result<Option<SourceCheckResponse>>` — `Ok(None)` ⇔ agent predates the route.

- [ ] **Step 1: Write the failing tests**

In `crates/bin/src/cli/api.rs` `mod tests`:

```rust
    async fn source_check_denied() -> impl IntoResponse {
        axum::Json(serde_json::json!({
            "ok": false,
            "pubkey": "ssh-ed25519 AAAA pi-deploy-demo",
            "error": "Permission denied (publickey)"
        }))
    }

    #[tokio::test]
    async fn source_check_returns_typed_payload() {
        let app = Router::new().route("/v1/projects/demo/source/check", post(source_check_denied));
        let client = ApiClient::new(spawn_app(app).await);

        let resp = client
            .source_check("demo", "git@github.com:x/y.git")
            .await
            .unwrap()
            .expect("new agent answers");

        assert!(!resp.ok);
        assert_eq!(resp.pubkey.as_deref(), Some("ssh-ed25519 AAAA pi-deploy-demo"));
        assert_eq!(resp.error.as_deref(), Some("Permission denied (publickey)"));
    }

    #[tokio::test]
    async fn source_check_404_means_old_agent_and_returns_none() {
        // Deliberate deviation from the bail-on-404 pattern (spec 2026-07-10):
        // old agents keep working, the fetch stage still prints the key hint.
        let app = Router::new().route("/v1/projects/demo/source/check", post(not_found_plain));
        let client = ApiClient::new(spawn_app(app).await);

        let resp = client
            .source_check("demo", "git@github.com:x/y.git")
            .await
            .unwrap();

        assert!(resp.is_none());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `rtk cargo test --locked -p pi source_check_returns`
Expected: COMPILE ERROR — no method `source_check` on `ApiClient`.

- [ ] **Step 3: Implement**

Extend the proto import at the top of `api.rs` with `SourceCheckRequest, SourceCheckResponse`. Add after `deploy()`:

```rust
    /// Deploy-key preflight. `Ok(None)` — the agent predates the route (bare
    /// 404): callers skip the preflight and deploy as before. Deliberate
    /// deviation from the usual bail-on-404 pattern (spec 2026-07-10): old
    /// agents still print the key hint inside the fetch stage, so degrading
    /// silently keeps first deploys working instead of blocking them.
    pub async fn source_check(
        &self,
        project: &str,
        repo: &str,
    ) -> anyhow::Result<Option<SourceCheckResponse>> {
        let resp = self
            .http
            .post(format!("{}/v1/projects/{project}/source/check", self.base))
            .json(&SourceCheckRequest {
                repo: repo.to_string(),
            })
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(extract_error(resp).await?.json().await?))
    }
```

- [ ] **Step 4: Run tests**

Run: `rtk cargo test --locked -p pi source_check`
Expected: PASS (the two new api tests + Task 2's handler tests).

- [ ] **Step 5: Gate + commit**

Run: `rtk cargo fmt --all -- --check && rtk cargo clippy --all-targets --locked -- -D warnings && rtk cargo test --locked`

```bash
rtk git add crates/bin/src/cli/api.rs
rtk git commit -m "feat(cli): ApiClient::source_check with silent old-agent degradation

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: `cli/sourcekey.rs` — pure helpers

**Files:**
- Create: `crates/bin/src/cli/sourcekey.rs`
- Modify: `crates/bin/src/cli/mod.rs` (add `pub mod sourcekey;` between `setup` and `sse`)
- Test: `crates/bin/src/cli/sourcekey.rs` `mod tests`

**Interfaces:**
- Consumes: `crate::output::{console_style, Sem}` (crate-visible), `crate::duration::format_elapsed`, `console::Emoji`.
- Produces (Task 5 builds on these exact signatures):
  - `pub(crate) fn parse_github_repo(url: &str) -> Option<(String, String)>`
  - `pub(crate) fn gh_register_args(owner: &str, repo: &str, title: &str, pubkey: &str) -> Vec<String>`
  - `pub(crate) fn key_box_lines(repo: &str, error: &str) -> Vec<String>`
  - `pub(crate) fn done_line(label: &str, elapsed: std::time::Duration, interactive: bool) -> String`

- [ ] **Step 1: Create the module with failing tests**

Create `crates/bin/src/cli/sourcekey.rs`:

```rust
//! Deploy-key preflight (spec 2026-07-10): before creating a deployment the
//! CLI verifies the agent can read the SSH repo; on denial it registers the
//! key via local `gh` or shows it with instructions and polls until access
//! works. Pure helpers live at the top, orchestration below.

use console::Emoji;

use crate::output::{console_style, Sem};

static CHECK: Emoji<'_, '_> = Emoji("✓", "ok");
static MARKER: Emoji<'_, '_> = Emoji("▸", ">");
static ARROW: Emoji<'_, '_> = Emoji("→", "->");

/// `git@github.com:owner/repo(.git)` or `ssh://git@github.com/owner/repo(.git)`
/// -> `(owner, repo)`. Anything else (incl. GHES hosts) -> None: manual path.
pub(crate) fn parse_github_repo(url: &str) -> Option<(String, String)> {
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, repo) = rest.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

/// Argv for `gh api` registering a read-only deploy key. Pure for tests.
pub(crate) fn gh_register_args(
    owner: &str,
    repo: &str,
    title: &str,
    pubkey: &str,
) -> Vec<String> {
    vec![
        "api".into(),
        "--method".into(),
        "POST".into(),
        format!("repos/{owner}/{repo}/keys"),
        "-f".into(),
        format!("title={title}"),
        "-f".into(),
        format!("key={pubkey}"),
        "-F".into(),
        "read_only=true".into(),
    ]
}

/// Body of the `deploy key needed` pane. The pubkey itself prints as a plain
/// full-width line above the pane — `LogPane` truncates content to the
/// terminal width and a clipped key can't be copied.
pub(crate) fn key_box_lines(repo: &str, error: &str) -> Vec<String> {
    let mut lines = vec![
        format!("The Pi can't read {repo} yet."),
        "Add the key above to the repository as a read-only deploy key:".to_string(),
    ];
    match parse_github_repo(repo) {
        Some((owner, name)) => {
            lines.push(format!(
                "{ARROW} https://github.com/{owner}/{name}/settings/keys/new"
            ));
            lines.push("  (check nothing extra: read-only is the default)".to_string());
        }
        None => lines.push(format!(
            "{ARROW} add it as a read-only deploy key in your git hosting"
        )),
    }
    lines.push(format!("agent said: {error}"));
    lines
}

/// One-line collapsed step, mirroring the pipeline's stage summary style
/// (`✓ label (elapsed)` interactive, `▸ label ok (elapsed)` otherwise).
pub(crate) fn done_line(label: &str, elapsed: std::time::Duration, interactive: bool) -> String {
    let elapsed = format!("({})", crate::duration::format_elapsed(elapsed));
    if interactive {
        format!(
            "{} {label} {}",
            console_style(Sem::Success).apply_to(CHECK.to_string()),
            console_style(Sem::Muted).apply_to(elapsed),
        )
    } else {
        format!("{MARKER} {label} ok {elapsed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_github_repo_accepts_both_ssh_forms() {
        assert_eq!(
            parse_github_repo("git@github.com:khmil/myapp.git"),
            Some(("khmil".into(), "myapp".into()))
        );
        assert_eq!(
            parse_github_repo("ssh://git@github.com/khmil/myapp.git"),
            Some(("khmil".into(), "myapp".into()))
        );
        assert_eq!(
            parse_github_repo("git@github.com:khmil/myapp"),
            Some(("khmil".into(), "myapp".into())),
            ".git suffix optional"
        );
    }

    #[test]
    fn parse_github_repo_rejects_non_github_and_malformed() {
        assert_eq!(parse_github_repo("https://github.com/khmil/myapp.git"), None);
        assert_eq!(parse_github_repo("git@gitlab.com:khmil/myapp.git"), None);
        assert_eq!(parse_github_repo("git@github.com:justowner"), None);
        assert_eq!(parse_github_repo("git@github.com:a/b/c"), None);
        assert_eq!(parse_github_repo("git@github.com:/x.git"), None);
    }

    #[test]
    fn gh_register_args_post_a_read_only_key() {
        let args = gh_register_args("khmil", "myapp", "pi-deploy-myapp", "ssh-ed25519 AAAA");
        assert_eq!(args[..4], ["api", "--method", "POST", "repos/khmil/myapp/keys"]);
        assert!(args.contains(&"title=pi-deploy-myapp".to_string()));
        assert!(args.contains(&"key=ssh-ed25519 AAAA".to_string()));
        assert!(args.contains(&"read_only=true".to_string()), "never a write key");
    }

    #[test]
    fn key_box_lines_github_variant_links_the_keys_page() {
        let lines = key_box_lines("git@github.com:khmil/myapp.git", "Permission denied");
        let all = lines.join("\n");
        assert!(all.contains("can't read git@github.com:khmil/myapp.git"), "{all}");
        assert!(all.contains("https://github.com/khmil/myapp/settings/keys/new"), "{all}");
        assert!(all.contains("agent said: Permission denied"), "{all}");
    }

    #[test]
    fn key_box_lines_non_github_gives_generic_instruction() {
        let lines = key_box_lines("git@gitlab.com:k/m.git", "denied");
        let all = lines.join("\n");
        assert!(all.contains("read-only deploy key in your git hosting"), "{all}");
        assert!(!all.contains("github.com/"), "{all}");
    }

    #[test]
    fn done_line_non_interactive_is_a_boundary_line() {
        let line = done_line("source access", std::time::Duration::from_secs(1), false);
        assert!(line.contains("source access ok (1.0s)"), "{line}");
    }

    #[test]
    fn done_line_interactive_has_label_and_elapsed() {
        let line = done_line("deploy key added", std::time::Duration::from_secs(83), true);
        assert!(line.contains("deploy key added"), "{line}");
        assert!(line.contains("(1m23s)"), "{line}");
    }
}
```

Register the module in `crates/bin/src/cli/mod.rs` (alphabetical, between `setup` and `sse`):

```rust
pub mod sourcekey;
```

- [ ] **Step 2: Run to verify current state**

Run: `rtk cargo test --locked -p pi sourcekey`
Expected: PASS on first run (module + tests land together; the failing state was the missing module). If anything fails, fix before proceeding.

- [ ] **Step 3: Gate + commit**

Run: `rtk cargo fmt --all -- --check && rtk cargo clippy --all-targets --locked -- -D warnings && rtk cargo test --locked`

Note: clippy may flag the not-yet-used helpers as dead code — if it does, this task may not compile standalone under `-D warnings`. In that case add `#[allow(dead_code)] // wired in the next commit (preflight orchestration)` on the flagged items and remove those allows in Task 5.

```bash
rtk git add crates/bin/src/cli/sourcekey.rs crates/bin/src/cli/mod.rs
rtk git commit -m "feat(cli): sourcekey helpers - github repo parsing, gh args, key box copy

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Preflight orchestration + deploy wiring + `--no-gh-key`

**Files:**
- Modify: `Cargo.toml` (workspace root — tokio `signal` feature)
- Modify: `crates/bin/src/cli/sourcekey.rs` (orchestration + integration tests)
- Modify: `crates/bin/src/cli/commands.rs:15` (deploy signature + preflight call)
- Modify: `crates/bin/src/main.rs` (`Cmd::Deploy` flag + match arm + parse test)
- Test: `crates/bin/src/cli/sourcekey.rs`, `crates/bin/src/main.rs`

**Interfaces:**
- Consumes: `ApiClient::source_check` (Task 3), helpers (Task 4), `output::LogPane` (`new(label, max_visible)`, `push_line`, `clear`), `output::{note, status}`, `console::Term::stdout().features().is_attended()`.
- Produces:
  - `pub async fn preflight(api: &ApiClient, project: &str, repo: &str, no_gh_key: bool) -> anyhow::Result<()>` in `cli::sourcekey`.
  - `cli::commands::deploy(git_ref: Option<String>, no_gh_key: bool, connect: ConnectOpts)` — note the added middle parameter.
  - `rpi deploy --no-gh-key` flag.

- [ ] **Step 1: tokio `signal` feature**

In the workspace root `Cargo.toml`, extend the tokio features list to:

```toml
tokio = { version = "1", features = ["macros", "rt-multi-thread", "process", "io-util", "sync", "time", "net", "fs", "signal"] }
```

- [ ] **Step 2: Write the failing integration tests**

Append to `mod tests` in `crates/bin/src/cli/sourcekey.rs`:

```rust
    use crate::cli::api::ApiClient;
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::Router;

    /// Ephemeral local agent stand-in (same pattern as api.rs tests).
    async fn spawn_app(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn preflight_skips_https_repos_without_calling_the_agent() {
        // port 1 would refuse any request — proves no request is made
        let api = ApiClient::new("http://127.0.0.1:1".into());
        preflight(&api, "demo", "https://github.com/x/y.git", true)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn preflight_skips_when_agent_lacks_the_route() {
        let app = Router::new(); // any request -> bare 404 (old agent)
        let api = ApiClient::new(spawn_app(app).await);
        preflight(&api, "demo", "git@github.com:x/y.git", true)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn preflight_passes_when_access_is_ok() {
        async fn ok() -> impl IntoResponse {
            axum::Json(serde_json::json!({ "ok": true }))
        }
        let app = Router::new().route("/v1/projects/demo/source/check", post(ok));
        let api = ApiClient::new(spawn_app(app).await);
        preflight(&api, "demo", "git@github.com:x/y.git", true)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn preflight_denied_without_tty_bails_with_rerun_hint() {
        // cargo test captures stdout -> is_attended() is false -> the manual
        // path prints the key + instructions and bails instead of polling.
        // (Under `--nocapture` on a real terminal this would poll once, get
        // the same denial... and keep polling to the 10-min cap — run
        // normally.) no_gh_key=true keeps `gh` out of the test.
        async fn denied() -> impl IntoResponse {
            axum::Json(serde_json::json!({
                "ok": false,
                "pubkey": "ssh-ed25519 AAAA pi-deploy-demo",
                "error": "Permission denied (publickey)"
            }))
        }
        let app = Router::new().route("/v1/projects/demo/source/check", post(denied));
        let api = ApiClient::new(spawn_app(app).await);
        let err = preflight(&api, "demo", "git@github.com:x/y.git", true)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("re-run rpi deploy"), "{err}");
    }
```

- [ ] **Step 3: Run to verify failure**

Run: `rtk cargo test --locked -p pi preflight`
Expected: COMPILE ERROR — `preflight` not defined.

- [ ] **Step 4: Implement the orchestration**

Add to `crates/bin/src/cli/sourcekey.rs` (below the pure helpers; extend the imports with `use crate::cli::api::ApiClient;` and `use crate::output;`):

```rust
const POLL_INTERVAL_SECS: u64 = 5;
const POLL_TIMEOUT_SECS: u64 = 600;

/// Deploy-key preflight (spec 2026-07-10): verify the agent can read the
/// repo before creating a deployment. `Ok(())` — proceed with the deploy;
/// `Err` — abort, the explanation is already on screen.
pub async fn preflight(
    api: &ApiClient,
    project: &str,
    repo: &str,
    no_gh_key: bool,
) -> anyhow::Result<()> {
    if !pi_infrastructure::git::is_ssh_repo(repo) {
        return Ok(());
    }
    let started = std::time::Instant::now();
    let interactive = console::Term::stdout().features().is_attended();
    let Some(first) = api.source_check(project, repo).await? else {
        return Ok(()); // old agent: no route; the fetch stage still hints
    };
    if first.ok {
        println!("{}", done_line("source access", started.elapsed(), interactive));
        return Ok(());
    }
    let Some(pubkey) = first.pubkey else {
        anyhow::bail!(
            "agent can't read {repo} and returned no deploy key: {}",
            first.error.as_deref().unwrap_or("unknown error")
        );
    };
    let error = first.error.unwrap_or_else(|| "access denied".to_string());

    if !no_gh_key && try_gh_register(api, project, repo, &pubkey).await? {
        println!(
            "{}",
            done_line("deploy key registered via gh", started.elapsed(), interactive)
        );
        return Ok(());
    }

    // Manual path: full-width copyable key above the pane (LogPane truncates
    // to terminal width), instructions inside it.
    println!("{pubkey}");
    let mut pane = output::LogPane::new("deploy key needed", 12);
    for line in key_box_lines(repo, &error) {
        pane.push_line(&line);
    }
    if !interactive {
        anyhow::bail!("deploy key not registered; add it to the repository and re-run rpi deploy");
    }
    pane.push_line(&format!(
        "waiting for access… (checking every {POLL_INTERVAL_SECS}s, Ctrl+C to abort)"
    ));
    let deadline = started + std::time::Duration::from_secs(POLL_TIMEOUT_SECS);
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                pane.clear();
                anyhow::bail!("aborted; add the deploy key and re-run rpi deploy");
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS)) => {}
        }
        if std::time::Instant::now() >= deadline {
            pane.clear();
            anyhow::bail!(
                "deploy key was not added within 10 minutes; add it and re-run rpi deploy"
            );
        }
        // Transient check failures (tunnel hiccup) keep polling to the deadline.
        if let Ok(Some(resp)) = api.source_check(project, repo).await {
            if resp.ok {
                pane.clear();
                println!("{}", done_line("deploy key added", started.elapsed(), interactive));
                return Ok(());
            }
        }
    }
}

/// GitHub auto-registration. `true` — key registered AND access confirmed.
/// `false` — fall back to the manual box: not a github.com repo, `gh`
/// missing (silent) or logged out / failed (hint printed via output::note).
async fn try_gh_register(
    api: &ApiClient,
    project: &str,
    repo: &str,
    pubkey: &str,
) -> anyhow::Result<bool> {
    let Some((owner, name)) = parse_github_repo(repo) else {
        return Ok(false);
    };
    match gh_logged_in().await {
        None => return Ok(false), // gh not installed
        Some(false) => {
            output::note("gh is not logged in (run: gh auth login) — add the key manually below");
            return Ok(false);
        }
        Some(true) => {}
    }
    output::status(format!(
        "registering read-only deploy key via gh ({owner}/{name})…"
    ));
    let title = format!("pi-deploy-{project}");
    if let Err(e) = gh_register(&owner, &name, &title, pubkey).await {
        output::note(format!("gh couldn't register the key ({e}) — add it manually below"));
        return Ok(false);
    }
    if let Some(resp) = api.source_check(project, repo).await? {
        if resp.ok {
            return Ok(true);
        }
    }
    output::note("key registered but access not confirmed yet — waiting below");
    Ok(false)
}

/// `gh auth token`: cheap, no network. `None` — gh missing; `Some(logged_in)`.
async fn gh_logged_in() -> Option<bool> {
    let out = tokio::process::Command::new("gh")
        .args(["auth", "token"])
        .stdin(std::process::Stdio::null())
        .output()
        .await;
    match out {
        Ok(o) => Some(o.status.success()),
        Err(_) => None,
    }
}

/// POST the deploy key via `gh api`; `Err` carries gh's first stderr line.
async fn gh_register(owner: &str, repo: &str, title: &str, pubkey: &str) -> Result<(), String> {
    let out = tokio::process::Command::new("gh")
        .args(gh_register_args(owner, repo, title, pubkey))
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("gh: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("gh api failed")
        .to_string())
}
```

Remove any `#[allow(dead_code)]` markers left by Task 4.

- [ ] **Step 5: Wire into deploy**

`crates/bin/src/cli/commands.rs` — change the signature at line 15 and insert the preflight between the version block and `let req = DeployRequest {`:

```rust
pub async fn deploy(
    git_ref: Option<String>,
    no_gh_key: bool,
    connect: ConnectOpts,
) -> anyhow::Result<()> {
```

```rust
    crate::cli::sourcekey::preflight(&api, &rpitoml.project.name, &project.repo, no_gh_key)
        .await?;
```

`crates/bin/src/main.rs` — extend `Cmd::Deploy`:

```rust
    Deploy {
        /// Branch or commit-sha (default — branch from rpi.toml)
        #[arg(long = "ref", conflicts_with = "cancel")]
        git_ref: Option<String>,
        /// Cancel the active deploy(s) of the current project instead
        #[arg(long)]
        cancel: bool,
        /// Skip deploy-key auto-registration via GitHub CLI; show the key
        /// for manual setup instead
        #[arg(long, conflicts_with = "cancel")]
        no_gh_key: bool,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
```

and the match arm in `run()`:

```rust
        Cmd::Deploy {
            git_ref,
            cancel,
            no_gh_key,
            connect,
        } => {
            if cancel {
                cli::commands::deploy_cancel(connect).await
            } else {
                cli::commands::deploy(git_ref, no_gh_key, connect).await
            }
        }
```

Parse test in `main.rs` `mod tests` (required by the add-cli-command checklist):

```rust
    #[test]
    fn deploy_no_gh_key_flag_parses() {
        let cli = Cli::try_parse_from(["pi", "deploy", "--no-gh-key"]).unwrap();
        match cli.cmd {
            Cmd::Deploy { no_gh_key, .. } => assert!(no_gh_key),
            _ => panic!("expected deploy"),
        }
        assert!(
            Cli::try_parse_from(["pi", "deploy", "--cancel", "--no-gh-key"]).is_err(),
            "--no-gh-key conflicts with --cancel"
        );
    }
```

- [ ] **Step 6: Run tests**

Run: `rtk cargo test --locked -p pi preflight`
Expected: PASS (4 integration tests).

Run: `rtk cargo test --locked`
Expected: PASS across the workspace.

- [ ] **Step 7: Manual smoke (optional but recommended)**

If a dev agent is reachable (see the rpi-cli skill for local-agent setup): from a project dir with an SSH `rpi.toml` repo, run `rtk cargo run -p pi -- deploy --no-gh-key` and confirm: key line + `deploy key needed` pane appear, Ctrl+C aborts with the re-run hint, exit code 1. Skip if no agent is available — the integration tests cover the logic.

- [ ] **Step 8: Gate + commit**

Run: `rtk cargo fmt --all -- --check && rtk cargo clippy --all-targets --locked -- -D warnings && rtk cargo test --locked`

```bash
rtk git add Cargo.toml Cargo.lock crates/bin/src/cli/sourcekey.rs crates/bin/src/cli/commands.rs crates/bin/src/main.rs
rtk git commit -m "feat(cli): deploy-key preflight - gh auto-registration, manual box with polling

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: Docs — README + spec amendment + final gate

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/specs/2026-07-10-deploy-key-preflight-design.md`

**Interfaces:**
- Consumes: shipped behaviour from Tasks 1–5 (flag name `--no-gh-key`, poll cadence, pubkey-above-pane layout).
- Produces: user-facing docs; spec brought in line with the implementation.

- [ ] **Step 1: README section**

In `README.md`, after the paragraph that describes the v0.17 deploy pipeline view (the `Status:` paragraph near the top mentioning "deploy pipeline view"), add:

```markdown
First deploy of a private SSH repo no longer fails on a missing deploy key:
`rpi deploy` now preflights repo access before starting the pipeline. If the
agent can't read the repo it registers a read-only deploy key through your
local `gh` automatically (the token never leaves your machine; the private
key never leaves the Pi), or — without `gh` — prints the public key with
instructions and continues by itself once you add it (polls every 5 s for up
to 10 min). `--no-gh-key` skips the GitHub API path. Old agents skip the
preflight; the fetch stage still prints the key hint there.
```

- [ ] **Step 2: Spec amendment**

In `docs/superpowers/specs/2026-07-10-deploy-key-preflight-design.md`, section "UX (interactive)": replace the box mockup and its intro to match the shipped layout, and note the two deviations:

- The pubkey prints as a plain full-width line **above** the `deploy key needed` pane (`LogPane` truncates to terminal width; a clipped key can't be copied). The pane references "the key above".
- The happy path prints only the collapsed one-liner `✓ source access (1.2s)` after the check (no live pane during the 1–2 s probe — an empty `LogPane` renders nothing until its first line, so there is nothing to show).

Update the mockup to:

```
ssh-ed25519 AAAAC3Nza… pi-deploy-myapp
╭─ deploy key needed ────────────────────────────────────────╮
│ The Pi can't read git@github.com:khmil/myapp.git yet.      │
│ Add the key above to the repository as a read-only deploy  │
│ key:                                                       │
│ → https://github.com/khmil/myapp/settings/keys/new         │
│   (check nothing extra: read-only is the default)          │
│ agent said: Permission denied (publickey)                  │
│ waiting for access… (checking every 5s, Ctrl+C to abort)   │
╰────────────────────────────────────────────────────────────╯
```

- [ ] **Step 3: Full gate + commit**

Run: `rtk cargo fmt --all -- --check && rtk cargo clippy --all-targets --locked -- -D warnings && rtk cargo test --locked`
Expected: all clean/green.

```bash
rtk git add README.md docs/superpowers/specs/2026-07-10-deploy-key-preflight-design.md
rtk git commit -m "docs: deploy-key preflight in README, spec UX amendment

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```
