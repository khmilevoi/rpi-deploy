# Cloudflare Tunnel Auto-Bootstrap — Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `sudo rpi agent setup --with-cloudflared --cf-token <t> --domain <zone>` bootstrap a working Cloudflare Tunnel end-to-end and idempotently — no interactive `cloudflared tunnel login`, no `cert.pem`, none of the 7 runbook pitfalls — while landing the shared foundation (Cloudflare API client, single-token secret model, `agent.toml` `schema` + `[cloudflare]`, unified migration framework) that Phase 2 (Caddy/LAN) will build on.

**Architecture:** A `CloudflareApi` port (domain) with an HTTP adapter (`infrastructure/cloudflare.rs`) owns the token and does tunnel create + DNS writes via the Cloudflare REST API; the running tunnel needs only the credentials JSON the tool constructs, so `cert.pem` is never involved. A detect-oriented migration framework (`bin/agent/migrate.rs`) with a `state.db` ledger runs all host migrations uniformly under `rpi agent migrate`; the existing `pi-agent → rpi-agent` migration is refactored into it. `bin/agent/setup.rs` orchestrates the bootstrap through the existing `Sys` trait (testable off-Linux via `FakeSys`).

**Tech Stack:** Rust (workspace crates `domain`/`application`/`infrastructure`/`bin`), `reqwest` (rustls) for HTTP, `rusqlite` + `rusqlite_migration` for the ledger, `serde`/`serde_json`/`toml`, `async-trait`, `mockall` (dev), `tempfile` (dev). Adds `rand = "0.8"` to `infrastructure` for the tunnel secret.

## Global Constraints

- **CI gate (must pass before every commit; run with `rtk`):**
  `rtk cargo fmt --all -- --check` · `rtk cargo clippy --all-targets --locked -- -D warnings` · `rtk cargo test --locked`. If fmt reports a diff, run `rtk cargo fmt --all` — never hand-format.
- **RTK:** prefix every shell command with `rtk`, including inside `&&` chains.
- **No `cert.pem`, no `cloudflared tunnel login`.** The running tunnel reads only the credentials JSON `{AccountTag, TunnelID, TunnelName, TunnelSecret}`. Management (create, DNS) is via the Cloudflare API with the token.
- **Token scopes (doc only):** `Zone:DNS:Edit` + `Zone:Zone:Read` + `Account:Cloudflare Tunnel:Edit`.
- **Canonical paths:** agent data `/var/lib/rpi`, config `/etc/rpi/agent.toml`, tunnel dir `/var/lib/rpi/cloudflared`, token `/var/lib/rpi/cloudflare/token`, binaries `/usr/local/bin/{rpi,cloudflared}`.
- **systemd `--user` fix is a drop-in**, never `usermod -d`: `/etc/systemd/system/user@<uid>.service.d/override.conf` with `XDG_CONFIG_HOME=/var/lib/rpi/.config` and `HOME=/var/lib/rpi`.
- **`config.yml` is written with spaces only** (never tabs) and validated with `cloudflared tunnel ingress validate <path>`.
- **`agent.toml` carries `schema = 1`** (u32), validated like `rpi.toml` (current OK, future = error, older = config-migration).
- **Idempotent, adopt-or-create.** Re-running setup adopts existing state; divergent managed files are backed up to `*.bak` (existing `write_unit_with_backup` convention).
- **All bootstrap OS effects go through the `Sys` trait** so tests use `FakeSys`. Tests must run on Windows/macOS/Linux.
- **Phase boundary:** do NOT implement Caddy, `[lan]`, `lan_hostname`, `CaddyIngress`, or the `nginx-to-caddy` migration here — those are Phase 2. The migration *framework* lands here with only `pi-to-rpi` registered.
- Spec: `docs/superpowers/specs/2026-07-09-rpi-cloudflare-lan-automation-design.md`.

---

## File Structure

**Created:**
- `crates/infrastructure/src/cloudflare.rs` — `CloudflareApi` HTTP adapter + pure helpers (credentials JSON, tunnel secret, request/response shapes).
- `crates/infrastructure/src/migrations.rs` — `MigrationLedger` (records applied host-migration ids in `state.db`).
- `crates/bin/src/agent/migrate.rs` — `Migration` trait, `MigrationState`/`MigrationOutcome`, registry, runner, `rpi agent migrate` entrypoint; the `pi-to-rpi` migration.

**Modified:**
- `crates/domain/src/contracts.rs` — add `CloudflareApi` port (+ automock).
- `crates/domain/src/error.rs` — reuse `DomainError::Ingress`; no new variant unless a test needs it.
- `crates/infrastructure/src/sqlite.rs` — new `M::up` for the `applied_migrations` ledger table.
- `crates/infrastructure/src/lib.rs` — `pub mod cloudflare; pub mod migrations;`.
- `crates/infrastructure/src/cloudflared.rs` — `CloudflaredIngress` gains a `CloudflareApi` dep; `route_dns_and_restart` calls `put_dns` instead of shelling `cloudflared tunnel route dns`.
- `crates/infrastructure/Cargo.toml` — add `rand`.
- `crates/bin/src/agent/config.rs` — `schema: u32` + validation; `CloudflareSection { zone, token_file, account_id }`.
- `crates/bin/src/agent/setup.rs` — full tunnel bootstrap: install cloudflared, create tunnel via API, write creds JSON + `config.yml` (+ validate), write `[cloudflare]`/`[cloudflared]`, systemd drop-in; step 0 calls the migration runner instead of `migrate_pi_agent_if_present` directly. The pi→rpi logic moves to `migrate.rs`.
- `crates/bin/src/agent/mod.rs` — `pub mod migrate;`.
- `crates/bin/src/agent/state.rs` — wire `HttpCloudflare` into `CloudflaredIngress`; read the token.
- `crates/bin/src/main.rs` — `AgentCmd::Setup` new flags; new `AgentCmd::Migrate`; dispatch.

---

## Task 1: Migration ledger table + `MigrationLedger`

**Files:**
- Modify: `crates/infrastructure/src/sqlite.rs` (add migration; ~line 42)
- Create: `crates/infrastructure/src/migrations.rs`
- Modify: `crates/infrastructure/src/lib.rs`

**Interfaces:**
- Produces: `MigrationLedger::new(db: Db) -> MigrationLedger`; `async fn is_applied(&self, id: &str) -> Result<bool, DomainError>`; `async fn mark_applied(&self, id: &str, at_unix: i64) -> Result<(), DomainError>`; `async fn applied(&self) -> Result<Vec<String>, DomainError>`.
- Consumes: `Db` and `storage_err` from `sqlite.rs`.

- [ ] **Step 1: Add the ledger table migration.** In `crates/infrastructure/src/sqlite.rs`, append one `M::up` to the vec in `migrations()` (after the `commands` migration):

```rust
        M::up(
            r#"
        CREATE TABLE applied_migrations (
            id         TEXT PRIMARY KEY,
            applied_at INTEGER NOT NULL
        );
        "#,
        ),
```

- [ ] **Step 2: Write the failing test for the ledger.** Create `crates/infrastructure/src/migrations.rs`:

```rust
use crate::sqlite::{storage_err, Db};
use pi_domain::error::DomainError;

/// Records which host-level migrations (§7) have been applied, in state.db.
#[derive(Clone)]
pub struct MigrationLedger {
    db: Db,
}

impl MigrationLedger {
    pub fn new(db: Db) -> MigrationLedger {
        MigrationLedger { db }
    }

    pub async fn is_applied(&self, id: &str) -> Result<bool, DomainError> {
        let id = id.to_string();
        self.db
            .call(move |c| {
                let n: i64 = c
                    .query_row(
                        "SELECT count(*) FROM applied_migrations WHERE id = ?1",
                        [&id],
                        |r| r.get(0),
                    )
                    .map_err(storage_err)?;
                Ok(n > 0)
            })
            .await
    }

    pub async fn mark_applied(&self, id: &str, at_unix: i64) -> Result<(), DomainError> {
        let id = id.to_string();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT OR IGNORE INTO applied_migrations (id, applied_at) VALUES (?1, ?2)",
                    rusqlite::params![id, at_unix],
                )
                .map_err(storage_err)?;
                Ok(())
            })
            .await
    }

    pub async fn applied(&self) -> Result<Vec<String>, DomainError> {
        self.db
            .call(|c| {
                let mut stmt = c
                    .prepare("SELECT id FROM applied_migrations ORDER BY applied_at")
                    .map_err(storage_err)?;
                let ids = stmt
                    .query_map([], |r| r.get::<_, String>(0))
                    .map_err(storage_err)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(storage_err)?;
                Ok(ids)
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn ledger() -> MigrationLedger {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("state.db")).unwrap();
        // keep tempdir alive by leaking it into the DB's lifetime for the test
        std::mem::forget(dir);
        MigrationLedger::new(db)
    }

    #[tokio::test]
    async fn unknown_id_is_not_applied() {
        let l = ledger().await;
        assert!(!l.is_applied("pi-to-rpi").await.unwrap());
    }

    #[tokio::test]
    async fn mark_then_is_applied_and_listed() {
        let l = ledger().await;
        l.mark_applied("pi-to-rpi", 100).await.unwrap();
        assert!(l.is_applied("pi-to-rpi").await.unwrap());
        assert_eq!(l.applied().await.unwrap(), vec!["pi-to-rpi".to_string()]);
    }

    #[tokio::test]
    async fn mark_is_idempotent() {
        let l = ledger().await;
        l.mark_applied("pi-to-rpi", 100).await.unwrap();
        l.mark_applied("pi-to-rpi", 200).await.unwrap();
        assert_eq!(l.applied().await.unwrap().len(), 1);
    }
}
```

- [ ] **Step 3: Register the module.** In `crates/infrastructure/src/lib.rs` add `pub mod migrations;` (alphabetical with the other `pub mod` lines).

- [ ] **Step 4: Run the tests.** Run: `rtk cargo test -p pi-infrastructure migrations`
Expected: PASS (3 tests) plus the existing `sqlite` tests still green.

- [ ] **Step 5: Commit.**

```bash
rtk git add crates/infrastructure/src/migrations.rs crates/infrastructure/src/sqlite.rs crates/infrastructure/src/lib.rs
rtk git commit -m "feat(infra): applied_migrations ledger in state.db"
```

---

## Task 2: `Migration` trait + registry + runner

**Files:**
- Create: `crates/bin/src/agent/migrate.rs`
- Modify: `crates/bin/src/agent/mod.rs` (add `pub mod migrate;`)

**Interfaces:**
- Consumes: `Sys` (from `agent::setup`), `SetupReport` (from `agent::setup`), `MigrationLedger` (Task 1).
- Produces:
  - `enum MigrationState { Applicable, Done, NotApplicable }`
  - `struct MigrationOutcome { pub changed: bool, pub note: String }`
  - `#[async_trait] trait Migration { fn id(&self)->&str; fn description(&self)->&str; fn disruptive(&self)->bool; async fn detect(&self, sys:&dyn Sys)->MigrationState; async fn apply(&self, sys:&dyn Sys, dry_run:bool)->Result<MigrationOutcome,String>; }`
  - `async fn run_auto(sys:&dyn Sys, ledger:&MigrationLedger, registry:&[Box<dyn Migration>], dry:bool, rep:&mut SetupReport)` — applies non-disruptive Applicable migrations, records them, reports disruptive Applicable ones as an actionable note.
  - `async fn run_explicit(sys:&dyn Sys, ledger:&MigrationLedger, registry:&[Box<dyn Migration>], ids:&[String], dry:bool, rep:&mut SetupReport)` — applies the named migrations regardless of disruptive.

- [ ] **Step 1: Write the failing test.** Create `crates/bin/src/agent/migrate.rs` with the types, a test-only fake migration, and tests. (The `Sys`/`FakeSys` and `SetupReport` come from `super::setup`.)

```rust
use super::setup::{Sys, SetupReport};
use crate::agent::migrate_ledger::LedgerHandle;
use async_trait::async_trait;

#[derive(Debug, PartialEq, Eq)]
pub enum MigrationState {
    Applicable,
    Done,
    NotApplicable,
}

pub struct MigrationOutcome {
    pub changed: bool,
    pub note: String,
}

#[async_trait]
pub trait Migration: Send + Sync {
    fn id(&self) -> &str;
    fn description(&self) -> &str;
    fn disruptive(&self) -> bool;
    async fn detect(&self, sys: &dyn Sys) -> MigrationState;
    async fn apply(&self, sys: &dyn Sys, dry_run: bool) -> Result<MigrationOutcome, String>;
}

/// Applies every non-disruptive Applicable migration and records it; for a
/// disruptive Applicable one, prints an actionable note instead of applying.
pub async fn run_auto(
    sys: &dyn Sys,
    ledger: &dyn LedgerHandle,
    registry: &[Box<dyn Migration>],
    dry: bool,
    rep: &mut SetupReport,
) {
    for m in registry {
        if ledger.is_applied(m.id()).await {
            continue;
        }
        match m.detect(sys).await {
            MigrationState::Done | MigrationState::NotApplicable => {}
            MigrationState::Applicable if m.disruptive() => {
                rep.warnings.push(format!(
                    "migration available: `{}` ({}). Run: sudo rpi agent migrate --run {}",
                    m.id(),
                    m.description(),
                    m.id()
                ));
            }
            MigrationState::Applicable => apply_one(sys, ledger, m.as_ref(), dry, rep).await,
        }
    }
}

/// Applies the named migrations regardless of `disruptive`.
pub async fn run_explicit(
    sys: &dyn Sys,
    ledger: &dyn LedgerHandle,
    registry: &[Box<dyn Migration>],
    ids: &[String],
    dry: bool,
    rep: &mut SetupReport,
) {
    for id in ids {
        match registry.iter().find(|m| m.id() == id) {
            None => rep.errors.push(format!("unknown migration: {id}")),
            Some(m) => match m.detect(sys).await {
                MigrationState::Done => rep.skipped.push(format!("migration {id} (already done)")),
                MigrationState::NotApplicable => {
                    rep.skipped.push(format!("migration {id} (not applicable)"))
                }
                MigrationState::Applicable => apply_one(sys, ledger, m.as_ref(), dry, rep).await,
            },
        }
    }
}

async fn apply_one(
    sys: &dyn Sys,
    ledger: &dyn LedgerHandle,
    m: &dyn Migration,
    dry: bool,
    rep: &mut SetupReport,
) {
    match m.apply(sys, dry).await {
        Ok(out) => {
            if !dry {
                ledger.mark_applied(m.id()).await;
            }
            rep.repaired
                .push(format!("migration {}: {}", m.id(), out.note));
        }
        Err(e) => rep.errors.push(format!("migration {} failed: {e}", m.id())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::setup::fake::FakeSys;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeLedger {
        applied: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl LedgerHandle for FakeLedger {
        async fn is_applied(&self, id: &str) -> bool {
            self.applied.lock().unwrap().iter().any(|a| a == id)
        }
        async fn mark_applied(&self, id: &str) {
            self.applied.lock().unwrap().push(id.to_string());
        }
    }

    struct Stub {
        id: &'static str,
        disruptive: bool,
        state: MigrationState,
    }
    #[async_trait]
    impl Migration for Stub {
        fn id(&self) -> &str {
            self.id
        }
        fn description(&self) -> &str {
            "stub"
        }
        fn disruptive(&self) -> bool {
            self.disruptive
        }
        async fn detect(&self, _sys: &dyn Sys) -> MigrationState {
            match self.state {
                MigrationState::Applicable => MigrationState::Applicable,
                MigrationState::Done => MigrationState::Done,
                MigrationState::NotApplicable => MigrationState::NotApplicable,
            }
        }
        async fn apply(&self, _sys: &dyn Sys, _dry: bool) -> Result<MigrationOutcome, String> {
            Ok(MigrationOutcome {
                changed: true,
                note: "applied".into(),
            })
        }
    }

    #[tokio::test]
    async fn auto_applies_nondisruptive_and_records() {
        let reg: Vec<Box<dyn Migration>> = vec![Box::new(Stub {
            id: "safe",
            disruptive: false,
            state: MigrationState::Applicable,
        })];
        let ledger = FakeLedger::default();
        let mut rep = SetupReport::default();
        run_auto(&FakeSys::default(), &ledger, &reg, false, &mut rep).await;
        assert!(rep.repaired.iter().any(|r| r.contains("migration safe")));
        assert!(ledger.is_applied("safe").await);
    }

    #[tokio::test]
    async fn auto_only_reports_disruptive() {
        let reg: Vec<Box<dyn Migration>> = vec![Box::new(Stub {
            id: "nginx-to-caddy",
            disruptive: true,
            state: MigrationState::Applicable,
        })];
        let ledger = FakeLedger::default();
        let mut rep = SetupReport::default();
        run_auto(&FakeSys::default(), &ledger, &reg, false, &mut rep).await;
        assert!(rep
            .warnings
            .iter()
            .any(|w| w.contains("--run nginx-to-caddy")));
        assert!(!ledger.is_applied("nginx-to-caddy").await, "not auto-applied");
    }

    #[tokio::test]
    async fn explicit_applies_disruptive() {
        let reg: Vec<Box<dyn Migration>> = vec![Box::new(Stub {
            id: "nginx-to-caddy",
            disruptive: true,
            state: MigrationState::Applicable,
        })];
        let ledger = FakeLedger::default();
        let mut rep = SetupReport::default();
        run_explicit(
            &FakeSys::default(),
            &ledger,
            &reg,
            &["nginx-to-caddy".to_string()],
            false,
            &mut rep,
        )
        .await;
        assert!(ledger.is_applied("nginx-to-caddy").await);
    }

    #[tokio::test]
    async fn explicit_unknown_is_error() {
        let reg: Vec<Box<dyn Migration>> = vec![];
        let ledger = FakeLedger::default();
        let mut rep = SetupReport::default();
        run_explicit(&FakeSys::default(), &ledger, &reg, &["nope".into()], false, &mut rep).await;
        assert!(rep.errors.iter().any(|e| e.contains("unknown migration")));
    }
}
```

- [ ] **Step 2: Add the `LedgerHandle` seam.** The runner must be testable without a real DB, and `MigrationLedger` (Task 1) is async-over-DB. Create the trait in a tiny module `crates/bin/src/agent/migrate_ledger.rs`:

```rust
use async_trait::async_trait;
use pi_infrastructure::migrations::MigrationLedger;

/// Thin async seam over MigrationLedger so the runner is unit-testable with a fake.
#[async_trait]
pub trait LedgerHandle: Send + Sync {
    async fn is_applied(&self, id: &str) -> bool;
    async fn mark_applied(&self, id: &str);
}

pub struct DbLedger {
    inner: MigrationLedger,
}

impl DbLedger {
    pub fn new(inner: MigrationLedger) -> DbLedger {
        DbLedger { inner }
    }
}

#[async_trait]
impl LedgerHandle for DbLedger {
    async fn is_applied(&self, id: &str) -> bool {
        self.inner.is_applied(id).await.unwrap_or(false)
    }
    async fn mark_applied(&self, id: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let _ = self.inner.mark_applied(id, now).await;
    }
}
```

- [ ] **Step 3: Register modules.** In `crates/bin/src/agent/mod.rs` add `pub mod migrate;` and `pub mod migrate_ledger;`.

- [ ] **Step 4: Run the tests.** Run: `rtk cargo test -p pi migrate::`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit.**

```bash
rtk git add crates/bin/src/agent/migrate.rs crates/bin/src/agent/migrate_ledger.rs crates/bin/src/agent/mod.rs
rtk git commit -m "feat(agent): migration framework (trait, registry, runner, ledger seam)"
```

---

## Task 3: Refactor `pi-to-rpi` into a `Migration`

**Files:**
- Modify: `crates/bin/src/agent/setup.rs` (extract `migrate_pi_agent_if_present` body)
- Modify: `crates/bin/src/agent/migrate.rs` (add `PiToRpi` impl + registry)

**Interfaces:**
- Consumes: existing helpers in `setup.rs` (`user_exists`, `rewrite_owned_paths`, constants `OLD_UNIT_PATH`, `CLOUDFLARED_UNIT_PATH`, `CLOUDFLARED_CONFIG_PATH`, `AGENT_TOML_PATH`). Make these `pub(crate)` so `migrate.rs` can call them.
- Produces: `pub struct PiToRpi;` implementing `Migration` (id `"pi-to-rpi"`, `disruptive() == false`); `pub fn registry() -> Vec<Box<dyn Migration>>` returning `vec![Box::new(PiToRpi)]`.

- [ ] **Step 1: Expose the reused items.** In `setup.rs`, change `const OLD_UNIT_PATH`, `const CLOUDFLARED_UNIT_PATH`, `const CLOUDFLARED_CONFIG_PATH` and `fn rewrite_owned_paths`, `async fn user_exists` to `pub(crate)`. (They are currently module-private / already `pub`.)

- [ ] **Step 2: Write the failing test in `migrate.rs`.** Add a test that a legacy install is detected `Applicable` and a fresh one `NotApplicable`, reusing `setup::fake::FakeSys` seeded the way `setup::tests::legacy_sys()` does. Add:

```rust
#[cfg(test)]
mod pi_to_rpi_tests {
    use super::*;
    use super::super::setup::fake::FakeSys;

    fn legacy() -> FakeSys {
        let mut sys = FakeSys::default();
        sys.err.insert(FakeSys::key("id", &["-u", "rpi-agent"]));
        sys.ok
            .insert(FakeSys::key("id", &["-u", "pi-agent"]), "999".into());
        sys
    }

    #[tokio::test]
    async fn detects_legacy_install() {
        assert_eq!(PiToRpi.detect(&legacy()).await, MigrationState::Applicable);
    }

    #[tokio::test]
    async fn fresh_install_not_applicable() {
        let mut sys = FakeSys::default();
        sys.err.insert(FakeSys::key("id", &["-u", "rpi-agent"]));
        sys.err.insert(FakeSys::key("id", &["-u", "pi-agent"]));
        assert_eq!(PiToRpi.detect(&sys).await, MigrationState::NotApplicable);
    }
}
```

- [ ] **Step 3: Implement `PiToRpi`.** In `migrate.rs`, add:

```rust
use super::setup;

pub struct PiToRpi;

#[async_trait]
impl Migration for PiToRpi {
    fn id(&self) -> &str {
        "pi-to-rpi"
    }
    fn description(&self) -> &str {
        "rename legacy pi-agent install to rpi-agent (user, group, /var/lib, /etc, /var/log)"
    }
    fn disruptive(&self) -> bool {
        false
    }
    async fn detect(&self, sys: &dyn Sys) -> MigrationState {
        if setup::user_exists(sys, "rpi-agent").await {
            MigrationState::NotApplicable // already migrated / fresh
        } else if setup::user_exists(sys, "pi-agent").await {
            MigrationState::Applicable
        } else {
            MigrationState::NotApplicable
        }
    }
    async fn apply(&self, sys: &dyn Sys, dry_run: bool) -> Result<MigrationOutcome, String> {
        let mut rep = SetupReport::default();
        setup::migrate_pi_agent(sys, dry_run, &mut rep).await;
        if let Some(e) = rep.errors.first() {
            return Err(e.clone());
        }
        Ok(MigrationOutcome {
            changed: true,
            note: "pi-agent -> rpi-agent".into(),
        })
    }
}

pub fn registry() -> Vec<Box<dyn Migration>> {
    vec![Box::new(PiToRpi)]
}
```

- [ ] **Step 4: Rename & keep the body.** In `setup.rs`, rename `migrate_pi_agent_if_present` to `pub(crate) async fn migrate_pi_agent` and **remove its two leading early-returns** (`if user_exists(rpi-agent) return; if !user_exists(pi-agent) return;`) since `detect()` now guards them — but keep the `dry` early-return that pushes the "dry run" repaired note. Leave the existing `setup.rs` unit tests that call it: update them to call `migrate_pi_agent`. The behavior for a legacy install is unchanged.

- [ ] **Step 5: Run tests.** Run: `rtk cargo test -p pi migrate:: && rtk cargo test -p pi setup::`
Expected: PASS (new pi-to-rpi tests + existing setup migration tests, adjusted to the new name).

- [ ] **Step 6: Commit.**

```bash
rtk git add crates/bin/src/agent/migrate.rs crates/bin/src/agent/setup.rs
rtk git commit -m "refactor(agent): pi-to-rpi becomes a registered Migration"
```

---

## Task 4: `agent.toml` `schema` + `[cloudflare]` section

**Files:**
- Modify: `crates/bin/src/agent/config.rs`

**Interfaces:**
- Produces: `AgentConfig.schema: u32` (serde default `1`); `AgentConfig.cloudflare: Option<CloudflareSection>`; `pub struct CloudflareSection { pub zone: String, pub token_file: PathBuf, pub account_id: Option<String> }`. `AgentConfig::parse` errors when `schema > CURRENT_SCHEMA`.
- Consumes: existing `AgentConfig::parse`.

- [ ] **Step 1: Write the failing tests.** In `config.rs` `mod tests`, add:

```rust
#[test]
fn schema_defaults_to_current_when_absent() {
    let config = AgentConfig::parse("").unwrap();
    assert_eq!(config.schema, 1);
}

#[test]
fn rejects_future_schema() {
    let err = AgentConfig::parse("schema = 2").unwrap_err().to_string();
    assert!(err.contains("schema"), "got: {err}");
}

#[test]
fn cloudflare_section_parses() {
    let cfg = AgentConfig::parse(
        "[cloudflare]\nzone = \"example.com\"\ntoken_file = \"/var/lib/rpi/cloudflare/token\"",
    )
    .unwrap();
    let cf = cfg.cloudflare.unwrap();
    assert_eq!(cf.zone, "example.com");
    assert_eq!(cf.account_id, None);
}
```

- [ ] **Step 2: Run to verify failure.** Run: `rtk cargo test -p pi config::tests::schema_defaults_to_current_when_absent`
Expected: FAIL (no field `schema`).

- [ ] **Step 3: Implement.** In `config.rs`:

Add the constant and struct:

```rust
pub const CURRENT_SCHEMA: u32 = 1;

fn default_schema() -> u32 {
    CURRENT_SCHEMA
}

#[derive(Debug, Deserialize)]
pub struct CloudflareSection {
    pub zone: String,
    pub token_file: PathBuf,
    #[serde(default)]
    pub account_id: Option<String>,
}
```

Add fields to `AgentConfig`:

```rust
    #[serde(default = "default_schema")]
    pub schema: u32,
    pub cloudflare: Option<CloudflareSection>,
```

In `AgentConfig::parse`, after `toml::from_str`, validate the schema before returning:

```rust
    pub fn parse(text: &str) -> anyhow::Result<AgentConfig> {
        let config: AgentConfig = toml::from_str(text)?;
        if config.schema > CURRENT_SCHEMA {
            anyhow::bail!(
                "unsupported agent.toml schema {} (this rpi supports schema {})",
                config.schema,
                CURRENT_SCHEMA
            );
        }
        config.stage_timeouts()?;
        Ok(config)
    }
```

- [ ] **Step 4: Run tests.** Run: `rtk cargo test -p pi config::`
Expected: PASS (all existing config tests + 3 new).

- [ ] **Step 5: Commit.**

```bash
rtk git add crates/bin/src/agent/config.rs
rtk git commit -m "feat(agent): agent.toml schema field + [cloudflare] section"
```

---

## Task 5: `CloudflareApi` port + HTTP adapter

**Files:**
- Modify: `crates/domain/src/contracts.rs` (new trait)
- Create: `crates/infrastructure/src/cloudflare.rs`
- Modify: `crates/infrastructure/src/lib.rs`, `crates/infrastructure/Cargo.toml`

**Interfaces:**
- Produces (domain):

```rust
pub struct TunnelCreds {
    pub account_tag: String,
    pub tunnel_id: String,
    pub tunnel_name: String,
    pub tunnel_secret: String, // base64
}

#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait CloudflareApi: Send + Sync {
    async fn zone_id(&self, zone: &str) -> Result<String, DomainError>;
    /// Adopt an existing tunnel by name, else create it. Returns creds.
    async fn find_or_create_tunnel(&self, name: &str) -> Result<TunnelCreds, DomainError>;
    /// Upsert a proxied CNAME <name> -> <tunnel_id>.cfargotunnel.com.
    async fn put_tunnel_cname(&self, zone: &str, name: &str, tunnel_id: &str)
        -> Result<(), DomainError>;
}
```

- Produces (infra): `HttpCloudflare::new(token: String, account_id: Option<String>) -> HttpCloudflare` implementing `CloudflareApi`; pure helper `pub fn credentials_json(creds: &TunnelCreds) -> String`.
- Consumes: `reqwest`, `serde_json`, `base64`, `rand`.

- [ ] **Step 1: Add the dep.** In `crates/infrastructure/Cargo.toml` `[dependencies]` add `rand = "0.8"`.

- [ ] **Step 2: Add the trait to the domain.** In `crates/domain/src/contracts.rs`, add `TunnelCreds` and the `CloudflareApi` trait shown above (place near `Ingress`; import nothing new beyond `DomainError`, `async_trait`, and the existing `automock` cfg).

- [ ] **Step 3: Write the failing test for the pure helper.** Create `crates/infrastructure/src/cloudflare.rs`:

```rust
use async_trait::async_trait;
use base64::Engine;
use pi_domain::contracts::{CloudflareApi, TunnelCreds};
use pi_domain::error::DomainError;
use rand::RngCore;

const API: &str = "https://api.cloudflare.com/client/v4";

fn api_err(msg: impl std::fmt::Display) -> DomainError {
    DomainError::Ingress(format!("cloudflare api: {msg}"))
}

/// The credentials file a locally-managed tunnel reads at runtime (no cert.pem).
pub fn credentials_json(creds: &TunnelCreds) -> String {
    serde_json::json!({
        "AccountTag": creds.account_tag,
        "TunnelID": creds.tunnel_id,
        "TunnelName": creds.tunnel_name,
        "TunnelSecret": creds.tunnel_secret,
    })
    .to_string()
}

/// 32 random bytes, base64-standard — the tunnel secret shared with create.
pub fn new_tunnel_secret() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::STANDARD.encode(buf)
}

pub struct HttpCloudflare {
    token: String,
    account_id: Option<String>,
    client: reqwest::Client,
    base: String,
}

impl HttpCloudflare {
    pub fn new(token: String, account_id: Option<String>) -> HttpCloudflare {
        HttpCloudflare {
            token,
            account_id,
            client: reqwest::Client::new(),
            base: API.to_string(),
        }
    }
}

#[async_trait]
impl CloudflareApi for HttpCloudflare {
    async fn zone_id(&self, zone: &str) -> Result<String, DomainError> {
        let v: serde_json::Value = self
            .client
            .get(format!("{}/zones?name={zone}", self.base))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(api_err)?
            .json()
            .await
            .map_err(api_err)?;
        v["result"][0]["id"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| api_err(format!("zone {zone} not found")))
    }

    async fn find_or_create_tunnel(&self, name: &str) -> Result<TunnelCreds, DomainError> {
        let account = self
            .account_id
            .clone()
            .ok_or_else(|| api_err("account_id required"))?;
        // adopt existing by name
        let list: serde_json::Value = self
            .client
            .get(format!(
                "{}/accounts/{account}/cfd_tunnel?name={name}&is_deleted=false",
                self.base
            ))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(api_err)?
            .json()
            .await
            .map_err(api_err)?;
        if let Some(id) = list["result"][0]["id"].as_str() {
            // Existing tunnel: we cannot recover its secret, so the caller must
            // already hold creds. Signal adoption with an empty secret.
            return Ok(TunnelCreds {
                account_tag: account,
                tunnel_id: id.to_string(),
                tunnel_name: name.to_string(),
                tunnel_secret: String::new(),
            });
        }
        let secret = new_tunnel_secret();
        let created: serde_json::Value = self
            .client
            .post(format!("{}/accounts/{account}/cfd_tunnel", self.base))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({ "name": name, "tunnel_secret": secret }))
            .send()
            .await
            .map_err(api_err)?
            .json()
            .await
            .map_err(api_err)?;
        let id = created["result"]["id"]
            .as_str()
            .ok_or_else(|| api_err("create tunnel: no id in response"))?;
        Ok(TunnelCreds {
            account_tag: account,
            tunnel_id: id.to_string(),
            tunnel_name: name.to_string(),
            tunnel_secret: secret,
        })
    }

    async fn put_tunnel_cname(
        &self,
        zone: &str,
        name: &str,
        tunnel_id: &str,
    ) -> Result<(), DomainError> {
        let zid = self.zone_id(zone).await?;
        let content = format!("{tunnel_id}.cfargotunnel.com");
        // find existing record id
        let existing: serde_json::Value = self
            .client
            .get(format!(
                "{}/zones/{zid}/dns_records?type=CNAME&name={name}",
                self.base
            ))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(api_err)?
            .json()
            .await
            .map_err(api_err)?;
        let body = serde_json::json!({
            "type": "CNAME", "name": name, "content": content, "proxied": true
        });
        let req = match existing["result"][0]["id"].as_str() {
            Some(rid) => self
                .client
                .put(format!("{}/zones/{zid}/dns_records/{rid}", self.base)),
            None => self.client.post(format!("{}/zones/{zid}/dns_records", self.base)),
        };
        req.bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(api_err)?
            .error_for_status()
            .map_err(api_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_json_has_the_four_fields() {
        let creds = TunnelCreds {
            account_tag: "acc".into(),
            tunnel_id: "tid".into(),
            tunnel_name: "myboard".into(),
            tunnel_secret: "c2VjcmV0".into(),
        };
        let v: serde_json::Value = serde_json::from_str(&credentials_json(&creds)).unwrap();
        assert_eq!(v["AccountTag"], "acc");
        assert_eq!(v["TunnelID"], "tid");
        assert_eq!(v["TunnelName"], "myboard");
        assert_eq!(v["TunnelSecret"], "c2VjcmV0");
    }

    #[test]
    fn tunnel_secret_is_32_bytes_base64() {
        let s = new_tunnel_secret();
        let raw = base64::engine::general_purpose::STANDARD.decode(s).unwrap();
        assert_eq!(raw.len(), 32);
    }
}
```

- [ ] **Step 4: Register the module.** In `crates/infrastructure/src/lib.rs` add `pub mod cloudflare;`.

- [ ] **Step 5: Run the tests.** Run: `rtk cargo test -p pi-infrastructure cloudflare`
Expected: PASS (2 tests). (HTTP methods are covered by the consumer mock in Task 6 and the integration on the Pi — no live API in unit tests.)

- [ ] **Step 6: Commit.**

```bash
rtk git add crates/domain/src/contracts.rs crates/infrastructure/src/cloudflare.rs crates/infrastructure/src/lib.rs crates/infrastructure/Cargo.toml
rtk git commit -m "feat(infra): CloudflareApi port + HTTP adapter (tunnel create, CNAME, creds json)"
```

---

## Task 6: DNS-via-API in `CloudflaredIngress`

**Files:**
- Modify: `crates/infrastructure/src/cloudflared.rs`
- Modify: `crates/bin/src/agent/state.rs`
- Modify: `crates/bin/src/agent/config.rs` (`CloudflaredSection` gains nothing; the zone comes from `[cloudflare]`)

**Interfaces:**
- Consumes: `CloudflareApi` (Task 5), `AgentConfig.cloudflare` (Task 4).
- Produces: `CloudflaredIngress::new(config_path, tunnel, tunnel_id, zone, restart, cf: Arc<dyn CloudflareApi>)`. `route_dns_and_restart` calls `cf.put_tunnel_cname(zone, hostname, tunnel_id)` instead of shelling `cloudflared tunnel route dns`.

- [ ] **Step 1: Write the failing test.** In `cloudflared.rs` `mod tests`, add a test that a successful `upsert` calls the CloudflareApi mock rather than a shell. Use `MockCloudflareApi` (available under the `mocks` feature). Add to the infra crate's dev-deps usage — the existing tests already run with mocks. Test:

```rust
#[tokio::test]
async fn upsert_routes_dns_via_api_not_shell() {
    use pi_domain::contracts::MockCloudflareApi;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yml");
    std::fs::write(&path, "tunnel: home\ningress:\n  - service: http_status:404\n").unwrap();

    let mut cf = MockCloudflareApi::new();
    cf.expect_put_tunnel_cname()
        .withf(|zone, name, tid| zone == "example.com" && name == "a.example.com" && tid == "tid")
        .returning(|_, _, _| Ok(()));

    let ingress = CloudflaredIngress::new(
        path.clone(),
        "home".into(),
        "tid".into(),
        "example.com".into(),
        vec!["true".into()], // restart command that succeeds
        std::sync::Arc::new(cf),
    );
    ingress
        .upsert("a.example.com", 8002, CollectSink::new())
        .await
        .unwrap();
}
```

> Note: the restart command `["true"]` is a real no-op binary on the Pi/CI Linux; on Windows CI this specific test is `#[cfg(unix)]`. Mark it `#[cfg(unix)]`.

- [ ] **Step 2: Change the struct + constructor.** In `cloudflared.rs`:

```rust
use pi_domain::contracts::CloudflareApi;

pub struct CloudflaredIngress {
    config_path: PathBuf,
    tunnel: String,
    tunnel_id: String,
    zone: String,
    restart: Vec<String>,
    cf: Arc<dyn CloudflareApi>,
}

impl CloudflaredIngress {
    pub fn new(
        config_path: PathBuf,
        tunnel: String,
        tunnel_id: String,
        zone: String,
        restart: Vec<String>,
        cf: Arc<dyn CloudflareApi>,
    ) -> Arc<CloudflaredIngress> {
        Arc::new(CloudflaredIngress {
            config_path,
            tunnel,
            tunnel_id,
            zone,
            restart,
            cf,
        })
    }
}
```

- [ ] **Step 3: Replace the shell in `route_dns_and_restart`.** Swap the `Command::new("cloudflared").args(["tunnel","route","dns",...])` block for:

```rust
        match self.cf.put_tunnel_cname(&self.zone, hostname, &self.tunnel_id).await {
            Ok(_) => log.line(&format!("ingress: DNS record ensured for {hostname}")),
            Err(err) => return Err(ingress_err(format!("route dns: {err}"))),
        }
```

Delete the now-unused `is_already_exists` helper and its test (the API upsert is idempotent, so "already exists" tolerance is no longer needed). Keep `self.tunnel` (still written into `config.yml` semantics) — it remains part of the struct even if only `tunnel_id` is used for DNS.

- [ ] **Step 4: Rewire construction.** In `crates/bin/src/agent/state.rs`, replace the `ingress` match:

```rust
    let ingress: Arc<dyn Ingress> = match (&config.cloudflared, &config.cloudflare) {
        (Some(cf_local), Some(cf_acct)) => {
            let token = std::fs::read_to_string(&cf_acct.token_file)
                .map_err(|e| anyhow::anyhow!("read cloudflare token {}: {e}", cf_acct.token_file.display()))?
                .trim()
                .to_string();
            let api: Arc<dyn pi_domain::contracts::CloudflareApi> =
                Arc::new(pi_infrastructure::cloudflare::HttpCloudflare::new(
                    token,
                    cf_acct.account_id.clone(),
                ));
            let tunnel_id = cf_local.tunnel_id.clone().unwrap_or_default();
            CloudflaredIngress::new(
                cf_local.config.clone(),
                cf_local.tunnel.clone(),
                tunnel_id,
                cf_acct.zone.clone(),
                cf_local.restart.clone(),
                api,
            )
        }
        _ => DisabledIngress::new(),
    };
```

Add `pub tunnel_id: Option<String>` (serde default) to `CloudflaredSection` in `config.rs` so the bootstrap can persist the created tunnel's id.

- [ ] **Step 5: Run tests.** Run: `rtk cargo test -p pi-infrastructure cloudflared && rtk cargo build -p pi`
Expected: PASS + the bin compiles with the new wiring.

- [ ] **Step 6: Commit.**

```bash
rtk git add crates/infrastructure/src/cloudflared.rs crates/bin/src/agent/state.rs crates/bin/src/agent/config.rs
rtk git commit -m "feat(ingress): route tunnel DNS via Cloudflare API (drop cert.pem dependency)"
```

---

## Task 7: Token secret file + `rpi-secrets` group

**Files:**
- Modify: `crates/bin/src/agent/setup.rs`

**Interfaces:**
- Produces: `pub(crate) async fn ensure_cloudflare_token(sys: &dyn Sys, token: &str, dry: bool, rep: &mut SetupReport)` — creates group `rpi-secrets`, writes `/var/lib/rpi/cloudflare/token` (0640, `root:rpi-secrets`), adds `rpi-agent` to `rpi-secrets`.
- Consumes: `Sys`.

- [ ] **Step 1: Write the failing test.** In `setup.rs` `mod tests`:

```rust
#[tokio::test]
async fn writes_token_with_rpi_secrets_group() {
    let sys = fresh_sys();
    let mut rep = SetupReport::default();
    ensure_cloudflare_token(&sys, "cf-token-value", false, &mut rep).await;
    let writes = sys.writes.lock().unwrap();
    assert!(
        writes.iter().any(|(p, c)| p == "/var/lib/rpi/cloudflare/token" && c == "cf-token-value"),
        "token written"
    );
    let calls = sys.calls();
    assert!(calls.iter().any(|c| c.contains("groupadd") && c.contains("rpi-secrets")));
    assert!(calls.iter().any(|c| c == "usermod -aG rpi-secrets rpi-agent"));
    assert!(calls.iter().any(|c| c.contains("chmod 640 /var/lib/rpi/cloudflare/token")));
    assert!(calls.iter().any(|c| c.contains("chown root:rpi-secrets /var/lib/rpi/cloudflare/token")));
}
```

- [ ] **Step 2: Implement.** In `setup.rs`:

```rust
pub(crate) const CLOUDFLARE_TOKEN_PATH: &str = "/var/lib/rpi/cloudflare/token";

pub(crate) async fn ensure_cloudflare_token(
    sys: &dyn Sys,
    token: &str,
    dry: bool,
    rep: &mut SetupReport,
) {
    if dry {
        rep.created.push(CLOUDFLARE_TOKEN_PATH.into());
        return;
    }
    let _ = sys.run("groupadd", &["-f", "rpi-secrets"]).await;
    let _ = sys
        .run("usermod", &["-aG", "rpi-secrets", "rpi-agent"])
        .await;
    let _ = sys
        .run("install", &["-d", "-m", "0750", "/var/lib/rpi/cloudflare"])
        .await;
    match sys.write(Path::new(CLOUDFLARE_TOKEN_PATH), token) {
        Ok(_) => {
            let _ = sys
                .run("chown", &["root:rpi-secrets", CLOUDFLARE_TOKEN_PATH])
                .await;
            let _ = sys.run("chmod", &["640", CLOUDFLARE_TOKEN_PATH]).await;
            rep.created.push(CLOUDFLARE_TOKEN_PATH.into());
        }
        Err(e) => rep.errors.push(format!("write {CLOUDFLARE_TOKEN_PATH}: {e}")),
    }
}
```

- [ ] **Step 3: Run tests.** Run: `rtk cargo test -p pi setup::tests::writes_token_with_rpi_secrets_group`
Expected: PASS.

- [ ] **Step 4: Commit.**

```bash
rtk git add crates/bin/src/agent/setup.rs
rtk git commit -m "feat(setup): write Cloudflare token under root:rpi-secrets 0640"
```

---

## Task 8: Install the `cloudflared` binary by arch

**Files:**
- Modify: `crates/bin/src/agent/setup.rs`

**Interfaces:**
- Produces: `pub(crate) fn cloudflared_asset(uname_m: &str) -> Option<&'static str>` (maps `uname -m` → cloudflared release asset suffix); `pub(crate) async fn ensure_cloudflared_binary(sys: &dyn Sys, dry: bool, rep: &mut SetupReport)`.
- Consumes: `Sys`.

- [ ] **Step 1: Write the failing test for the arch map.** In `setup.rs` `mod tests`:

```rust
#[test]
fn cloudflared_asset_maps_known_arches() {
    assert_eq!(cloudflared_asset("aarch64"), Some("cloudflared-linux-arm64"));
    assert_eq!(cloudflared_asset("armv7l"), Some("cloudflared-linux-arm"));
    assert_eq!(cloudflared_asset("x86_64"), Some("cloudflared-linux-amd64"));
    assert_eq!(cloudflared_asset("mips"), None);
}

#[tokio::test]
async fn installs_cloudflared_when_absent() {
    let mut sys = fresh_sys();
    sys.ok.insert(FakeSys::key("uname", &["-m"]), "aarch64".into());
    // `cloudflared --version` fails => not installed yet
    sys.err.insert(FakeSys::key("cloudflared", &["--version"]));
    let mut rep = SetupReport::default();
    ensure_cloudflared_binary(&sys, false, &mut rep).await;
    let calls = sys.calls();
    assert!(
        calls.iter().any(|c| c.contains("cloudflared-linux-arm64")),
        "downloads the arm64 asset: {calls:?}"
    );
    assert!(calls.iter().any(|c| c.contains("chmod") && c.contains("/usr/local/bin/cloudflared")));
}
```

- [ ] **Step 2: Implement.** In `setup.rs`:

```rust
pub(crate) const CLOUDFLARED_BIN: &str = "/usr/local/bin/cloudflared";

pub(crate) fn cloudflared_asset(uname_m: &str) -> Option<&'static str> {
    match uname_m {
        "aarch64" | "arm64" => Some("cloudflared-linux-arm64"),
        "armv7l" | "armv6l" | "arm" => Some("cloudflared-linux-arm"),
        "x86_64" | "amd64" => Some("cloudflared-linux-amd64"),
        _ => None,
    }
}

pub(crate) async fn ensure_cloudflared_binary(sys: &dyn Sys, dry: bool, rep: &mut SetupReport) {
    if sys.run("cloudflared", &["--version"]).await.is_ok() {
        rep.skipped.push(CLOUDFLARED_BIN.into());
        return;
    }
    if dry {
        rep.created.push(CLOUDFLARED_BIN.into());
        return;
    }
    let arch = match sys.run("uname", &["-m"]).await {
        Ok(a) => a,
        Err(e) => {
            rep.errors.push(format!("uname -m failed: {e}"));
            return;
        }
    };
    let Some(asset) = cloudflared_asset(arch.trim()) else {
        rep.errors
            .push(format!("unsupported architecture for cloudflared: {arch}"));
        return;
    };
    let url = format!(
        "https://github.com/cloudflare/cloudflared/releases/latest/download/{asset}"
    );
    if let Err(e) = sys
        .run("curl", &["-fsSL", "-o", CLOUDFLARED_BIN, &url])
        .await
    {
        rep.errors.push(format!("download cloudflared: {e}"));
        return;
    }
    let _ = sys.run("chmod", &["0755", CLOUDFLARED_BIN]).await;
    rep.created.push(CLOUDFLARED_BIN.into());
}
```

- [ ] **Step 3: Run tests.** Run: `rtk cargo test -p pi setup::tests::cloudflared_asset_maps_known_arches && rtk cargo test -p pi setup::tests::installs_cloudflared_when_absent`
Expected: PASS.

- [ ] **Step 4: Commit.**

```bash
rtk git add crates/bin/src/agent/setup.rs
rtk git commit -m "feat(setup): install cloudflared binary by architecture"
```

---

## Task 9: Full tunnel bootstrap (API create → creds JSON → config.yml → agent.toml)

**Files:**
- Modify: `crates/bin/src/agent/setup.rs`

**Interfaces:**
- Consumes: `CloudflareApi` (Task 5 — the real `HttpCloudflare`), `ensure_cloudflared_binary` (Task 8), `credentials_json` (Task 5).
- Produces: `pub struct CloudflaredBootstrap { pub tunnel_name: String, pub zone: String }`; `pub(crate) async fn cloudflared_bootstrap_full(sys: &dyn Sys, cf: &dyn CloudflareApi, opts: &CloudflaredBootstrap, dry: bool, rep: &mut SetupReport)`. Writes `config.yml` via `pub(crate) fn render_config_yml(tunnel_id, creds_path, ingress_rules) -> String` (spaces only, catch-all last).

- [ ] **Step 1: Write the failing test for `render_config_yml`.** In `setup.rs` `mod tests`:

```rust
#[test]
fn config_yml_uses_spaces_and_keeps_catch_all() {
    let yml = render_config_yml(
        "tid",
        "/var/lib/rpi/cloudflared/tid.json",
    );
    assert!(!yml.contains('\t'), "no tabs allowed in cloudflared config");
    assert!(yml.contains("tunnel: tid"));
    assert!(yml.contains("credentials-file: /var/lib/rpi/cloudflared/tid.json"));
    assert!(yml.trim_end().ends_with("service: http_status:404"), "catch-all last");
}
```

- [ ] **Step 2: Implement `render_config_yml`.** In `setup.rs`:

```rust
/// Minimal locally-managed cloudflared config: spaces only, catch-all last.
/// Per-hostname ingress rules are added later at deploy time by CloudflaredIngress.
pub(crate) fn render_config_yml(tunnel_id: &str, creds_path: &str) -> String {
    format!(
        "tunnel: {tunnel_id}\n\
         credentials-file: {creds_path}\n\
         \n\
         ingress:\n\
         \x20\x20- service: http_status:404\n"
    )
}
```

- [ ] **Step 3: Write the failing test for the bootstrap orchestration.** Use a fake `CloudflareApi` (mock) that returns creds, and assert the creds JSON + config.yml are written and `ingress validate` is run:

```rust
#[tokio::test]
async fn bootstrap_writes_creds_config_and_validates() {
    use pi_domain::contracts::{MockCloudflareApi, TunnelCreds};
    let mut sys = fresh_sys();
    sys.ok.insert(FakeSys::key("uname", &["-m"]), "aarch64".into());
    sys.err.insert(FakeSys::key("cloudflared", &["--version"])); // triggers install path
    let mut cf = MockCloudflareApi::new();
    cf.expect_find_or_create_tunnel().returning(|name| {
        Ok(TunnelCreds {
            account_tag: "acc".into(),
            tunnel_id: "tid".into(),
            tunnel_name: name.to_string(),
            tunnel_secret: "c2VjcmV0".into(),
        })
    });
    let mut rep = SetupReport::default();
    let opts = CloudflaredBootstrap {
        tunnel_name: "myboard".into(),
        zone: "example.com".into(),
    };
    cloudflared_bootstrap_full(&sys, &cf, &opts, false, &mut rep).await;
    let writes = sys.writes.lock().unwrap();
    assert!(writes.iter().any(|(p, _)| p == "/var/lib/rpi/cloudflared/tid.json"), "creds json");
    assert!(writes.iter().any(|(p, c)| p == "/var/lib/rpi/cloudflared/config.yml" && !c.contains('\t')));
    assert!(sys.calls().iter().any(|c| c.contains("ingress validate")));
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
}
```

- [ ] **Step 4: Implement `cloudflared_bootstrap_full`.** In `setup.rs`:

```rust
use pi_domain::contracts::CloudflareApi;
use pi_infrastructure::cloudflare::credentials_json;

pub struct CloudflaredBootstrap {
    pub tunnel_name: String,
    pub zone: String,
}

pub(crate) async fn cloudflared_bootstrap_full(
    sys: &dyn Sys,
    cf: &dyn CloudflareApi,
    opts: &CloudflaredBootstrap,
    dry: bool,
    rep: &mut SetupReport,
) {
    ensure_cloudflared_binary(sys, dry, rep).await;
    let _ = sys
        .run("install", &["-d", "-o", "rpi-agent", "-g", "rpi-agent", "/var/lib/rpi/cloudflared"])
        .await;

    if dry {
        rep.created.push("cloudflared tunnel (dry run)".into());
        return;
    }

    let creds = match cf.find_or_create_tunnel(&opts.tunnel_name).await {
        Ok(c) => c,
        Err(e) => {
            rep.errors.push(format!("create tunnel: {e}"));
            return;
        }
    };
    let creds_path = format!("/var/lib/rpi/cloudflared/{}.json", creds.tunnel_id);
    // Only (re)write creds when we hold a secret (freshly created). An adopted
    // tunnel (empty secret) must already have its creds file on disk.
    if !creds.tunnel_secret.is_empty() {
        if let Err(e) = sys.write(Path::new(&creds_path), &credentials_json(&creds)) {
            rep.errors.push(format!("write creds: {e}"));
            return;
        }
        let _ = sys.run("chown", &["rpi-agent:rpi-agent", &creds_path]).await;
        let _ = sys.run("chmod", &["640", &creds_path]).await;
        rep.created.push(creds_path.clone());
    } else if !sys.exists(Path::new(&creds_path)) {
        rep.errors.push(format!(
            "adopted tunnel {} but no credentials at {creds_path}; re-create the tunnel or restore its JSON",
            creds.tunnel_id
        ));
        return;
    }

    let config = render_config_yml(&creds.tunnel_id, &creds_path);
    if let Err(e) = sys.write(Path::new(CLOUDFLARED_CONFIG_PATH), &config) {
        rep.errors.push(format!("write config.yml: {e}"));
        return;
    }
    let _ = sys.run("chown", &["rpi-agent:rpi-agent", CLOUDFLARED_CONFIG_PATH]).await;
    let _ = sys.run("chmod", &["640", CLOUDFLARED_CONFIG_PATH]).await;

    match sys
        .run("cloudflared", &["tunnel", "--config", CLOUDFLARED_CONFIG_PATH, "ingress", "validate"])
        .await
    {
        Ok(_) => rep.created.push(CLOUDFLARED_CONFIG_PATH.into()),
        Err(e) => rep.errors.push(format!("cloudflared ingress validate: {e}")),
    }

    upsert_cloudflared_agent_toml(sys, &creds.tunnel_id, &opts.zone, rep);
}
```

- [ ] **Step 5: Implement `upsert_cloudflared_agent_toml`.** Append `[cloudflare]` + `[cloudflared]` to `/etc/rpi/agent.toml` only when absent (parse-and-check, backup on divergence). Minimal version that appends when the sections are missing:

```rust
fn upsert_cloudflared_agent_toml(sys: &dyn Sys, tunnel_id: &str, zone: &str, rep: &mut SetupReport) {
    let existing = sys.read(Path::new(AGENT_TOML_PATH)).unwrap_or_default();
    if existing.contains("[cloudflared]") {
        rep.skipped.push("agent.toml [cloudflared]".into());
        return;
    }
    let block = format!(
        "\n[cloudflare]\nzone = \"{zone}\"\ntoken_file = \"{CLOUDFLARE_TOKEN_PATH}\"\n\n\
         [cloudflared]\nconfig = \"{CLOUDFLARED_CONFIG_PATH}\"\ntunnel = \"{tunnel_id}\"\ntunnel_id = \"{tunnel_id}\"\n"
    );
    match sys.write(Path::new(AGENT_TOML_PATH), &format!("{existing}{block}")) {
        Ok(_) => rep.created.push("agent.toml [cloudflare]/[cloudflared]".into()),
        Err(e) => rep.errors.push(format!("write agent.toml sections: {e}")),
    }
}
```

- [ ] **Step 6: Run tests.** Run: `rtk cargo test -p pi setup::tests::config_yml_uses_spaces_and_keeps_catch_all && rtk cargo test -p pi setup::tests::bootstrap_writes_creds_config_and_validates`
Expected: PASS.

- [ ] **Step 7: Commit.**

```bash
rtk git add crates/bin/src/agent/setup.rs
rtk git commit -m "feat(setup): create tunnel via API, write validated config.yml + agent.toml"
```

---

## Task 10: systemd `--user` drop-in fix + enable/linger

**Files:**
- Modify: `crates/bin/src/agent/setup.rs`

**Interfaces:**
- Produces: `pub(crate) async fn cloudflared_user_service(sys: &dyn Sys, dry: bool, rep: &mut SetupReport)` — resolves `rpi-agent` uid, writes the `user@<uid>.service.d/override.conf` drop-in, reloads, enables linger, enables the user unit.
- Consumes: `Sys`, existing `CLOUDFLARED_UNIT`/`CLOUDFLARED_UNIT_PATH`.

- [ ] **Step 1: Write the failing test.** In `setup.rs` `mod tests`:

```rust
#[tokio::test]
async fn user_service_writes_dropin_and_enables() {
    let mut sys = fresh_sys();
    sys.ok.insert(FakeSys::key("id", &["-u", "rpi-agent"]), "999".into());
    let mut rep = SetupReport::default();
    cloudflared_user_service(&sys, false, &mut rep).await;
    let writes = sys.writes.lock().unwrap();
    assert!(
        writes.iter().any(|(p, c)| p.contains("user@999.service.d/override.conf")
            && c.contains("XDG_CONFIG_HOME=/var/lib/rpi/.config")),
        "drop-in written for uid 999"
    );
    let calls = sys.calls();
    assert!(calls.iter().any(|c| c == "systemctl daemon-reload"));
    assert!(calls.iter().any(|c| c == "systemctl restart user@999.service"));
    assert!(calls.iter().any(|c| c == "loginctl enable-linger rpi-agent"));
}
```

- [ ] **Step 2: Implement.** In `setup.rs`:

```rust
pub(crate) async fn cloudflared_user_service(sys: &dyn Sys, dry: bool, rep: &mut SetupReport) {
    let uid = match sys.run("id", &["-u", "rpi-agent"]).await {
        Ok(u) => u.trim().to_string(),
        Err(e) => {
            rep.errors.push(format!("id -u rpi-agent: {e}"));
            return;
        }
    };
    let dropin_dir = format!("/etc/systemd/system/user@{uid}.service.d");
    let dropin = format!("{dropin_dir}/override.conf");
    let dropin_body =
        "[Service]\nEnvironment=XDG_CONFIG_HOME=/var/lib/rpi/.config\nEnvironment=HOME=/var/lib/rpi\n";

    if dry {
        rep.created.push(dropin.clone());
        return;
    }

    let _ = sys.run("install", &["-d", &dropin_dir]).await;
    if let Err(e) = sys.write(Path::new(&dropin), dropin_body) {
        rep.errors.push(format!("write {dropin}: {e}"));
        return;
    }
    rep.created.push(dropin);

    // ensure the user unit exists (existing scaffold path)
    let _ = sys
        .run("install", &["-d", "-o", "rpi-agent", "-g", "rpi-agent", "/var/lib/rpi/.config/systemd/user"])
        .await;
    let _ = sys.write(Path::new(CLOUDFLARED_UNIT_PATH), CLOUDFLARED_UNIT);

    let _ = sys.run("systemctl", &["daemon-reload"]).await;
    let _ = sys
        .run("systemctl", &["restart", &format!("user@{uid}.service")])
        .await;
    let _ = sys.run("loginctl", &["enable-linger", "rpi-agent"]).await;
    let runtime = format!("XDG_RUNTIME_DIR=/run/user/{uid}");
    // enable+start the user unit as rpi-agent with the runtime dir set
    let _ = sys
        .run(
            "sudo",
            &["-u", "rpi-agent", &runtime, "systemctl", "--user", "enable", "--now", "cloudflared"],
        )
        .await;
    rep.created.push("cloudflared user service enabled".into());
}
```

> Note: whether `sudo -u rpi-agent VAR=... systemctl --user …` needs `env` depends on the host sudoers; if the integration test on the Pi shows the env var isn't honored, switch to `sudo -u rpi-agent env XDG_RUNTIME_DIR=/run/user/<uid> systemctl --user …`. Keep this as a single `sys.run` call so the seam stays testable.

- [ ] **Step 3: Run tests.** Run: `rtk cargo test -p pi setup::tests::user_service_writes_dropin_and_enables`
Expected: PASS.

- [ ] **Step 4: Wire the full flow.** In `setup.rs`, replace the body of the old `cloudflared_bootstrap` (called from `setup()` when `opts.with_cloudflared`) so that, **when a token+domain are present**, it runs: `ensure_cloudflare_token` → `cloudflared_bootstrap_full` (constructing `HttpCloudflare` from the token) → `cloudflared_user_service`; and **when they are absent**, keeps today's scaffold-and-warn behavior (backward compatible). Add `cf_token: Option<String>`, `domain: Option<String>`, `tunnel_name: Option<String>` to `SetupOpts`.

- [ ] **Step 5: Run the whole setup test module.** Run: `rtk cargo test -p pi setup::`
Expected: PASS (existing scaffold tests still green on the no-token path; new tests green on the token path).

- [ ] **Step 6: Commit.**

```bash
rtk git add crates/bin/src/agent/setup.rs
rtk git commit -m "feat(setup): systemd --user drop-in fix + enable cloudflared; wire full flow"
```

---

## Task 11: CLI — `agent setup` flags + `agent migrate` subcommand

**Files:**
- Modify: `crates/bin/src/main.rs`
- Modify: `crates/bin/src/agent/setup.rs` (`run_cmd` signature), `crates/bin/src/agent/migrate.rs` (entrypoint)

**Interfaces:**
- Consumes: `setup::run_cmd`, migration `registry()` + `run_explicit`/`run_auto`, `DbLedger`.
- Produces: `AgentCmd::Migrate { list: bool, dry_run: bool, run: Vec<String>, all: bool, yes: bool }`; extended `AgentCmd::Setup`.

- [ ] **Step 1: Write the failing parse test.** In `main.rs` `mod tests` (there is an existing `agent setup` parse test near line 487), add:

```rust
#[test]
fn parses_agent_migrate() {
    let cli = Cli::try_parse_from(["rpi", "agent", "migrate", "--run", "nginx-to-caddy"]).unwrap();
    match cli.command {
        Cmd::Agent { cmd: AgentCmd::Migrate { run, .. } } => {
            assert_eq!(run, vec!["nginx-to-caddy".to_string()]);
        }
        _ => panic!("expected agent migrate"),
    }
}

#[test]
fn parses_agent_setup_cloudflare_flags() {
    let cli = Cli::try_parse_from([
        "rpi", "agent", "setup", "--user", "piuser",
        "--with-cloudflared", "--cf-token", "t", "--domain", "example.com",
    ])
    .unwrap();
    match cli.command {
        Cmd::Agent { cmd: AgentCmd::Setup { with_cloudflared, cf_token, domain, .. } } => {
            assert!(with_cloudflared);
            assert_eq!(cf_token.as_deref(), Some("t"));
            assert_eq!(domain.as_deref(), Some("example.com"));
        }
        _ => panic!("expected agent setup"),
    }
}
```

- [ ] **Step 2: Extend `AgentCmd::Setup` and add `Migrate`.** In `main.rs`, update the `Setup` variant and add `Migrate`:

```rust
    /// Bootstrap the agent on this Pi (run with sudo; idempotent)
    Setup {
        #[arg(long)]
        user: Option<String>,
        #[arg(long)]
        with_cloudflared: bool,
        /// Cloudflare API token (or env CLOUDFLARE_API_TOKEN); enables full auto-bootstrap
        #[arg(long)]
        cf_token: Option<String>,
        /// Base zone, e.g. example.com
        #[arg(long)]
        domain: Option<String>,
        /// Tunnel name (default: derived)
        #[arg(long)]
        tunnel: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Run host migrations uniformly (idempotent; detect-oriented)
    Migrate {
        /// List all migrations and their status
        #[arg(long)]
        list: bool,
        /// Show the plan without changing anything
        #[arg(long)]
        dry_run: bool,
        /// Apply a specific migration by id (repeatable; needed for disruptive ones)
        #[arg(long)]
        run: Vec<String>,
        /// Apply every applicable migration (with --yes for disruptive ones)
        #[arg(long)]
        all: bool,
        #[arg(long)]
        yes: bool,
    },
```

- [ ] **Step 3: Dispatch.** In `main.rs` where `AgentCmd::Setup { .. } => agent::setup::run_cmd(...)`, pass the new fields (fall back to `env CLOUDFLARE_API_TOKEN` when `cf_token` is None). Add a `Migrate` arm that opens the DB (via `AgentConfig::load` → data_dir → `Db::open` → `MigrationLedger` → `DbLedger`) and calls the runner:

```rust
        AgentCmd::Migrate { list, dry_run, run, all, yes } => {
            agent::migrate::run_cmd(list, dry_run, run, all, yes).await
        }
```

- [ ] **Step 4: Implement `migrate::run_cmd`.** In `migrate.rs`:

```rust
pub async fn run_cmd(
    list: bool,
    dry_run: bool,
    run: Vec<String>,
    all: bool,
    yes: bool,
) -> anyhow::Result<()> {
    use super::setup::HostSys;
    let config = super::config::AgentConfig::load(None)?;
    let db = pi_infrastructure::sqlite::Db::open(&config.data_dir.join("state.db"))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let ledger = super::migrate_ledger::DbLedger::new(
        pi_infrastructure::migrations::MigrationLedger::new(db),
    );
    let registry = registry();
    let mut rep = super::setup::SetupReport::default();
    let sys = HostSys;

    if list {
        for m in &registry {
            let applied = ledger.is_applied(m.id()).await;
            println!("{}\t{}\tapplied={applied}", m.id(), m.description());
        }
        return Ok(());
    }
    if !run.is_empty() {
        run_explicit(&sys, &ledger, &registry, &run, dry_run, &mut rep).await;
    } else if all && yes {
        let ids: Vec<String> = registry.iter().map(|m| m.id().to_string()).collect();
        run_explicit(&sys, &ledger, &registry, &ids, dry_run, &mut rep).await;
    } else {
        run_auto(&sys, &ledger, &registry, dry_run, &mut rep).await;
    }
    rep.print();
    if !rep.errors.is_empty() {
        anyhow::bail!("migrate completed with {} error(s)", rep.errors.len());
    }
    Ok(())
}
```

- [ ] **Step 5: Run tests + full build.** Run: `rtk cargo test -p pi main:: && rtk cargo build`
Expected: PASS (parse tests) and the workspace builds.

- [ ] **Step 6: Full CI gate.** Run: `rtk cargo fmt --all -- --check && rtk cargo clippy --all-targets --locked -- -D warnings && rtk cargo test --locked`
Expected: all green.

- [ ] **Step 7: Commit.**

```bash
rtk git add crates/bin/src/main.rs crates/bin/src/agent/setup.rs crates/bin/src/agent/migrate.rs
rtk git commit -m "feat(cli): agent setup cloudflare flags + agent migrate subcommand"
```

---

## Task 12: README — Cloudflare Tunnel auto-bootstrap + migrations

**Files:**
- Modify: `README.md` (Cloudflare Tunnel section ~line 803; add a Migrations note)

**Interfaces:** none (docs).

- [ ] **Step 1: Rewrite the Cloudflare Tunnel section** to document the one-command auto-bootstrap: `sudo rpi agent setup --with-cloudflared --cf-token <t> --domain <zone>`; the required token scopes (`Zone:DNS:Edit`, `Zone:Zone:Read`, `Account:Cloudflare Tunnel:Edit`); that no `cloudflared tunnel login`/`cert.pem` is needed; and that the manual path still works when the token is omitted (backward compatible).

- [ ] **Step 2: Add a short "Migrations" subsection** documenting `rpi agent migrate [--list] [--dry-run] [--run <id>]`, that non-disruptive migrations run automatically during `setup`, and that disruptive ones require `--run`.

- [ ] **Step 3: Commit.**

```bash
rtk git add README.md
rtk git commit -m "docs: Cloudflare Tunnel auto-bootstrap + rpi agent migrate"
```

---

## Self-Review Notes (for the implementer)

- **Spec coverage (Phase 1 slice):** API-token tunnel create (Task 5, 9) · no cert.pem/login (Task 5, 9) · creds JSON only (Task 5) · config.yml spaces + validate (Task 9) · systemd drop-in fix (Task 10) · schema field (Task 4) · `[cloudflare]` section (Task 4) · single-token/`rpi-secrets` (Task 7) · migration framework + ledger (Task 1, 2) · pi-to-rpi refactor (Task 3) · DNS-via-API replacing route-dns shell (Task 6) · CLI surface (Task 11) · docs (Task 12). Pitfall 1 (login-user validation) is already present in `run_cmd`; pitfall 2 (binary install) is Task 8.
- **Deferred to Phase 2 (do NOT build here):** Caddy install/config, `[lan]` section, `lan_hostname`/`lan`, `CaddyIngress`, `*.lan` DNS `A` record, `nginx-to-caddy` migration. The `Migration` trait's `disruptive` flag and `run_explicit`/`--run` path exist here but the only registered migration is `pi-to-rpi`.
- **Type consistency check:** `CloudflareApi::{zone_id, find_or_create_tunnel, put_tunnel_cname}`, `TunnelCreds{account_tag,tunnel_id,tunnel_name,tunnel_secret}`, `MigrationLedger::{is_applied,mark_applied,applied}`, `LedgerHandle::{is_applied,mark_applied}`, `Migration::{id,description,disruptive,detect,apply}`, `MigrationState::{Applicable,Done,NotApplicable}` — used identically across tasks.
- **Adoption caveat (integration-only):** `find_or_create_tunnel` cannot recover an existing tunnel's secret; for an adopted tunnel Task 9 requires the creds JSON to already exist on disk (the user's `myboard` case). If it does not, setup errors with an actionable message rather than writing a broken creds file. Verify this path on the Pi with the existing `myboard` tunnel.
