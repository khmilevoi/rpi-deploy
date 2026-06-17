# pi — LAN-expose (`expose = "lan"`) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** дать per-project настройку `[ingress] expose = "lan"`, при которой агент биндит host-порт на `0.0.0.0` (доступ из локальной сети), показывает готовый `http://<lan-ip>:<port>` в `pi deploy`/`pi ls`, и не меняет поведение проектов без этой настройки.

**Architecture:** расширяем существующие слои (domain → infrastructure → application → bin) по правилам репозитория. В `domain` — `ExposeMode` + поле `ProjectConfig.expose` и контракт `HostNetwork`; `OverrideStore::write` получает bind-адрес. В `infrastructure` — bind в compose-override, миграция БД + персист `expose`, `UdpHostNetwork`. В `application` — deploy пишет lan-строку, list отдаёт `expose`+`lan_ip`. В `bin` — парсинг `pi.toml`, wire-DTO с `#[serde(default)]`, рендер `pi ls`, бамп версии, README.

**Tech Stack:** как в репозитории (Rust, tokio, axum, rusqlite + rusqlite_migration, async-trait, mockall за фичей `mocks`, serde). Новых зависимостей нет — `UdpHostNetwork` использует `std::net::UdpSocket`.

**Спека:** `docs/superpowers/specs/2026-06-17-pi-lan-expose-design.md`; базовая — `docs/superpowers/specs/2026-06-09-pi-deploy-tool-design.md` §11, §12, §12.1.

## Global Constraints

- **Версия:** патч-бамп `0.3.0 → 0.3.1` в корневом `Cargo.toml:6` (`version = "0.3.1"`); все крейты наследуют через `version.workspace = true`. API-строка остаётся `"v1"` (wire-совместимость через `#[serde(default)]`).
- **Все команды** — с префиксом `rtk` (`rtk cargo test`, `rtk git add …`).
- Коммиты — conventional commits на английском; трейлер `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Разработка на **Windows**, целевая платформа агента — Linux. Юнит-тесты обязаны проходить на Windows; интеграция с реальным docker/сетью — `#[ignore]`.
- Код и комментарии — на английском. Без `unwrap()`/`expect()` в use-cases/адаптерах (§19); в тестах `unwrap()` допустим.
- Дефолт везде — `private` (бинд `127.0.0.1`). Старые `pi.toml` и старые wire-payload'ы без `expose` → `private`.
- Один таск = один коммит после зелёных тестов.

## File Structure

```
Cargo.toml                              # MOD: version 0.3.0 -> 0.3.1 (Task 10)
crates/
├─ domain/src/
│  ├─ entities.rs       # MOD: +ExposeMode enum; +ProjectConfig.expose (Task 1)
│  └─ contracts.rs      # MOD: OverrideStore::write +bind (Task 2); +HostNetwork (Task 6)
├─ infrastructure/src/
│  ├─ overrides.rs      # MOD: override_yaml/write take bind (Task 2)
│  ├─ sqlite.rs         # MOD: migration #2 — projects.expose column (Task 3)
│  ├─ repo.rs           # MOD: SELECT/INSERT/UPDATE + row_to_project expose (Task 3)
│  ├─ hostnet.rs        # NEW: UdpHostNetwork (Task 6)
│  └─ lib.rs            # MOD: +pub mod hostnet (Task 6)
├─ application/src/
│  ├─ deploy.rs         # MOD: bind from expose (Task 2); +host_network, lan log (Task 7)
│  ├─ env.rs            # MOD: bind from registered.config.expose (Task 2)
│  └─ list.rs           # MOD: ProjectView +expose/+lan_ip; +host_network (Task 8)
└─ bin/src/
   ├─ cli/pitoml.rs     # MOD: IngressSection.expose + validation + mapping (Task 4)
   ├─ proto.rs          # MOD: ProjectDto.expose (Task 5); ProjectViewDto +expose/+lan_ip (Task 8)
   ├─ cli/commands.rs   # MOD: pi ls renders expose=lan + URL (Task 9)
   └─ agent/state.rs    # MOD: wire UdpHostNetwork into DeployProject & ListProjects (Task 7, 8)
```

---

### Task 1: Domain — `ExposeMode` + `ProjectConfig.expose`

**Files:**
- Modify: `crates/domain/src/entities.rs`
- Modify (construction sites, keep behavior identical): `crates/infrastructure/src/repo.rs`, `crates/application/src/list.rs`, `crates/application/src/deploy.rs`, `crates/application/src/env.rs`, `crates/bin/src/cli/pitoml.rs`, `crates/bin/src/proto.rs`
- Test: `crates/domain/src/entities.rs` (unit tests module)

**Interfaces:**
- Produces:
  - `enum ExposeMode { Private, Lan }` with `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`, `Default = Private`.
  - `ExposeMode::as_str(&self) -> &'static str` → `"private"` / `"lan"`.
  - `ExposeMode::bind_addr(&self) -> &'static str` → `"127.0.0.1"` / `"0.0.0.0"`.
  - `ExposeMode::parse(s: &str) -> Option<ExposeMode>`.
  - field `pub expose: ExposeMode` on `ProjectConfig`.

- [ ] **Step 1: Write the failing test** — append to the `tests` module in `crates/domain/src/entities.rs`:

```rust
#[test]
fn expose_mode_maps_strings_bind_addrs_and_default() {
    assert_eq!(ExposeMode::default(), ExposeMode::Private);
    assert_eq!(ExposeMode::Private.as_str(), "private");
    assert_eq!(ExposeMode::Lan.as_str(), "lan");
    assert_eq!(ExposeMode::Private.bind_addr(), "127.0.0.1");
    assert_eq!(ExposeMode::Lan.bind_addr(), "0.0.0.0");
    assert_eq!(ExposeMode::parse("private"), Some(ExposeMode::Private));
    assert_eq!(ExposeMode::parse("lan"), Some(ExposeMode::Lan));
    assert_eq!(ExposeMode::parse("public"), None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `rtk cargo test -p pi-domain expose_mode_maps_strings`
Expected: FAIL — `cannot find type ExposeMode`.

- [ ] **Step 3: Implement the enum** — add to `crates/domain/src/entities.rs` (near `ProjectConfig`):

```rust
/// Where the agent publishes the project's host port ([ingress].expose, §12.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExposeMode {
    /// Bind 127.0.0.1 — reachable only on the host (and via Cloudflare). Default.
    Private,
    /// Bind 0.0.0.0 — reachable from the local network.
    Lan,
}

impl Default for ExposeMode {
    fn default() -> ExposeMode {
        ExposeMode::Private
    }
}

impl ExposeMode {
    /// Token used in pi.toml, the DB and the wire.
    pub fn as_str(&self) -> &'static str {
        match self {
            ExposeMode::Private => "private",
            ExposeMode::Lan => "lan",
        }
    }

    /// Host bind address written into the compose-override (§12.1).
    pub fn bind_addr(&self) -> &'static str {
        match self {
            ExposeMode::Private => "127.0.0.1",
            ExposeMode::Lan => "0.0.0.0",
        }
    }

    /// None for any unknown token (callers validate or default).
    pub fn parse(s: &str) -> Option<ExposeMode> {
        match s {
            "private" => Some(ExposeMode::Private),
            "lan" => Some(ExposeMode::Lan),
            _ => None,
        }
    }
}
```

- [ ] **Step 4: Add the field to `ProjectConfig`** — in `crates/domain/src/entities.rs`, add after `hostname`:

```rust
    /// Host bind mode ([ingress].expose). Default Private (127.0.0.1).
    pub expose: ExposeMode,
```

- [ ] **Step 5: Fix all construction sites to compile (behavior identical)** — add `expose: ExposeMode::default(),` (or `ExposeMode::Private`) to every `ProjectConfig { … }` literal. Concretely:
  - `crates/infrastructure/src/repo.rs` — `row_to_project` (the `ProjectConfig { … }`), and the test helper `cfg` → add `expose: ExposeMode::default(),`. Add `ExposeMode` to the `use pi_domain::entities::{…}` lines (both top-of-file and the `#[cfg(test)]` import).
  - `crates/application/src/list.rs` — test helper `project` → add `expose: ExposeMode::default(),`; add `ExposeMode` to test imports.
  - `crates/application/src/deploy.rs` — every `ProjectConfig { … }` in tests → add `expose: ExposeMode::default(),`; add `ExposeMode` to test imports.
  - `crates/application/src/env.rs` — every `ProjectConfig { … }` in tests → add `expose: ExposeMode::default(),`; add `ExposeMode` to test imports.
  - `crates/bin/src/cli/pitoml.rs` — `to_project_config` → add `expose: ExposeMode::default(),`; add `ExposeMode` to the `use pi_domain::entities::{…}`.
  - `crates/bin/src/proto.rs` — `From<ProjectDto> for ProjectConfig` → add `expose: ExposeMode::default(),`; add `ExposeMode` to imports.

> Note: `row_to_project` hardcodes `ExposeMode::default()` for now — Task 3 makes it read the DB column.

- [ ] **Step 6: Run the whole suite**

Run: `rtk cargo test --workspace`
Expected: PASS (all existing tests + the new domain test; behavior unchanged).

- [ ] **Step 7: Commit**

```bash
rtk git add crates/domain/src/entities.rs crates/infrastructure/src/repo.rs crates/application/src/list.rs crates/application/src/deploy.rs crates/application/src/env.rs crates/bin/src/cli/pitoml.rs crates/bin/src/proto.rs
rtk git commit -m "feat(domain): add ExposeMode and ProjectConfig.expose (default private)"
```

---

### Task 2: `OverrideStore::write` takes the bind address; wire it from `expose`

**Files:**
- Modify: `crates/domain/src/contracts.rs` (`OverrideStore::write` signature + doc)
- Modify: `crates/infrastructure/src/overrides.rs` (`override_yaml`, `FsOverrideStore::write`)
- Modify: `crates/application/src/deploy.rs` (pass `config.expose.bind_addr()`)
- Modify: `crates/application/src/env.rs` (pass `registered.config.expose.bind_addr()`)
- Test: `crates/infrastructure/src/overrides.rs`, `crates/application/src/deploy.rs`

**Interfaces:**
- Consumes: `ExposeMode::bind_addr()` (Task 1).
- Produces: `OverrideStore::write(&self, project, service, bind: &str, host_port, container_port) -> Result<PathBuf, DomainError>`; `override_yaml(service, bind, host_port, container_port) -> String`.

- [ ] **Step 1: Write the failing override test** — replace `yaml_maps_loopback_host_port_to_container_port` in `crates/infrastructure/src/overrides.rs` and add a lan case:

```rust
#[test]
fn yaml_maps_bind_host_port_to_container_port() {
    let loopback = override_yaml("web", "127.0.0.1", 8000, 3000);
    assert_eq!(
        loopback,
        "# generated by pi - do not edit\nservices:\n  web:\n    ports:\n      - \"127.0.0.1:8000:3000\"\n"
    );
    let lan = override_yaml("web", "0.0.0.0", 8000, 3000);
    assert!(lan.contains("\"0.0.0.0:8000:3000\""));
}
```

Also update the existing `write_creates_file_in_overrides_dir` test call to the new signature: `store.write("rateme", "web", "127.0.0.1", 8000, 3000)`.

- [ ] **Step 2: Run test to verify it fails**

Run: `rtk cargo test -p pi-infrastructure overrides`
Expected: FAIL — `override_yaml` takes 3 args, not 4 (arity mismatch).

- [ ] **Step 3: Implement bind in `overrides.rs`**

```rust
pub(crate) fn override_yaml(service: &str, bind: &str, host_port: u16, container_port: u16) -> String {
    format!(
        "# generated by pi - do not edit\nservices:\n  {service}:\n    ports:\n      - \"{bind}:{host_port}:{container_port}\"\n"
    )
}
```

And the trait impl:

```rust
    async fn write(
        &self,
        project: &str,
        service: &str,
        bind: &str,
        host_port: u16,
        container_port: u16,
    ) -> Result<PathBuf, DomainError> {
        let io_err = |e: std::io::Error| DomainError::Storage(format!("override write: {e}"));
        tokio::fs::create_dir_all(&self.dir).await.map_err(io_err)?;
        let path = self.dir.join(format!("{project}.yml"));
        tokio::fs::write(&path, override_yaml(service, bind, host_port, container_port))
            .await
            .map_err(io_err)?;
        Ok(path)
    }
```

- [ ] **Step 4: Update the trait in `contracts.rs`** — change the doc comment and add the `bind` param:

```rust
/// Writes compose-override mapping <bind>:<host> -> <container> (§12.1).
/// `bind` is "127.0.0.1" (private) or "0.0.0.0" (lan), from ExposeMode.
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait OverrideStore: Send + Sync {
    async fn write(
        &self,
        project: &str,
        service: &str,
        bind: &str,
        host_port: u16,
        container_port: u16,
    ) -> Result<PathBuf, DomainError>;
}
```

- [ ] **Step 5: Update callers** — in `crates/application/src/deploy.rs` (`run_stages`):

```rust
        let override_file = self
            .overrides
            .write(
                &config.name,
                &config.service,
                config.expose.bind_addr(),
                project.host_port,
                config.container_port,
            )
            .await?;
```

In `crates/application/src/env.rs` (`execute`):

```rust
        let override_file = self
            .overrides
            .write(
                project,
                &config.service,
                config.expose.bind_addr(),
                registered.host_port,
                config.container_port,
            )
            .await?;
```

- [ ] **Step 6: Add a deploy unit test for the lan bind** — in `crates/application/src/deploy.rs` tests, find the existing `m.overrides.expect_write()` setups and tighten one to assert the bind. Add a focused test based on the existing happy-path test, with `config.expose = ExposeMode::Lan` and:

```rust
    m.overrides
        .expect_write()
        .withf(|_project, _service, bind, _host, _container| bind == "0.0.0.0")
        .returning(|_, _, _, _, _| Ok(PathBuf::from("/var/lib/pi/overrides/rateme.yml")));
```

For the existing private-path tests, update their `expect_write()` closures/returning to the new 5-arg arity (`|_, _, _, _, _|`).

- [ ] **Step 7: Run the suite**

Run: `rtk cargo test --workspace`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
rtk git add crates/domain/src/contracts.rs crates/infrastructure/src/overrides.rs crates/application/src/deploy.rs crates/application/src/env.rs
rtk git commit -m "feat: bind compose-override host port per ExposeMode"
```

---

### Task 3: Persist `expose` in the project registry (migration + repo)

**Files:**
- Modify: `crates/infrastructure/src/sqlite.rs` (migration #2)
- Modify: `crates/infrastructure/src/repo.rs` (`SELECT`, `INSERT`, `UPDATE`, `row_to_project`)
- Test: `crates/infrastructure/src/sqlite.rs`, `crates/infrastructure/src/repo.rs`

**Interfaces:**
- Consumes: `ExposeMode::as_str()` / `ExposeMode::parse()` (Task 1).
- Produces: `projects.expose` column (TEXT NOT NULL DEFAULT 'private'); repo reads/writes it.

- [ ] **Step 1: Write the failing migration test** — add to `crates/infrastructure/src/sqlite.rs` tests:

```rust
#[tokio::test]
async fn migration_adds_expose_column_defaulting_private() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("state.db")).unwrap();
    db.call(|c| {
        c.execute(
            "INSERT INTO projects
             (name, repo, branch, compose_path, service, container_port, host_port, created_at)
             VALUES ('a', 'repo-a', 'main', 'docker-compose.yml', 'web', 3000, 8000, 1)",
            [],
        )
        .map_err(storage_err)?;
        Ok(())
    })
    .await
    .unwrap();
    let expose: String = db
        .call(|c| {
            c.query_row("SELECT expose FROM projects WHERE name='a'", [], |r| r.get(0))
                .map_err(storage_err)
        })
        .await
        .unwrap();
    assert_eq!(expose, "private");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `rtk cargo test -p pi-infrastructure migration_adds_expose`
Expected: FAIL — `no such column: expose`.

- [ ] **Step 3: Add migration #2** — in `crates/infrastructure/src/sqlite.rs`, append a second `M::up` to the `vec!` in `migrations()` (do NOT edit migration #1):

```rust
        M::up(
            r#"
            ALTER TABLE projects ADD COLUMN expose TEXT NOT NULL DEFAULT 'private';
            "#,
        ),
```

- [ ] **Step 4: Run the migration test**

Run: `rtk cargo test -p pi-infrastructure migration_adds_expose`
Expected: PASS.

- [ ] **Step 5: Write the failing repo roundtrip test** — add to `crates/infrastructure/src/repo.rs` tests:

```rust
#[tokio::test]
async fn roundtrips_expose_mode() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo(&dir, 8000, 8999);
    let mut c = cfg("a");
    c.expose = pi_domain::entities::ExposeMode::Lan;
    repo.upsert(&c).await.unwrap();
    let got = repo.get("a").await.unwrap().unwrap();
    assert_eq!(got.config.expose, pi_domain::entities::ExposeMode::Lan);
}
```

- [ ] **Step 6: Run test to verify it fails**

Run: `rtk cargo test -p pi-infrastructure roundtrips_expose_mode`
Expected: FAIL — repo writes/reads no `expose`, so it comes back `Private`.

- [ ] **Step 7: Update `repo.rs`**
  - `SELECT` const → add `expose` at the end:

```rust
const SELECT: &str = "SELECT name, repo, branch, compose_path, service, container_port, hostname, host_port, created_at, expose FROM projects";
```

  - `row_to_project` → read the new column (index 9) and replace the hardcoded default:

```rust
        expose: pi_domain::entities::ExposeMode::parse(&row.get::<_, String>(9)?)
            .unwrap_or_default(),
```

  Place `expose` inside the `ProjectConfig { … }` (the field order is irrelevant; keep it after `hostname`). Note `host_port`/`created_at` stay at indices 7/8.

  - `UPDATE` branch → add `expose=?8`:

```rust
                    tx.execute(
                        "UPDATE projects SET repo=?2, branch=?3, compose_path=?4, service=?5, container_port=?6, hostname=?7, expose=?8 WHERE name=?1",
                        params![
                            &config.name,
                            &config.repo,
                            &config.branch,
                            &config.compose_path,
                            &config.service,
                            config.container_port,
                            &config.hostname,
                            config.expose.as_str()
                        ],
                    )
                    .map_err(storage_err)?;
```

  - `INSERT` branch → add `expose` column + bind `?9`:

```rust
                    tx.execute(
                        "INSERT INTO projects (name, repo, branch, compose_path, service, container_port, hostname, host_port, created_at, expose)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, unixepoch(), ?9)",
                        params![
                            &config.name,
                            &config.repo,
                            &config.branch,
                            &config.compose_path,
                            &config.service,
                            config.container_port,
                            &config.hostname,
                            port,
                            config.expose.as_str()
                        ],
                    )
                    .map_err(storage_err)?;
```

- [ ] **Step 8: Run the suite**

Run: `rtk cargo test --workspace`
Expected: PASS (migration test, roundtrip test, and all existing repo tests — old INSERTs in tests omit `expose` and rely on the column DEFAULT).

- [ ] **Step 9: Commit**

```bash
rtk git add crates/infrastructure/src/sqlite.rs crates/infrastructure/src/repo.rs
rtk git commit -m "feat(infra): persist project expose mode (migration #2)"
```

---

### Task 4: Parse `[ingress] expose` from `pi.toml`

**Files:**
- Modify: `crates/bin/src/cli/pitoml.rs`
- Test: `crates/bin/src/cli/pitoml.rs`

**Interfaces:**
- Consumes: `ExposeMode::parse()` (Task 1).
- Produces: `IngressSection.expose: Option<String>`; `to_project_config` maps it into `ProjectConfig.expose`.

- [ ] **Step 1: Write failing tests** — add to `crates/bin/src/cli/pitoml.rs` tests:

```rust
#[test]
fn expose_defaults_private_and_parses_lan() {
    let default_cfg = PiToml::parse(SAMPLE).unwrap().to_project_config();
    assert_eq!(default_cfg.expose, pi_domain::entities::ExposeMode::Private);

    let lan = SAMPLE.replace("port = 3000", "port = 3000\nexpose = \"lan\"");
    let lan_cfg = PiToml::parse(&lan).unwrap().to_project_config();
    assert_eq!(lan_cfg.expose, pi_domain::entities::ExposeMode::Lan);
}

#[test]
fn invalid_expose_is_rejected() {
    let bad = SAMPLE.replace("port = 3000", "port = 3000\nexpose = \"public\"");
    let err = PiToml::parse(&bad).unwrap_err().to_string();
    assert!(err.contains("expose"), "got: {err}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk cargo test -p pi expose_defaults_private_and_parses_lan invalid_expose_is_rejected`
Expected: FAIL — unknown field `expose` is ignored / no validation yet (assert fails or compile error on missing field).

- [ ] **Step 3: Add the field to `IngressSection`** — in `crates/bin/src/cli/pitoml.rs`:

```rust
#[derive(Debug, Deserialize)]
pub struct IngressSection {
    pub hostname: Option<String>,
    pub service: String,
    pub port: u16,
    #[serde(default)]
    pub expose: Option<String>,
}
```

- [ ] **Step 4: Validate in `PiToml::parse`** — add before `Ok(parsed)`:

```rust
        if let Some(expose) = &parsed.ingress.expose {
            if pi_domain::entities::ExposeMode::parse(expose).is_none() {
                anyhow::bail!(
                    "invalid [ingress].expose '{expose}' (use \"private\" or \"lan\")"
                );
            }
        }
```

- [ ] **Step 5: Map in `to_project_config`** — set the field (replace the Task 1 hardcode):

```rust
            expose: self
                .ingress
                .expose
                .as_deref()
                .and_then(pi_domain::entities::ExposeMode::parse)
                .unwrap_or_default(),
```

- [ ] **Step 6: Run the suite**

Run: `rtk cargo test --workspace`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
rtk git add crates/bin/src/cli/pitoml.rs
rtk git commit -m "feat(cli): parse and validate [ingress].expose"
```

---

### Task 5: Carry `expose` over the wire (`ProjectDto`)

**Files:**
- Modify: `crates/bin/src/proto.rs`
- Test: `crates/bin/src/proto.rs`

**Interfaces:**
- Consumes: `ExposeMode` (Task 1).
- Produces: `ProjectDto.expose: Option<String>` (`#[serde(default)]`); `From<ProjectDto>`/`From<&ProjectConfig>` carry it.

- [ ] **Step 1: Write failing tests** — add to `crates/bin/src/proto.rs` tests:

```rust
#[test]
fn expose_roundtrips_and_defaults_private_when_absent() {
    // legacy payload without `expose` -> Private
    let json = r#"{"project":{"name":"a","repo":"r","branch":"main","compose":"docker-compose.yml","service":"web","port":3000,"hostname":null},"ref":null}"#;
    let req: DeployRequest = serde_json::from_str(json).unwrap();
    let config: ProjectConfig = req.project.into();
    assert_eq!(config.expose, pi_domain::entities::ExposeMode::Private);

    // lan roundtrip
    let mut config = config;
    config.expose = pi_domain::entities::ExposeMode::Lan;
    let dto = ProjectDto::from(&config);
    assert_eq!(dto.expose.as_deref(), Some("lan"));
    let back: ProjectConfig = dto.into();
    assert_eq!(back.expose, pi_domain::entities::ExposeMode::Lan);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `rtk cargo test -p pi expose_roundtrips_and_defaults_private`
Expected: FAIL — `ProjectDto` has no field `expose`.

- [ ] **Step 3: Add the field** — in `ProjectDto`:

```rust
    #[serde(default)]
    pub expose: Option<String>,
```

- [ ] **Step 4: Map both directions** — in `From<ProjectDto> for ProjectConfig`:

```rust
            expose: dto
                .expose
                .as_deref()
                .and_then(pi_domain::entities::ExposeMode::parse)
                .unwrap_or_default(),
```

In `From<&ProjectConfig> for ProjectDto`:

```rust
            expose: Some(config.expose.as_str().to_string()),
```

(Add `use pi_domain::entities::ExposeMode;` if it makes the code cleaner; the fully-qualified path above also works.)

- [ ] **Step 5: Run the suite**

Run: `rtk cargo test --workspace`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
rtk git add crates/bin/src/proto.rs
rtk git commit -m "feat(proto): carry expose mode in ProjectDto (serde default)"
```

---

### Task 6: `HostNetwork` contract + `UdpHostNetwork`

**Files:**
- Modify: `crates/domain/src/contracts.rs` (new trait)
- Create: `crates/infrastructure/src/hostnet.rs`
- Modify: `crates/infrastructure/src/lib.rs` (`pub mod hostnet;`)
- Test: `crates/infrastructure/src/hostnet.rs`

**Interfaces:**
- Produces:
  - `trait HostNetwork: Send + Sync { fn primary_ipv4(&self) -> Option<std::net::IpAddr>; }` (under `#[cfg_attr(feature = "mocks", automock)]`) → generates `MockHostNetwork`.
  - `UdpHostNetwork` with `pub fn new() -> Arc<UdpHostNetwork>`.

- [ ] **Step 1: Add the trait to `contracts.rs`**:

```rust
/// Detects the agent host's primary LAN IPv4 for building reachable URLs
/// (used by `pi deploy`/`pi ls` for expose=lan projects). None when undetectable.
#[cfg_attr(feature = "mocks", automock)]
pub trait HostNetwork: Send + Sync {
    fn primary_ipv4(&self) -> Option<std::net::IpAddr>;
}
```

- [ ] **Step 2: Write the smoke test** — create `crates/infrastructure/src/hostnet.rs`:

```rust
use std::net::{IpAddr, UdpSocket};
use std::sync::Arc;

use pi_domain::contracts::HostNetwork;

/// Detects the outbound-route interface IP via the UDP-connect trick: connecting
/// a UDP socket sets its local address to the interface that would carry traffic
/// to the target, without sending any packet.
pub struct UdpHostNetwork;

impl UdpHostNetwork {
    pub fn new() -> Arc<UdpHostNetwork> {
        Arc::new(UdpHostNetwork)
    }
}

impl HostNetwork for UdpHostNetwork {
    fn primary_ipv4(&self) -> Option<IpAddr> {
        let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
        socket.connect("8.8.8.8:80").ok()?;
        let addr = socket.local_addr().ok()?;
        match addr.ip() {
            IpAddr::V4(v4) if !v4.is_unspecified() && !v4.is_loopback() => Some(IpAddr::V4(v4)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_ipv4_does_not_panic() {
        // Environment-dependent value; we only assert it returns without panic
        // and, when Some, is a non-loopback IPv4.
        if let Some(ip) = UdpHostNetwork.primary_ipv4() {
            assert!(matches!(ip, IpAddr::V4(v4) if !v4.is_loopback()));
        }
    }
}
```

- [ ] **Step 3: Register the module** — add to `crates/infrastructure/src/lib.rs`:

```rust
pub mod hostnet;
```

- [ ] **Step 4: Run tests**

Run: `rtk cargo test -p pi-infrastructure hostnet`
Expected: PASS. Also run `rtk cargo build --workspace` to confirm `MockHostNetwork` generates under the `mocks` feature.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/domain/src/contracts.rs crates/infrastructure/src/hostnet.rs crates/infrastructure/src/lib.rs
rtk git commit -m "feat: add HostNetwork contract and UdpHostNetwork adapter"
```

---

### Task 7: `pi deploy` prints the LAN URL

**Files:**
- Modify: `crates/application/src/deploy.rs` (`DeployProject` + `new` + `run_stages`)
- Modify: `crates/bin/src/agent/state.rs` (wire `UdpHostNetwork`)
- Test: `crates/application/src/deploy.rs`

**Interfaces:**
- Consumes: `HostNetwork` (Task 6), `ExposeMode` (Task 1).
- Produces: `DeployProject::new(…, host_network: Arc<dyn HostNetwork>, …)` — appended as a new parameter after `ingress`.

- [ ] **Step 1: Write the failing test** — add to `crates/application/src/deploy.rs` tests. Extend the mock-deps harness (`MockDeps`/`build`) to include a `MockHostNetwork`, then a test that a lan deploy logs the URL. Add `host_network: MockHostNetwork` to the harness struct and its construction, and to the `DeployProject::new(...)` call (pass `Arc::new(m.host_network)` in the right position). The test:

```rust
#[tokio::test]
async fn lan_deploy_logs_reachable_url() {
    let mut m = MockDeps::new();
    // ... arrange the usual happy path (fetch/secrets/override/build/up/health ok) ...
    m.host_network
        .expect_primary_ipv4()
        .returning(|| Some("192.168.1.50".parse().unwrap()));
    // config.expose = ExposeMode::Lan for this deploy
    // run execute(...) with a CollectSink and assert:
    assert!(sink.lines().iter().any(|l| l == "lan: http://192.168.1.50:8000"));
}
```

For the IP-not-detected branch, a second test with `expect_primary_ipv4().returning(|| None)` asserts a line containing `"lan: 8000 (ip not detected)"` and that the deploy still succeeds.

> Implementer note: model these on the existing happy-path deploy test in this file; reuse its mock arrangements. Default `host_network` in `MockDeps::new()` should expect no calls (private path) — use `.expect_primary_ipv4().times(0)` in private tests or leave it unset and only set expectations in lan tests.

- [ ] **Step 2: Run test to verify it fails**

Run: `rtk cargo test -p pi-application lan_deploy_logs_reachable_url`
Expected: FAIL — `DeployProject` has no `host_network` / no lan log line.

- [ ] **Step 3: Add the field + constructor param** — in `crates/application/src/deploy.rs`:

Struct: add after `ingress: Arc<dyn Ingress>,`

```rust
    host_network: Arc<dyn HostNetwork>,
```

`new(...)`: add the parameter right after `ingress: Arc<dyn Ingress>,`:

```rust
        host_network: Arc<dyn HostNetwork>,
```

and `host_network,` in the struct initializer. Add `HostNetwork` to the `use pi_domain::contracts::{…}` import.

- [ ] **Step 4: Emit the log line** — in `run_stages`, right after the health check succeeds (`self.health.check(...).await?;`), before the hostname block:

```rust
        // §12.1: LAN reachability hint for expose=lan projects.
        if config.expose == ExposeMode::Lan {
            match self.host_network.primary_ipv4() {
                Some(ip) => log.line(&format!("lan: http://{ip}:{}", project.host_port)),
                None => log.line(&format!("lan: {} (ip not detected)", project.host_port)),
            }
        }
```

Add `ExposeMode` to the `use pi_domain::entities::{…}` import at the top of `deploy.rs`.

- [ ] **Step 5: Wire the adapter in `state.rs`** — in `build_state`, after `runtime` is created:

```rust
    let host_network = pi_infrastructure::hostnet::UdpHostNetwork::new();
```

Pass it into `DeployProject::new(...)` in the same position as the new parameter (after `ingress`):

```rust
        ingress,
        Arc::clone(&host_network) as Arc<dyn pi_domain::contracts::HostNetwork>,
        SystemClock::new(),
```

- [ ] **Step 6: Run the suite**

Run: `rtk cargo test --workspace`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
rtk git add crates/application/src/deploy.rs crates/bin/src/agent/state.rs
rtk git commit -m "feat(deploy): print LAN URL after health for expose=lan"
```

---

### Task 8: `pi ls` exposes `expose` + `lan_ip` over the wire

**Files:**
- Modify: `crates/application/src/list.rs` (`ProjectView` + `ListProjects` + `new`)
- Modify: `crates/bin/src/proto.rs` (`ProjectViewDto` + `From<ProjectView>`)
- Modify: `crates/bin/src/agent/state.rs` (wire `host_network` into `ListProjects`)
- Test: `crates/application/src/list.rs`, `crates/bin/src/proto.rs`

**Interfaces:**
- Consumes: `HostNetwork` (Task 6), `ExposeMode` (Task 1).
- Produces:
  - `ProjectView.expose: ExposeMode`, `ProjectView.lan_ip: Option<std::net::IpAddr>`.
  - `ListProjects::new(projects, runtime, host_network)`.
  - `ProjectViewDto.expose: Option<String>`, `ProjectViewDto.lan_ip: Option<String>` (`#[serde(default)]`).

- [ ] **Step 1: Write the failing list test** — in `crates/application/src/list.rs` tests: update the existing `project(...)` helper to allow setting expose; add `host_network` (a `MockHostNetwork`) to `ListProjects::new` calls. New test:

```rust
#[tokio::test]
async fn lan_projects_get_ip_private_projects_do_not() {
    let mut projects = MockProjectRepository::new();
    projects.expect_list().returning(|| {
        let mut lan = project("lan-app", 8000);
        lan.config.expose = pi_domain::entities::ExposeMode::Lan;
        Ok(vec![lan, project("priv-app", 8001)])
    });
    let mut runtime = MockContainerRuntime::new();
    runtime.expect_ps().returning(|_| Ok(vec![]));
    let mut net = pi_domain::contracts::MockHostNetwork::new();
    net.expect_primary_ipv4().returning(|| Some("192.168.1.50".parse().unwrap()));

    let list = ListProjects::new(Arc::new(projects), Arc::new(runtime), Arc::new(net));
    let views = list.execute().await.unwrap();

    let lan = views.iter().find(|v| v.name == "lan-app").unwrap();
    let priv_ = views.iter().find(|v| v.name == "priv-app").unwrap();
    assert_eq!(lan.expose, pi_domain::entities::ExposeMode::Lan);
    assert_eq!(lan.lan_ip, Some("192.168.1.50".parse().unwrap()));
    assert_eq!(priv_.expose, pi_domain::entities::ExposeMode::Private);
    assert_eq!(priv_.lan_ip, None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `rtk cargo test -p pi-application lan_projects_get_ip`
Expected: FAIL — `ProjectView` has no `expose`/`lan_ip`; `ListProjects::new` takes 2 args.

- [ ] **Step 3: Update `ProjectView` + `ListProjects`** — in `crates/application/src/list.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectView {
    pub name: String,
    pub repo: String,
    pub branch: String,
    pub hostname: Option<String>,
    pub host_port: u16,
    pub expose: ExposeMode,
    pub lan_ip: Option<std::net::IpAddr>,
    pub services: Vec<ServiceState>,
}
```

Add imports: `use pi_domain::contracts::{ContainerRuntime, HostNetwork, ProjectRepository};` and `use pi_domain::entities::{ExposeMode, ServiceState};`.

Struct + `new`:

```rust
pub struct ListProjects {
    projects: Arc<dyn ProjectRepository>,
    runtime: Arc<dyn ContainerRuntime>,
    host_network: Arc<dyn HostNetwork>,
}

impl ListProjects {
    pub fn new(
        projects: Arc<dyn ProjectRepository>,
        runtime: Arc<dyn ContainerRuntime>,
        host_network: Arc<dyn HostNetwork>,
    ) -> Arc<ListProjects> {
        Arc::new(ListProjects { projects, runtime, host_network })
    }
```

`execute`: detect the IP once, attach to lan projects only:

```rust
    pub async fn execute(&self) -> Result<Vec<ProjectView>, DomainError> {
        let lan_ip = self.host_network.primary_ipv4();
        let mut views = Vec::new();
        for project in self.projects.list().await? {
            let services = self
                .runtime
                .ps(&project.config.name)
                .await
                .unwrap_or_default();
            let expose = project.config.expose;
            views.push(ProjectView {
                name: project.config.name,
                repo: project.config.repo,
                branch: project.config.branch,
                hostname: project.config.hostname,
                host_port: project.host_port,
                expose,
                lan_ip: if expose == ExposeMode::Lan { lan_ip } else { None },
                services,
            });
        }
        Ok(views)
    }
```

- [ ] **Step 4: Update the `proto.rs` DTO + test** — add fields to `ProjectViewDto`:

```rust
    #[serde(default)]
    pub expose: Option<String>,
    #[serde(default)]
    pub lan_ip: Option<String>,
```

`From<ProjectView>`:

```rust
            expose: Some(v.expose.as_str().to_string()),
            lan_ip: v.lan_ip.map(|ip| ip.to_string()),
```

Add a proto test:

```rust
#[test]
fn project_view_dto_carries_expose_and_lan_ip() {
    let view = ProjectView {
        name: "a".into(),
        repo: "r".into(),
        branch: "main".into(),
        hostname: None,
        host_port: 8000,
        expose: pi_domain::entities::ExposeMode::Lan,
        lan_ip: Some("192.168.1.50".parse().unwrap()),
        services: vec![],
    };
    let dto = ProjectViewDto::from(view);
    assert_eq!(dto.expose.as_deref(), Some("lan"));
    assert_eq!(dto.lan_ip.as_deref(), Some("192.168.1.50"));
}
```

- [ ] **Step 5: Wire `state.rs`** — pass the already-created `host_network` into `ListProjects::new`:

```rust
    let list = ListProjects::new(
        projects.clone(),
        runtime.clone(),
        Arc::clone(&host_network) as Arc<dyn pi_domain::contracts::HostNetwork>,
    );
```

- [ ] **Step 6: Run the suite**

Run: `rtk cargo test --workspace`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
rtk git add crates/application/src/list.rs crates/bin/src/proto.rs crates/bin/src/agent/state.rs
rtk git commit -m "feat(ls): expose mode and detected LAN ip in project view"
```

---

### Task 9: Render `expose=lan` + URL in `pi ls`

**Files:**
- Modify: `crates/bin/src/cli/commands.rs` (`ls` + a testable formatter)
- Test: `crates/bin/src/cli/commands.rs`

**Interfaces:**
- Consumes: `ProjectViewDto.expose` / `.lan_ip` (Task 8).
- Produces: `fn expose_cell(expose: Option<&str>, lan_ip: Option<&str>, host_port: u16) -> String` (testable).

- [ ] **Step 1: Write the failing test** — add to `crates/bin/src/cli/commands.rs` tests:

```rust
#[test]
fn expose_cell_shows_lan_url_only_for_lan() {
    assert_eq!(expose_cell(Some("private"), None, 8000), "-".to_string());
    assert_eq!(expose_cell(None, None, 8000), "-".to_string());
    assert_eq!(
        expose_cell(Some("lan"), Some("192.168.1.50"), 8000),
        "lan http://192.168.1.50:8000".to_string()
    );
    assert_eq!(
        expose_cell(Some("lan"), None, 8000),
        "lan (ip n/a)".to_string()
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `rtk cargo test -p pi expose_cell_shows_lan_url`
Expected: FAIL — `expose_cell` not defined.

- [ ] **Step 3: Implement the formatter + use it** — add to `crates/bin/src/cli/commands.rs`:

```rust
/// `pi ls` EXPOSE cell: blank-ish for private, URL for lan (§12.1).
fn expose_cell(expose: Option<&str>, lan_ip: Option<&str>, host_port: u16) -> String {
    match expose {
        Some("lan") => match lan_ip {
            Some(ip) => format!("lan http://{ip}:{host_port}"),
            None => "lan (ip n/a)".to_string(),
        },
        _ => "-".to_string(),
    }
}
```

Update the `ls` printer to add an EXPOSE column. Header:

```rust
    println!(
        "{:<16} {:<10} {:<28} {:<6} {:<28} SERVICES",
        "NAME", "BRANCH", "HOSTNAME", "PORT", "EXPOSE"
    );
```

Row (compute `expose` cell before printing):

```rust
        let expose = expose_cell(p.expose.as_deref(), p.lan_ip.as_deref(), p.host_port);
        println!(
            "{:<16} {:<10} {:<28} {:<6} {:<28} {services}",
            p.name,
            p.branch,
            p.hostname.unwrap_or_else(|| "-".into()),
            p.host_port,
            expose
        );
```

- [ ] **Step 4: Run the suite**

Run: `rtk cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/bin/src/cli/commands.rs
rtk git commit -m "feat(ls): render expose column with LAN url"
```

---

### Task 10: Version bump + README documentation

**Files:**
- Modify: `Cargo.toml` (workspace version)
- Modify: `README.md`
- Test: full workspace build/test

**Interfaces:**
- Consumes: everything above.
- Produces: shipped docs + `0.3.1`.

- [ ] **Step 1: Bump the version** — `Cargo.toml:6`:

```toml
version = "0.3.1"
```

- [ ] **Step 2: Update the README status line** — change `Current status: \`v0.3.0\`.` to `Current status: \`v0.3.1\`.` and add `expose = "private" | "lan"` to the feature list and to the `[ingress]` field docs:

In the "Fields:" list under `pi.toml`, add:

```text
- `ingress.expose` is optional; `"private"` (default, bind 127.0.0.1) or `"lan"`
  (bind 0.0.0.0, reachable from the local network).
```

- [ ] **Step 3: Add a security/Docker note** — in the "Docker Compose Requirements" section (near the override example), document that the override binds `expose` (private → `127.0.0.1`, lan → `0.0.0.0`), plus a warning block:

```text
`expose = "lan"` opens the host port on all interfaces. On a Pi behind a home
router with no port forwarding this is effectively LAN-only; on a Pi with a
public IP and no NAT it becomes publicly reachable. pi does not manage the
firewall. Note that publishing on 0.0.0.0 on Linux inserts iptables rules that
can bypass a host firewall such as ufw.
```

- [ ] **Step 4: Update the override example** — show the lan variant of the generated override:

```yaml
services:
  web:
    ports:
      - "0.0.0.0:8000:3000"   # when [ingress].expose = "lan"
```

- [ ] **Step 5: Run the full suite + clippy**

Run: `rtk cargo test --workspace && rtk cargo clippy --workspace`
Expected: PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
rtk git add Cargo.toml README.md
rtk git commit -m "docs: document expose=lan; bump to v0.3.1"
```

---

## Self-Review

**Spec coverage:**
- §1/§2 expose enum, default private, 0.0.0.0 bind, no firewall → Tasks 1, 2, 4, 10.
- §2.2 orthogonal to Cloudflare (0.0.0.0 includes loopback) → no code change needed; documented Task 10. Hostname path in `run_stages` untouched.
- §2.3 agent detects LAN IP (UDP trick) → Task 6.
- §2.4 graceful degradation (None → no panic, deploy ok) → Tasks 6, 7 (ip-not-detected test), 9 (`lan (ip n/a)`).
- §2.6 healthcheck unchanged → untouched (verified: `health.check` still uses `host_port` on 127.0.0.1; 0.0.0.0 includes loopback).
- §3 pi.toml surface + `pi ls`/`pi deploy` output → Tasks 4, 7, 9.
- §4 domain (`ExposeMode`, `HostNetwork`, `OverrideStore` bind) → Tasks 1, 2, 6.
- §5 infra (override bind, migration, repo, UdpHostNetwork) → Tasks 2, 3, 6.
- §6 application (deploy bind + lan line, list expose + lan_ip) → Tasks 2, 7, 8.
- §7 wire + CLI → Tasks 4, 5, 8, 9.
- §8 tests → embedded per task.
- §9 docs → Task 10.
- Extra coverage beyond spec: `pi env send --apply` also threads bind from `registered.config.expose` (Task 2) so a running LAN stack keeps its binding on env re-apply.

**Placeholder scan:** no TBD/TODO; every code step shows concrete code; test steps include assertions.

**Type consistency:** `ExposeMode { Private, Lan }`, `.as_str()`/`.bind_addr()`/`.parse()` used identically across Tasks 1–9. `OverrideStore::write(project, service, bind, host_port, container_port)` — 5 args used in contracts, infra, deploy, env (Tasks 2). `HostNetwork::primary_ipv4() -> Option<IpAddr>` used in Tasks 6–8. `ProjectView`/`ProjectViewDto` field names (`expose`, `lan_ip`) match across Tasks 8–9. `ListProjects::new` 3-arg form consistent in list.rs + state.rs (Task 8). `DeployProject::new` extra `host_network` param after `ingress` consistent in deploy.rs + state.rs (Task 7).
