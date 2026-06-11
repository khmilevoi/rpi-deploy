# pi v0.2 (Секреты + ingress) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** добавление нового проекта не требует ручной настройки на Pi (§23 v0.2): `pi env send/ls` доставляет секреты (age-шифрование на диске, маскировка в логах), деплой сам правит cloudflared ingress + DNS (diff-restart), и проходит health-check-гейт (docker → HTTP → TCP).

**Architecture:** расширяем слои v0.1 по тем же правилам (§5): в `domain` — сущности `EnvBundle`/`HealthcheckConfig` и контракты `SecretStore`/`EnvFileWriter`/`HealthGate`/`Ingress`; в `application` — `MaskingSink`, use-cases `SendEnv`/`ListEnvKeys` и интеграция новых стадий в `DeployProject`; в `infrastructure` — адаптеры `EncryptedFileStore` (age), `FsEnvFileWriter`, `HybridHealthGate`, `CloudflaredIngress`; в `bin` — env-эндпоинты `/v1`, секции `[healthcheck]`/`[env]` в `pi.toml`, `[cloudflared]` в `agent.toml`, команды `pi env send/ls`.

**Tech Stack:** как v0.1 + `age = "0.11"` (top-level `age::encrypt`/`age::decrypt`, ключ `x25519::Identity`), `serde_yaml = "0.9"` (правка cloudflared `config.yml`), `reqwest` в `infrastructure` (HTTP-проба health-гейта).

**Спека:** `docs/superpowers/specs/2026-06-09-pi-deploy-tool-design.md` — §8 (поток + health-гейт), §8.1 (маскировка), §10 (секреты), §11 (ingress), §23 v0.2 (границы скоупа).

---

## Скоуп v0.2 (что входит / что НЕ входит)

Входит (§23 v0.2):
- `EncryptedFileStore` (age): bundle в `<data_dir>/secrets/<project>.env.age`, ключ агента `<data_dir>/secret.key` (`0600`), генерится при первом старте.
- `pi env send [--file] [--apply]` / `pi env ls`; API `PUT/GET /v1/projects/{name}/env`.
- Инжект `.env` в workdir при каждом деплое (и при `--apply`); файл остаётся (`0600`).
- Маскировка значений EnvBundle (≥6 символов → `***KEY***`) во всех потоках: SSE-стрим, `log_tail` в БД (§8.1).
- Health-check-гейт после `up`: docker healthcheck → HTTP GET → TCP-connect; провал = `failed`, стек не сносится (§8).
- `CloudflaredIngress`: upsert ingress-правила в `config.yml`, `cloudflared tunnel route dns`, рестарт юнита **только при diff** (§11). Без `[cloudflared]` в `agent.toml` — `DisabledIngress` (лог-подсказка, деплой не падает).

НЕ входит (позже по роадмапу): очередь latest-wins / поэтапные таймауты / `--cancel` / свип при старте / build-семафор / GC (v0.3); `pi doctor` / `stats` / lifecycle / `pi rm` (v0.4); `pi agent setup` / `pi setup` / `pi init` / бутстрап cloudflared (v0.5 — в v0.2 туннель создаётся вручную один раз, см. Task 14). Авто-удаление DNS и Cloudflare API token — §21.

Решения, зафиксированные планом (в рамках §22):
- `HealthcheckConfig` едет в `ProjectConfig` из `pi.toml` при каждом деплое и **не персистится** в SQLite (это вход деплоя, не состояние реестра) — миграция БД не нужна; `HealthGate.check(config, host_port, log)` берёт порт из реестра, конфиг — из запроса.
- Docker-health читается из `docker compose ps --format json` (поле `Health`) — новый метод в `ContainerRuntime` не нужен, расширяется `ServiceState`.
- `expect` — строка `"2xx" | "3xx" | "<код>"` (дефолт без поля — 2xx/3xx); `timeout` — `"60s"/"2m"` (дефолт 60s); интервал проб — 2s (констромка адаптера, в тестах настраивается).
- Маскировка: `MaskingSink` создаётся пустым до расшифровки bundle и «заряжается» (`arm`) сразу после неё — значения не могут утечь раньше, чем стали известны процессу. Порог 6 символов (§8.1).
- `pi env send` для ещё не задеплоенного проекта **работает** (bundle сохраняется впрок — сценарий `pi init` → `env send` → `deploy`, §15); `--apply` для незадеплоенного — `404`.
- dotenv-формат: `KEY=VALUE` построчно, `#`-комментарии, опц. `export `, опц. одинарные/двойные кавычки вокруг значения; multi-line значения не поддерживаются (валидация на `PUT`).

## Конвенции для исполнителя

- **Все команды** запускать с префиксом `rtk`: `rtk cargo test`, `rtk git add …`.
- Коммит-сообщения — conventional commits на английском; завершать трейлером `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Разработка на **Windows**, целевая платформа агента — Linux. Права файлов (`0600`) — под `#[cfg(unix)]`. Юнит-тесты обязаны проходить на Windows; интеграция с реальным cloudflared — `#[ignore]`.
- Код и комментарии — на английском (в репо всё переведено, держать так).
- Без `unwrap()`/`expect()` в use-cases/адаптерах — ошибки через `Result` (§19). В тестах `unwrap()` допустим.
- После каждого зелёного шага — коммит. Один таск = один коммит.

## File Structure

```
crates/
├─ domain/src/
│  ├─ entities.rs        # MOD: +EnvBundle, +HealthcheckConfig, ProjectConfig.healthcheck, ServiceState.health
│  ├─ error.rs           # MOD: +Secrets, +Ingress, +HealthCheck
│  └─ contracts.rs       # MOD: +SecretStore, +EnvFileWriter, +HealthGate, +Ingress; Source::workdir; ProjectRepository::get
├─ application/src/
│  ├─ mask.rs            # NEW: MaskingSink (§8.1)
│  ├─ env.rs             # NEW: SendEnv, ListEnvKeys (§7, §10)
│  ├─ deploy.rs          # MOD: стадии env-inject / health / ingress + masking
│  └─ lib.rs             # MOD: +mod mask, +mod env
├─ infrastructure/src/
│  ├─ dotenv.rs          # NEW: parse/serialize dotenv (общий для клиента и агента)
│  ├─ secrets.rs         # NEW: EncryptedFileStore (age)
│  ├─ envfile.rs         # NEW: FsEnvFileWriter (.env в workdir, 0600)
│  ├─ health.rs          # NEW: HybridHealthGate (docker → HTTP → TCP)
│  ├─ cloudflared.rs     # NEW: CloudflaredIngress + DisabledIngress
│  ├─ docker.rs          # MOD: parse_ps_json читает поле Health
│  └─ lib.rs             # MOD: +5 модулей
└─ bin/src/
   ├─ proto.rs           # MOD: +EnvSendRequest/Response, +EnvKeysResponse, +HealthcheckDto в ProjectDto
   ├─ agent/config.rs    # MOD: +[cloudflared] секция
   ├─ agent/state.rs     # MOD: wiring новых адаптеров + use-cases
   ├─ agent/http.rs      # MOD: PUT/GET /v1/projects/{name}/env, TracingSink
   ├─ cli/pitoml.rs      # MOD: +[env], +[healthcheck], parse_duration_secs
   ├─ cli/api.rs         # MOD: send_env, env_keys
   ├─ cli/commands.rs    # MOD: env_send, env_ls
   └─ main.rs            # MOD: pi env send|ls
docs/install-agent-v0.1.md   # MOD: cloudflared-бутстрап, [cloudflared], приёмка v0.2
dev/agent.toml               # MOD: пример [cloudflared] (закомментирован)
Cargo.toml                   # MOD: +age, +serde_yaml; version 0.2.0 (Task 14)
```

---

### Task 1: Domain — EnvBundle, HealthcheckConfig, health в ServiceState, новые ошибки

**Files:**
- Modify: `crates/domain/src/entities.rs`
- Modify: `crates/domain/src/error.rs`
- Modify (компил-фоллаут, только новые поля с дефолтами): `crates/application/src/deploy.rs`, `crates/application/src/list.rs`, `crates/infrastructure/src/repo.rs`, `crates/infrastructure/src/git.rs`, `crates/infrastructure/src/docker.rs`, `crates/bin/src/proto.rs`, `crates/bin/src/cli/pitoml.rs`

- [ ] **Step 1: Написать падающие тесты в `entities.rs`**

Добавить в `mod tests` файла `crates/domain/src/entities.rs`:

```rust
    #[test]
    fn env_bundle_default_is_empty_and_keys_are_sorted() {
        let mut bundle = EnvBundle::default();
        assert!(bundle.is_empty());
        bundle.vars.insert("Z_KEY".into(), "1".into());
        bundle.vars.insert("A_KEY".into(), "2".into());
        assert!(!bundle.is_empty());
        assert_eq!(bundle.keys(), vec!["A_KEY".to_string(), "Z_KEY".to_string()]);
    }

    #[test]
    fn healthcheck_defaults_match_spec() {
        let hc = HealthcheckConfig::default();
        assert_eq!(hc.path, None);
        assert_eq!(hc.expect, None);
        assert_eq!(hc.timeout_secs, 60);
    }
```

- [ ] **Step 2: Запустить — убедиться, что падает**

Run: `rtk cargo test -p pi-domain`
Expected: FAIL — `EnvBundle`/`HealthcheckConfig` not found.

- [ ] **Step 3: Реализовать сущности**

В начало `crates/domain/src/entities.rs` добавить импорт и типы:

```rust
use std::collections::BTreeMap;

/// Project secrets: key -> value (§4). Values never leave the agent unmasked.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EnvBundle {
    pub vars: BTreeMap<String, String>,
}

impl EnvBundle {
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }

    /// Key names only (sorted, BTreeMap order) — what `pi env ls` shows (§10).
    pub fn keys(&self) -> Vec<String> {
        self.vars.keys().cloned().collect()
    }
}

/// Deploy gate settings from [healthcheck] in pi.toml (§8, §12).
/// Per-deploy input: travels with ProjectConfig, not persisted in the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthcheckConfig {
    /// HTTP probe path; None => plain TCP connect (when no docker healthcheck).
    pub path: Option<String>,
    /// Expected HTTP status: "2xx" | "3xx" | exact like "204". None => 2xx/3xx.
    pub expect: Option<String>,
    /// Total gate budget in seconds.
    pub timeout_secs: u64,
}

impl Default for HealthcheckConfig {
    fn default() -> HealthcheckConfig {
        HealthcheckConfig { path: None, expect: None, timeout_secs: 60 }
    }
}
```

В `ProjectConfig` добавить поле (после `hostname`):

```rust
    /// Health gate settings ([healthcheck] from pi.toml). Not persisted in DB.
    pub healthcheck: HealthcheckConfig,
```

В `ServiceState` добавить поле:

```rust
    /// Docker healthcheck state ("healthy"/"unhealthy"/"starting"), None when
    /// the service declares no healthcheck.
    pub health: Option<String>,
```

- [ ] **Step 4: Новые варианты ошибок**

`crates/domain/src/error.rs` — добавить в enum:

```rust
    #[error("secret store error: {0}")]
    Secrets(String),
    #[error("ingress error: {0}")]
    Ingress(String),
    #[error("health check failed: {0}")]
    HealthCheck(String),
```

- [ ] **Step 5: Починить компиляцию по всему workspace**

Во всех местах, где конструируется `ProjectConfig`, добавить `healthcheck: HealthcheckConfig::default(),` (импортируя `HealthcheckConfig`); где конструируется `ServiceState` — `health: None`:

- `crates/application/src/deploy.rs` — `sample_config()` в тестах.
- `crates/application/src/list.rs` — `project()` в тестах; оба литерала `ServiceState` в тестах получают `health: None`.
- `crates/infrastructure/src/repo.rs` — `row_to_project` (поле `healthcheck: HealthcheckConfig::default()` с комментарием `// per-deploy input, not stored`) и `cfg()` в тестах.
- `crates/infrastructure/src/git.rs` — `cfg()` в `mod integration`.
- `crates/infrastructure/src/docker.rs` — `service_state()` пока возвращает `health: None` (парсинг — Task 6); литералы `ServiceState` в тестах — `health: None`.
- `crates/bin/src/proto.rs` — `From<ProjectDto> for ProjectConfig`: `healthcheck: HealthcheckConfig::default(),` (реальный DTO — Task 12).
- `crates/bin/src/cli/pitoml.rs` — `to_project_config()`: `healthcheck: HealthcheckConfig::default(),` (реальный парсинг — Task 12).

- [ ] **Step 6: Прогнать тесты**

Run: `rtk cargo test --workspace`
Expected: PASS (все старые + 2 новых).

- [ ] **Step 7: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(domain): EnvBundle, HealthcheckConfig, service health state, new error variants"
```

### Task 2: Domain-контракты + реализации `Source::workdir` и `ProjectRepository::get`

**Files:**
- Modify: `crates/domain/src/contracts.rs`
- Modify: `crates/infrastructure/src/git.rs` (impl `workdir`)
- Modify: `crates/infrastructure/src/repo.rs` (impl `get`)
- Modify (фоллаут): `crates/bin/src/agent/http.rs` (`GatedSource` в тестах)

- [ ] **Step 1: Падающие тесты на новые методы адаптеров**

`crates/infrastructure/src/git.rs`, в `mod tests`:

```rust
    #[test]
    fn workdir_is_under_data_dir_workdirs() {
        let source = GitSource::new(std::path::Path::new("/var/lib/pi"));
        assert_eq!(
            source.workdir("rateme"),
            std::path::PathBuf::from("/var/lib/pi/workdirs/rateme")
        );
    }
```

`crates/infrastructure/src/repo.rs`, в `mod tests`:

```rust
    #[tokio::test]
    async fn get_returns_upserted_project_and_none_for_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir, 8000, 8999);
        repo.upsert(&cfg("a")).await.unwrap();
        let found = repo.get("a").await.unwrap().unwrap();
        assert_eq!(found.config.name, "a");
        assert_eq!(found.host_port, 8000);
        assert!(repo.get("nope").await.unwrap().is_none());
    }
```

- [ ] **Step 2: Запустить — убедиться, что падает**

Run: `rtk cargo test -p pi-infrastructure workdir_is_under -- --nocapture` и `rtk cargo test -p pi-infrastructure get_returns`
Expected: FAIL — методов нет.

- [ ] **Step 3: Расширить контракты**

`crates/domain/src/contracts.rs`. В импорт сущностей добавить `EnvBundle, ProjectConfig` (уже есть) и в `use std::path::PathBuf;` добавить `Path`:

```rust
use std::path::{Path, PathBuf};
```

В трейт `Source` добавить метод:

```rust
    /// Where this project's working copy lives on the agent host (used by
    /// `pi env send --apply` to re-inject .env without a fetch, §10).
    fn workdir(&self, project_name: &str) -> PathBuf;
```

В трейт `ProjectRepository` добавить:

```rust
    async fn get(&self, name: &str) -> Result<Option<Project>, DomainError>;
```

Новые контракты (после `OverrideStore`):

```rust
/// Store/retrieve the project EnvBundle, encrypted at rest (§6, §10).
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn save(&self, project: &str, bundle: &EnvBundle) -> Result<(), DomainError>;
    /// Empty bundle when nothing is stored for the project.
    async fn load(&self, project: &str) -> Result<EnvBundle, DomainError>;
}

/// Writes the decrypted bundle as `.env` into the project workdir (§10).
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait EnvFileWriter: Send + Sync {
    /// Fails with NotFound when the workdir does not exist (never deployed).
    async fn write(&self, workdir: &Path, bundle: &EnvBundle) -> Result<(), DomainError>;
}

/// Deploy gate (§8): hybrid docker healthcheck -> HTTP -> TCP.
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait HealthGate: Send + Sync {
    async fn check(
        &self,
        config: &ProjectConfig,
        host_port: u16,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError>;
}

/// Routes hostname -> 127.0.0.1:host_port on the edge (§6, §11).
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait Ingress: Send + Sync {
    async fn upsert(
        &self,
        hostname: &str,
        host_port: u16,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError>;
}
```

- [ ] **Step 4: Реализации**

`crates/infrastructure/src/git.rs` — в `impl Source for GitSource` добавить и переиспользовать в `fetch`:

```rust
    fn workdir(&self, project_name: &str) -> PathBuf {
        self.workdirs.join(project_name)
    }
```

(в `fetch` заменить `let workdir = self.workdirs.join(&project.name);` на `let workdir = self.workdir(&project.name);`).

`crates/infrastructure/src/repo.rs` — в `impl ProjectRepository for SqliteProjectRepo`:

```rust
    async fn get(&self, name: &str) -> Result<Option<Project>, DomainError> {
        let name = name.to_string();
        self.db
            .call(move |conn| {
                conn.query_row(
                    &format!("{SELECT} WHERE name = ?1"),
                    params![name],
                    row_to_project,
                )
                .optional()
                .map_err(storage_err)
            })
            .await
    }
}
```

`crates/bin/src/agent/http.rs` — тестовый `GatedSource` обязан реализовать новый метод:

```rust
            fn workdir(&self, project_name: &str) -> std::path::PathBuf {
                std::env::temp_dir().join(project_name)
            }
```

- [ ] **Step 5: Прогнать тесты**

Run: `rtk cargo test --workspace`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(domain): SecretStore/EnvFileWriter/HealthGate/Ingress contracts, Source::workdir, ProjectRepository::get"
```

---

### Task 3: `dotenv` — parse/serialize (infrastructure)

**Files:**
- Create: `crates/infrastructure/src/dotenv.rs`
- Modify: `crates/infrastructure/src/lib.rs` (`pub mod dotenv;`)

- [ ] **Step 1: Создать модуль с падающими тестами**

`crates/infrastructure/src/dotenv.rs`:

```rust
//! Dotenv parse/serialize shared by the CLI (`pi env send` reads a local
//! file) and the agent (bundle payload <-> workdir `.env`) (§10, §12).

use pi_domain::entities::EnvBundle;

/// `KEY=VALUE` lines; skips blanks and `#` comments; strips an optional
/// `export ` prefix and one pair of matching single/double quotes.
pub fn parse(text: &str) -> Result<EnvBundle, String> {
    todo!()
}

/// `[A-Za-z_][A-Za-z0-9_]*` — also used to validate PUT /env payloads.
pub fn is_valid_key(key: &str) -> bool {
    todo!()
}

/// Deterministic KEY=VALUE serialization (BTreeMap order, trailing newline).
pub fn serialize(bundle: &EnvBundle) -> String {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comments_export_quotes_and_equals_in_value() {
        let text = "# comment\n\nDB_URL=postgres://u:p@db/x?a=b\nexport TOKEN=\"abc=def\"\nNAME='single'\n";
        let bundle = parse(text).unwrap();
        assert_eq!(bundle.vars["DB_URL"], "postgres://u:p@db/x?a=b");
        assert_eq!(bundle.vars["TOKEN"], "abc=def");
        assert_eq!(bundle.vars["NAME"], "single");
        assert_eq!(bundle.vars.len(), 3);
    }

    #[test]
    fn rejects_invalid_keys_and_lines_without_equals() {
        assert!(parse("1BAD=x").unwrap_err().contains("line 1"));
        assert!(parse("OK=1\nno-equals-here").unwrap_err().contains("line 2"));
        assert!(is_valid_key("_OK_2"));
        assert!(!is_valid_key("BAD-DASH"));
        assert!(!is_valid_key(""));
    }

    #[test]
    fn serialize_then_parse_roundtrips() {
        let mut bundle = EnvBundle::default();
        bundle.vars.insert("B".into(), "2".into());
        bundle.vars.insert("A".into(), "1".into());
        let text = serialize(&bundle);
        assert_eq!(text, "A=1\nB=2\n");
        assert_eq!(parse(&text).unwrap(), bundle);
    }
}
```

В `crates/infrastructure/src/lib.rs` добавить `pub mod dotenv;` (по алфавиту, после `docker`).

- [ ] **Step 2: Запустить — убедиться, что падает**

Run: `rtk cargo test -p pi-infrastructure dotenv`
Expected: FAIL (panics on `todo!()`).

- [ ] **Step 3: Реализация**

```rust
pub fn parse(text: &str) -> Result<EnvBundle, String> {
    let mut bundle = EnvBundle::default();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected KEY=VALUE", i + 1))?;
        let key = key.trim();
        if !is_valid_key(key) {
            return Err(format!("line {}: invalid key '{key}'", i + 1));
        }
        bundle.vars.insert(key.to_string(), unquote(value.trim()).to_string());
    }
    Ok(bundle)
}

pub fn is_valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some('A'..='Z' | 'a'..='z' | '_'))
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return &value[1..value.len() - 1];
        }
    }
    value
}

pub fn serialize(bundle: &EnvBundle) -> String {
    let mut out = String::new();
    for (key, value) in &bundle.vars {
        out.push_str(key);
        out.push('=');
        out.push_str(value);
        out.push('\n');
    }
    out
}
```

- [ ] **Step 4: Прогнать тесты**

Run: `rtk cargo test -p pi-infrastructure dotenv`
Expected: PASS (3 теста).

- [ ] **Step 5: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(infra): dotenv parse/serialize module"
```

---

### Task 4: `EncryptedFileStore` (age)

**Files:**
- Modify: `Cargo.toml` (workspace deps: `age = "0.11"`)
- Modify: `crates/infrastructure/Cargo.toml` (deps: `age`)
- Create: `crates/infrastructure/src/secrets.rs`
- Modify: `crates/infrastructure/src/lib.rs` (`pub mod secrets;`)

- [ ] **Step 1: Зависимости**

В `[workspace.dependencies]` корневого `Cargo.toml` (рядом с `uuid`):

```toml
age = "0.11"
```

В `crates/infrastructure/Cargo.toml` `[dependencies]` добавить `age = { workspace = true }`.

- [ ] **Step 2: Создать модуль с падающими тестами**

`crates/infrastructure/src/secrets.rs` (в `lib.rs` — `pub mod secrets;`):

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;

use age::secrecy::ExposeSecret;
use async_trait::async_trait;
use pi_domain::contracts::SecretStore;
use pi_domain::entities::EnvBundle;
use pi_domain::error::DomainError;

use crate::dotenv;

fn secrets_err(msg: impl std::fmt::Display) -> DomainError {
    DomainError::Secrets(msg.to_string())
}

/// age-encrypted bundles at <data_dir>/secrets/<project>.env.age; the agent
/// key is generated on first start at <data_dir>/secret.key, 0600 (§10, §17).
pub struct EncryptedFileStore {
    dir: PathBuf,
    identity: age::x25519::Identity,
}

impl EncryptedFileStore {
    pub fn open(data_dir: &Path) -> Result<Arc<EncryptedFileStore>, DomainError> {
        todo!()
    }

    fn bundle_path(&self, project: &str) -> PathBuf {
        self.dir.join(format!("{project}.env.age"))
    }
}

#[async_trait]
impl SecretStore for EncryptedFileStore {
    async fn save(&self, project: &str, bundle: &EnvBundle) -> Result<(), DomainError> {
        todo!()
    }

    async fn load(&self, project: &str) -> Result<EnvBundle, DomainError> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle() -> EnvBundle {
        let mut b = EnvBundle::default();
        b.vars.insert("DB_PASSWORD".into(), "super-secret-value".into());
        b.vars.insert("PORT".into(), "3000".into());
        b
    }

    #[tokio::test]
    async fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::open(dir.path()).unwrap();
        store.save("rateme", &bundle()).await.unwrap();
        assert_eq!(store.load("rateme").await.unwrap(), bundle());
    }

    #[tokio::test]
    async fn load_missing_project_returns_empty_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::open(dir.path()).unwrap();
        assert!(store.load("nope").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn reopened_store_reuses_key_and_decrypts_old_bundles() {
        let dir = tempfile::tempdir().unwrap();
        EncryptedFileStore::open(dir.path())
            .unwrap()
            .save("rateme", &bundle())
            .await
            .unwrap();
        let reopened = EncryptedFileStore::open(dir.path()).unwrap();
        assert_eq!(reopened.load("rateme").await.unwrap(), bundle());
    }

    #[tokio::test]
    async fn bundle_on_disk_is_not_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::open(dir.path()).unwrap();
        store.save("rateme", &bundle()).await.unwrap();
        let raw = std::fs::read(dir.path().join("secrets").join("rateme.env.age")).unwrap();
        let needle = b"super-secret-value";
        assert!(!raw.windows(needle.len()).any(|w| w == needle));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn key_and_bundle_files_are_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::open(dir.path()).unwrap();
        store.save("rateme", &bundle()).await.unwrap();
        for file in ["secret.key", "secrets/rateme.env.age"] {
            let mode = std::fs::metadata(dir.path().join(file)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{file}");
        }
    }
}
```

- [ ] **Step 3: Запустить — убедиться, что падает**

Run: `rtk cargo test -p pi-infrastructure secrets`
Expected: FAIL (panics on `todo!()`).

- [ ] **Step 4: Реализация**

```rust
impl EncryptedFileStore {
    pub fn open(data_dir: &Path) -> Result<Arc<EncryptedFileStore>, DomainError> {
        let key_path = data_dir.join("secret.key");
        let identity = if key_path.exists() {
            std::fs::read_to_string(&key_path)
                .map_err(secrets_err)?
                .trim()
                .parse::<age::x25519::Identity>()
                .map_err(secrets_err)?
        } else {
            std::fs::create_dir_all(data_dir).map_err(secrets_err)?;
            let identity = age::x25519::Identity::generate();
            write_private(&key_path, identity.to_string().expose_secret().as_bytes())?;
            identity
        };
        let dir = data_dir.join("secrets");
        std::fs::create_dir_all(&dir).map_err(secrets_err)?;
        Ok(Arc::new(EncryptedFileStore { dir, identity }))
    }
    // bundle_path как в Step 2
}

fn write_private(path: &Path, contents: &[u8]) -> Result<(), DomainError> {
    std::fs::write(path, contents).map_err(secrets_err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(secrets_err)?;
    }
    Ok(())
}

#[async_trait]
impl SecretStore for EncryptedFileStore {
    async fn save(&self, project: &str, bundle: &EnvBundle) -> Result<(), DomainError> {
        let plaintext = dotenv::serialize(bundle);
        let ciphertext =
            age::encrypt(&self.identity.to_public(), plaintext.as_bytes()).map_err(secrets_err)?;
        let path = self.bundle_path(project);
        tokio::task::spawn_blocking(move || write_private(&path, &ciphertext))
            .await
            .map_err(|e| secrets_err(format!("join error: {e}")))?
    }

    async fn load(&self, project: &str) -> Result<EnvBundle, DomainError> {
        let ciphertext = match tokio::fs::read(self.bundle_path(project)).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(EnvBundle::default())
            }
            Err(e) => return Err(secrets_err(e)),
        };
        let plaintext = age::decrypt(&self.identity, &ciphertext).map_err(secrets_err)?;
        let text = String::from_utf8(plaintext).map_err(secrets_err)?;
        dotenv::parse(&text).map_err(secrets_err)
    }
}
```

- [ ] **Step 5: Прогнать тесты**

Run: `rtk cargo test -p pi-infrastructure secrets`
Expected: PASS (4 теста на Windows; 5-й — unix-only).

- [ ] **Step 6: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(infra): age-encrypted secret store (EncryptedFileStore)"
```

---

### Task 5: `FsEnvFileWriter` — `.env` в workdir

**Files:**
- Create: `crates/infrastructure/src/envfile.rs`
- Modify: `crates/infrastructure/src/lib.rs` (`pub mod envfile;`)

- [ ] **Step 1: Модуль с падающими тестами**

`crates/infrastructure/src/envfile.rs` (в `lib.rs` — `pub mod envfile;`):

```rust
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::EnvFileWriter;
use pi_domain::entities::EnvBundle;
use pi_domain::error::DomainError;

use crate::dotenv;

/// Writes the decrypted bundle as `<workdir>/.env`, 0600 (§10). The file
/// stays in place: compose re-reads it on every `up`/`restart`.
pub struct FsEnvFileWriter;

impl FsEnvFileWriter {
    pub fn new() -> Arc<FsEnvFileWriter> {
        Arc::new(FsEnvFileWriter)
    }
}

#[async_trait]
impl EnvFileWriter for FsEnvFileWriter {
    async fn write(&self, workdir: &Path, bundle: &EnvBundle) -> Result<(), DomainError> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle() -> EnvBundle {
        let mut b = EnvBundle::default();
        b.vars.insert("A".into(), "1".into());
        b
    }

    #[tokio::test]
    async fn writes_env_file_into_existing_workdir() {
        let dir = tempfile::tempdir().unwrap();
        FsEnvFileWriter::new().write(dir.path(), &bundle()).await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join(".env")).unwrap(), "A=1\n");
    }

    #[tokio::test]
    async fn missing_workdir_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = FsEnvFileWriter::new()
            .write(&dir.path().join("never-deployed"), &bundle())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::NotFound(_)), "got: {err}");
    }
}
```

- [ ] **Step 2: Запустить — убедиться, что падает**

Run: `rtk cargo test -p pi-infrastructure envfile`
Expected: FAIL.

- [ ] **Step 3: Реализация**

```rust
    async fn write(&self, workdir: &Path, bundle: &EnvBundle) -> Result<(), DomainError> {
        if !workdir.is_dir() {
            return Err(DomainError::NotFound(format!(
                "workdir {} does not exist; deploy the project first",
                workdir.display()
            )));
        }
        let path = workdir.join(".env");
        tokio::fs::write(&path, dotenv::serialize(bundle))
            .await
            .map_err(|e| DomainError::Storage(format!("write .env: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .await
                .map_err(|e| DomainError::Storage(format!("chmod .env: {e}")))?;
        }
        Ok(())
    }
```

- [ ] **Step 4: Прогнать тесты**

Run: `rtk cargo test -p pi-infrastructure envfile`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(infra): .env writer for project workdirs"
```

---

### Task 6: Docker health в `ps` + `HybridHealthGate`

**Files:**
- Modify: `crates/infrastructure/src/docker.rs` (парсинг `Health`)
- Create: `crates/infrastructure/src/health.rs`
- Modify: `crates/infrastructure/src/lib.rs` (`pub mod health;`)
- Modify: `crates/infrastructure/Cargo.toml` (deps: `reqwest`; dev-deps: `pi-domain` mocks, `mockall`)

- [ ] **Step 1: Падающий тест на парсинг Health**

`crates/infrastructure/src/docker.rs`, в `mod tests`:

```rust
    #[test]
    fn parse_ps_json_reads_health_field() {
        let out = concat!(
            r#"{"Service":"web","State":"running","Health":"healthy"}"#, "\n",
            r#"{"Service":"db","State":"running","Health":""}"#, "\n",
            r#"{"Service":"worker","State":"running"}"#, "\n",
        );
        let states = parse_ps_json(out);
        assert_eq!(states[0].health.as_deref(), Some("healthy"));
        assert_eq!(states[1].health, None, "empty Health means no healthcheck");
        assert_eq!(states[2].health, None);
    }
```

Run: `rtk cargo test -p pi-infrastructure reads_health` → FAIL.

- [ ] **Step 2: Реализовать парсинг**

В `docker.rs` заменить `service_state`:

```rust
fn service_state(v: &serde_json::Value) -> Option<ServiceState> {
    let health = v
        .get("Health")
        .and_then(|h| h.as_str())
        .filter(|h| !h.is_empty())
        .map(str::to_string);
    Some(ServiceState {
        service: v.get("Service")?.as_str()?.to_string(),
        state: v.get("State")?.as_str()?.to_string(),
        health,
    })
}
```

Run: `rtk cargo test -p pi-infrastructure docker` → PASS. Commit:

```bash
rtk git add -A && rtk git commit -m "feat(infra): expose docker healthcheck state via compose ps"
```

- [ ] **Step 3: Зависимости для health-гейта**

`crates/infrastructure/Cargo.toml`:

```toml
[dependencies]
# ... существующие ...
reqwest = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
pi-domain = { workspace = true, features = ["mocks"] }
mockall = { workspace = true }
```

- [ ] **Step 4: Модуль `health.rs` с падающими тестами**

`crates/infrastructure/src/health.rs` (в `lib.rs` — `pub mod health;`):

```rust
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pi_domain::contracts::{ContainerRuntime, HealthGate, LogSink};
use pi_domain::entities::ProjectConfig;
use pi_domain::error::DomainError;

/// "2xx" | "3xx" | exact code; None => 2xx/3xx ([healthcheck].expect, §12).
pub(crate) fn status_matches(expect: Option<&str>, status: u16) -> bool {
    match expect {
        None => (200..400).contains(&status),
        Some("2xx") => (200..300).contains(&status),
        Some("3xx") => (300..400).contains(&status),
        Some(code) => code.parse::<u16>().map(|c| c == status).unwrap_or(false),
    }
}

enum Probe {
    Pass,
    Wait(String),
}

/// Hybrid deploy gate (§8): docker healthcheck when declared on the public
/// service, else HTTP GET on the host port when [healthcheck].path is set,
/// else plain TCP connect. Polls until pass or timeout.
pub struct HybridHealthGate {
    runtime: Arc<dyn ContainerRuntime>,
    http: reqwest::Client,
    interval: Duration,
}

impl HybridHealthGate {
    pub fn new(runtime: Arc<dyn ContainerRuntime>) -> Arc<HybridHealthGate> {
        HybridHealthGate::with_interval(runtime, Duration::from_secs(2))
    }

    /// Tests use a short interval.
    pub fn with_interval(
        runtime: Arc<dyn ContainerRuntime>,
        interval: Duration,
    ) -> Arc<HybridHealthGate> {
        Arc::new(HybridHealthGate { runtime, http: reqwest::Client::new(), interval })
    }

    async fn probe(&self, config: &ProjectConfig, host_port: u16) -> Result<Probe, DomainError> {
        todo!()
    }
}

#[async_trait]
impl HealthGate for HybridHealthGate {
    async fn check(
        &self,
        config: &ProjectConfig,
        host_port: u16,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_domain::contracts::MockContainerRuntime;
    use pi_domain::entities::{DeploymentStatus, HealthcheckConfig, ServiceState};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct NullSink;
    impl LogSink for NullSink {
        fn line(&self, _line: &str) {}
        fn finished(&self, _status: DeploymentStatus) {}
    }

    fn sink() -> Arc<dyn LogSink> {
        Arc::new(NullSink)
    }

    fn config(healthcheck: HealthcheckConfig) -> ProjectConfig {
        ProjectConfig {
            name: "rateme".into(),
            repo: "https://github.com/x/y.git".into(),
            branch: "main".into(),
            compose_path: "docker-compose.yml".into(),
            service: "web".into(),
            container_port: 3000,
            hostname: None,
            healthcheck,
        }
    }

    fn web(health: Option<&str>) -> Vec<ServiceState> {
        vec![ServiceState {
            service: "web".into(),
            state: "running".into(),
            health: health.map(str::to_string),
        }]
    }

    fn gate(runtime: MockContainerRuntime) -> Arc<HybridHealthGate> {
        HybridHealthGate::with_interval(Arc::new(runtime), Duration::from_millis(10))
    }

    #[test]
    fn status_matches_classes_and_exact_codes() {
        assert!(status_matches(None, 200) && status_matches(None, 302));
        assert!(!status_matches(None, 404));
        assert!(status_matches(Some("2xx"), 204) && !status_matches(Some("2xx"), 301));
        assert!(status_matches(Some("3xx"), 301) && !status_matches(Some("3xx"), 200));
        assert!(status_matches(Some("418"), 418) && !status_matches(Some("418"), 200));
        assert!(!status_matches(Some("bogus"), 200));
    }

    #[tokio::test]
    async fn docker_healthy_passes() {
        let mut runtime = MockContainerRuntime::new();
        runtime.expect_ps().returning(|_| Ok(web(Some("healthy"))));
        gate(runtime).check(&config(HealthcheckConfig::default()), 1, sink()).await.unwrap();
    }

    #[tokio::test]
    async fn docker_unhealthy_fails_without_waiting_for_timeout() {
        let mut runtime = MockContainerRuntime::new();
        runtime.expect_ps().times(1).returning(|_| Ok(web(Some("unhealthy"))));
        let err = gate(runtime)
            .check(&config(HealthcheckConfig::default()), 1, sink())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::HealthCheck(_)), "got: {err}");
    }

    #[tokio::test]
    async fn docker_starting_then_healthy_passes() {
        let calls = AtomicUsize::new(0);
        let mut runtime = MockContainerRuntime::new();
        runtime.expect_ps().returning(move |_| {
            if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                Ok(web(Some("starting")))
            } else {
                Ok(web(Some("healthy")))
            }
        });
        gate(runtime).check(&config(HealthcheckConfig::default()), 1, sink()).await.unwrap();
    }

    #[tokio::test]
    async fn tcp_probe_passes_when_port_listens() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let _ = listener.accept().await;
            }
        });
        let mut runtime = MockContainerRuntime::new();
        runtime.expect_ps().returning(|_| Ok(web(None)));
        gate(runtime).check(&config(HealthcheckConfig::default()), port, sink()).await.unwrap();
    }

    #[tokio::test]
    async fn http_probe_checks_expected_status() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                if let Ok((mut conn, _)) = listener.accept().await {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 1024];
                    let _ = conn.read(&mut buf).await;
                    let _ = conn
                        .write_all(b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n")
                        .await;
                }
            }
        });
        let mut runtime = MockContainerRuntime::new();
        runtime.expect_ps().returning(|_| Ok(web(None)));
        let hc = HealthcheckConfig {
            path: Some("/health".into()),
            expect: Some("204".into()),
            timeout_secs: 5,
        };
        gate(runtime).check(&config(hc), port, sink()).await.unwrap();
    }

    #[tokio::test]
    async fn times_out_with_last_probe_reason() {
        // port from a dropped listener: nothing listens there
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let mut runtime = MockContainerRuntime::new();
        runtime.expect_ps().returning(|_| Ok(web(None)));
        let hc = HealthcheckConfig { timeout_secs: 0, ..HealthcheckConfig::default() };
        let err = gate(runtime).check(&config(hc), port, sink()).await.unwrap_err();
        assert!(matches!(&err, DomainError::HealthCheck(m) if m.contains("timed out")), "got: {err}");
    }

    #[tokio::test]
    async fn ps_failure_propagates_as_error() {
        let mut runtime = MockContainerRuntime::new();
        runtime
            .expect_ps()
            .returning(|_| Err(DomainError::Runtime("docker daemon down".into())));
        let err = gate(runtime)
            .check(&config(HealthcheckConfig::default()), 1, sink())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Runtime(_)), "got: {err}");
    }
}
```

- [ ] **Step 5: Запустить — убедиться, что падает**

Run: `rtk cargo test -p pi-infrastructure health`
Expected: FAIL (`todo!()`); `status_matches_...` уже PASS.

- [ ] **Step 6: Реализация `probe` и `check`**

```rust
    async fn probe(&self, config: &ProjectConfig, host_port: u16) -> Result<Probe, DomainError> {
        // 1. docker healthcheck on the public service, when declared
        let services = self.runtime.ps(&config.name).await?;
        let health = services
            .iter()
            .find(|s| s.service == config.service)
            .and_then(|s| s.health.clone());
        match health.as_deref() {
            Some("healthy") => return Ok(Probe::Pass),
            Some("unhealthy") => {
                return Err(DomainError::HealthCheck(
                    "docker reports the public service unhealthy".into(),
                ))
            }
            Some(other) => return Ok(Probe::Wait(format!("docker health: {other}"))),
            None => {}
        }
        // 2. HTTP probe when a path is configured, else 3. TCP connect
        match &config.healthcheck.path {
            Some(path) => {
                let url = format!("http://127.0.0.1:{host_port}{path}");
                match self.http.get(&url).send().await {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        if status_matches(config.healthcheck.expect.as_deref(), status) {
                            Ok(Probe::Pass)
                        } else {
                            Ok(Probe::Wait(format!("GET {path} -> {status}")))
                        }
                    }
                    Err(e) => Ok(Probe::Wait(format!("GET {path}: {e}"))),
                }
            }
            None => match tokio::net::TcpStream::connect(("127.0.0.1", host_port)).await {
                Ok(_) => Ok(Probe::Pass),
                Err(e) => Ok(Probe::Wait(format!("tcp connect 127.0.0.1:{host_port}: {e}"))),
            },
        }
    }
```

```rust
    async fn check(
        &self,
        config: &ProjectConfig,
        host_port: u16,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError> {
        let budget = Duration::from_secs(config.healthcheck.timeout_secs);
        let deadline = tokio::time::Instant::now() + budget;
        log.line(&format!("healthcheck: waiting up to {}s ...", budget.as_secs()));
        let mut last = String::from("no probe attempted");
        loop {
            match self.probe(config, host_port).await? {
                Probe::Pass => {
                    log.line("healthcheck: passed");
                    return Ok(());
                }
                Probe::Wait(reason) => last = reason,
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(DomainError::HealthCheck(format!(
                    "timed out after {}s (last probe: {last})",
                    budget.as_secs()
                )));
            }
            tokio::time::sleep(self.interval).await;
        }
    }
```

- [ ] **Step 7: Прогнать тесты**

Run: `rtk cargo test -p pi-infrastructure health`
Expected: PASS (8 тестов).

- [ ] **Step 8: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(infra): hybrid health gate (docker -> HTTP -> TCP)"
```

---

### Task 7: `CloudflaredIngress` + `DisabledIngress`

**Files:**
- Modify: `Cargo.toml` (workspace deps: `serde_yaml = "0.9"`)
- Modify: `crates/infrastructure/Cargo.toml` (deps: `serde_yaml`)
- Create: `crates/infrastructure/src/cloudflared.rs`
- Modify: `crates/infrastructure/src/lib.rs` (`pub mod cloudflared;`)

- [ ] **Step 1: Зависимость**

Workspace `Cargo.toml`: `serde_yaml = "0.9"`; в `crates/infrastructure/Cargo.toml` — `serde_yaml = { workspace = true }`.

- [ ] **Step 2: Модуль с падающими тестами**

`crates/infrastructure/src/cloudflared.rs` (в `lib.rs` — `pub mod cloudflared;`):

```rust
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::{Ingress, LogSink};
use pi_domain::error::DomainError;
use tokio::process::Command;

use crate::process::run_capture;

fn ingress_err(msg: impl std::fmt::Display) -> DomainError {
    DomainError::Ingress(msg.to_string())
}

/// Upserts `hostname -> service` into the cloudflared config document.
/// Returns true when the document changed (diff-restart, §11). Keeps the
/// catch-all rule (no `hostname`) last, appending one if missing.
pub(crate) fn upsert_ingress_rule(
    doc: &mut serde_yaml::Value,
    hostname: &str,
    service: &str,
) -> Result<bool, String> {
    todo!()
}

/// `cloudflared tunnel route dns` fails when the record exists — tolerated.
pub(crate) fn is_already_exists(stderr: &str) -> bool {
    todo!()
}

/// Locally-managed cloudflared (§11): edits config.yml, creates the DNS
/// route, restarts the unit without sudo — and only when the config changed.
pub struct CloudflaredIngress {
    config_path: PathBuf,
    tunnel: String,
    restart: Vec<String>,
}

impl CloudflaredIngress {
    pub fn new(
        config_path: PathBuf,
        tunnel: String,
        restart: Vec<String>,
    ) -> Arc<CloudflaredIngress> {
        Arc::new(CloudflaredIngress { config_path, tunnel, restart })
    }
}

#[async_trait]
impl Ingress for CloudflaredIngress {
    async fn upsert(
        &self,
        hostname: &str,
        host_port: u16,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError> {
        todo!()
    }
}

/// No [cloudflared] in agent.toml: deploys succeed, routing is manual.
pub struct DisabledIngress;

impl DisabledIngress {
    pub fn new() -> Arc<DisabledIngress> {
        Arc::new(DisabledIngress)
    }
}

#[async_trait]
impl Ingress for DisabledIngress {
    async fn upsert(
        &self,
        hostname: &str,
        host_port: u16,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError> {
        log.line(&format!(
            "ingress: [cloudflared] is not configured in agent.toml; \
             route {hostname} -> http://127.0.0.1:{host_port} manually"
        ));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_domain::entities::DeploymentStatus;
    use std::sync::Mutex;

    struct VecSink(Mutex<Vec<String>>);
    impl VecSink {
        fn new() -> Arc<VecSink> {
            Arc::new(VecSink(Mutex::new(vec![])))
        }
    }
    impl LogSink for VecSink {
        fn line(&self, line: &str) {
            self.0.lock().unwrap().push(line.to_string());
        }
        fn finished(&self, _status: DeploymentStatus) {}
    }

    fn doc(yaml: &str) -> serde_yaml::Value {
        serde_yaml::from_str(yaml).unwrap()
    }

    const BASE: &str = "tunnel: home\ncredentials-file: /var/lib/pi/cloudflared/home.json\ningress:\n  - hostname: old.example.com\n    service: http://127.0.0.1:8001\n  - service: http_status:404\n";

    #[test]
    fn adds_new_rule_before_catch_all() {
        let mut d = doc(BASE);
        let changed = upsert_ingress_rule(&mut d, "new.example.com", "http://127.0.0.1:8002").unwrap();
        assert!(changed);
        let rules = d.get("ingress").unwrap().as_sequence().unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[1].get("hostname").unwrap().as_str(), Some("new.example.com"));
        assert!(rules[2].get("hostname").is_none(), "catch-all stays last");
    }

    #[test]
    fn same_rule_is_a_noop() {
        let mut d = doc(BASE);
        let changed = upsert_ingress_rule(&mut d, "old.example.com", "http://127.0.0.1:8001").unwrap();
        assert!(!changed);
        assert_eq!(d, doc(BASE), "document untouched");
    }

    #[test]
    fn changed_port_replaces_rule_in_place() {
        let mut d = doc(BASE);
        let changed = upsert_ingress_rule(&mut d, "old.example.com", "http://127.0.0.1:9000").unwrap();
        assert!(changed);
        let rules = d.get("ingress").unwrap().as_sequence().unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].get("service").unwrap().as_str(), Some("http://127.0.0.1:9000"));
    }

    #[test]
    fn missing_ingress_list_and_catch_all_are_created() {
        let mut d = doc("tunnel: home\n");
        let changed = upsert_ingress_rule(&mut d, "a.example.com", "http://127.0.0.1:8000").unwrap();
        assert!(changed);
        let rules = d.get("ingress").unwrap().as_sequence().unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[1].get("service").unwrap().as_str(), Some("http_status:404"));
    }

    #[test]
    fn non_mapping_document_is_an_error() {
        let mut d = doc("- just\n- a list\n");
        assert!(upsert_ingress_rule(&mut d, "a.example.com", "x").is_err());
    }

    #[test]
    fn already_exists_detection() {
        assert!(is_already_exists("... record with that host already exists ..."));
        assert!(is_already_exists("Already configured CNAME for this hostname"));
        assert!(!is_already_exists("connection refused"));
    }

    #[tokio::test]
    async fn missing_config_file_gives_actionable_error() {
        let dir = tempfile::tempdir().unwrap();
        let ingress = CloudflaredIngress::new(
            dir.path().join("config.yml"),
            "home".into(),
            vec!["whatever".into()],
        );
        let err = ingress.upsert("a.example.com", 8000, VecSink::new()).await.unwrap_err();
        assert!(matches!(&err, DomainError::Ingress(m) if m.contains("config.yml")), "got: {err}");
    }

    #[tokio::test]
    async fn no_diff_skips_dns_and_restart_entirely() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(&path, BASE).unwrap();
        // commands would fail if executed: binaries do not exist
        let ingress = CloudflaredIngress::new(
            path.clone(),
            "home".into(),
            vec!["pi-test-no-such-binary".into()],
        );
        ingress.upsert("old.example.com", 8001, VecSink::new()).await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), BASE, "file untouched");
    }

    #[tokio::test]
    async fn diff_writes_config_before_running_commands() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(&path, BASE).unwrap();
        let ingress = CloudflaredIngress::new(
            path.clone(),
            "home".into(),
            vec!["pi-test-no-such-binary".into()],
        );
        // `cloudflared` is not on PATH in tests -> route dns fails -> Err,
        // but the config must already be updated (§11 order: config, dns, restart).
        let err = ingress.upsert("new.example.com", 8002, VecSink::new()).await.unwrap_err();
        assert!(matches!(err, DomainError::Ingress(_)));
        assert!(std::fs::read_to_string(&path).unwrap().contains("new.example.com"));
    }
}
```

- [ ] **Step 3: Запустить — убедиться, что падает**

Run: `rtk cargo test -p pi-infrastructure cloudflared`
Expected: FAIL (`todo!()`).

- [ ] **Step 4: Реализация**

```rust
pub(crate) fn upsert_ingress_rule(
    doc: &mut serde_yaml::Value,
    hostname: &str,
    service: &str,
) -> Result<bool, String> {
    let map = doc
        .as_mapping_mut()
        .ok_or("config.yml: top level must be a mapping")?;
    let key = serde_yaml::Value::from("ingress");
    if !map.contains_key(&key) {
        map.insert(key.clone(), serde_yaml::Value::Sequence(vec![]));
    }
    let rules = map
        .get_mut(&key)
        .and_then(|v| v.as_sequence_mut())
        .ok_or("config.yml: `ingress` must be a list")?;

    let mut desired = serde_yaml::Mapping::new();
    desired.insert("hostname".into(), hostname.into());
    desired.insert("service".into(), service.into());
    let desired = serde_yaml::Value::Mapping(desired);

    if let Some(existing) = rules
        .iter_mut()
        .find(|r| r.get("hostname").and_then(|h| h.as_str()) == Some(hostname))
    {
        if existing.get("service").and_then(|s| s.as_str()) == Some(service) {
            return Ok(false);
        }
        *existing = desired;
        return Ok(true);
    }

    match rules.iter().position(|r| r.get("hostname").is_none()) {
        Some(catch_all) => rules.insert(catch_all, desired),
        None => {
            rules.push(desired);
            let mut catch_all = serde_yaml::Mapping::new();
            catch_all.insert("service".into(), "http_status:404".into());
            rules.push(serde_yaml::Value::Mapping(catch_all));
        }
    }
    Ok(true)
}

pub(crate) fn is_already_exists(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("already exists") || s.contains("already configured")
}
```

```rust
#[async_trait]
impl Ingress for CloudflaredIngress {
    async fn upsert(
        &self,
        hostname: &str,
        host_port: u16,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError> {
        let text = tokio::fs::read_to_string(&self.config_path).await.map_err(|e| {
            ingress_err(format!(
                "cannot read {}: {e}; bootstrap the tunnel first (docs/install-agent-v0.1.md)",
                self.config_path.display()
            ))
        })?;
        let mut doc: serde_yaml::Value = serde_yaml::from_str(&text).map_err(ingress_err)?;
        let service = format!("http://127.0.0.1:{host_port}");
        let changed = upsert_ingress_rule(&mut doc, hostname, &service).map_err(ingress_err)?;
        if !changed {
            log.line(&format!(
                "ingress: {hostname} -> {service} already routed; cloudflared untouched"
            ));
            return Ok(());
        }
        let updated = serde_yaml::to_string(&doc).map_err(ingress_err)?;
        tokio::fs::write(&self.config_path, updated).await.map_err(ingress_err)?;
        log.line(&format!("ingress: routing {hostname} -> {service}"));

        let mut dns = Command::new("cloudflared");
        dns.args(["tunnel", "route", "dns", &self.tunnel, hostname]);
        match run_capture(dns).await {
            Ok(_) => log.line(&format!("ingress: DNS record created for {hostname}")),
            Err(err) if is_already_exists(&err) => {
                log.line(&format!("ingress: DNS for {hostname} already exists; leaving as is"));
            }
            Err(err) => return Err(ingress_err(format!("route dns: {err}"))),
        }

        let (program, args) = self
            .restart
            .split_first()
            .ok_or_else(|| ingress_err("empty cloudflared restart command"))?;
        let mut restart = Command::new(program);
        restart.args(args);
        run_capture(restart)
            .await
            .map_err(|e| ingress_err(format!("restart cloudflared: {e}")))?;
        log.line("ingress: cloudflared restarted");
        Ok(())
    }
}
```

- [ ] **Step 5: Прогнать тесты**

Run: `rtk cargo test -p pi-infrastructure cloudflared`
Expected: PASS (9 тестов).

- [ ] **Step 6: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(infra): cloudflared ingress with diff-restart and dns route"
```

---

### Task 8: `MaskingSink` (application)

**Files:**
- Create: `crates/application/src/mask.rs`
- Modify: `crates/application/src/lib.rs` (`pub mod mask;`)

- [ ] **Step 1: Модуль с падающими тестами**

`crates/application/src/mask.rs` (в `lib.rs` — `pub mod mask;`):

```rust
use std::sync::{Arc, Mutex};

use pi_domain::contracts::LogSink;
use pi_domain::entities::{DeploymentStatus, EnvBundle};

/// Secret values shorter than this are not masked — filters out false
/// positives like `true`/`3000` (§8.1, §22).
pub const MASK_MIN_LEN: usize = 6;

/// LogSink wrapper replacing armed secret values with ***KEY*** (§8.1).
/// Created empty and armed once the bundle is decrypted mid-deploy: values
/// cannot leak before the process knows them.
pub struct MaskingSink {
    inner: Arc<dyn LogSink>,
    /// (mask, value), longest values first so nested secrets mask fully.
    secrets: Mutex<Vec<(String, String)>>,
}

impl MaskingSink {
    pub fn new(inner: Arc<dyn LogSink>) -> Arc<MaskingSink> {
        Arc::new(MaskingSink { inner, secrets: Mutex::new(Vec::new()) })
    }

    pub fn arm(&self, bundle: &EnvBundle) {
        todo!()
    }

    fn masked(&self, line: &str) -> String {
        todo!()
    }
}

impl LogSink for MaskingSink {
    fn line(&self, line: &str) {
        self.inner.line(&self.masked(line));
    }

    fn finished(&self, status: DeploymentStatus) {
        self.inner.finished(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CollectSink;

    fn bundle(pairs: &[(&str, &str)]) -> EnvBundle {
        let mut b = EnvBundle::default();
        for (k, v) in pairs {
            b.vars.insert(k.to_string(), v.to_string());
        }
        b
    }

    #[test]
    fn masks_armed_values_and_keeps_short_ones() {
        let inner = CollectSink::new();
        let mask = MaskingSink::new(inner.clone());
        mask.arm(&bundle(&[("DB_PASSWORD", "hunter2-long"), ("PORT", "3000")]));

        mask.line("connecting with hunter2-long to db on 3000");

        assert_eq!(
            *inner.lines.lock().unwrap(),
            vec!["connecting with ***DB_PASSWORD*** to db on 3000".to_string()]
        );
    }

    #[test]
    fn masks_every_occurrence_and_longest_value_first() {
        let inner = CollectSink::new();
        let mask = MaskingSink::new(inner.clone());
        mask.arm(&bundle(&[("TOKEN", "abc123"), ("URL", "https://u:abc123@host")]));

        mask.line("https://u:abc123@host then abc123 again abc123");

        assert_eq!(
            *inner.lines.lock().unwrap(),
            vec!["***URL*** then ***TOKEN*** again ***TOKEN***".to_string()]
        );
    }

    #[test]
    fn passthrough_before_arm_and_finished_forwarded() {
        let inner = CollectSink::new();
        let mask = MaskingSink::new(inner.clone());
        mask.line("raw hunter2-long");
        mask.finished(DeploymentStatus::Success);
        assert_eq!(*inner.lines.lock().unwrap(), vec!["raw hunter2-long".to_string()]);
        assert_eq!(*inner.finished.lock().unwrap(), vec![DeploymentStatus::Success]);
    }
}
```

Примечание: `test_support` сейчас под `#[cfg(test)]` в `lib.rs` — тесты внутри крейта его видят, менять ничего не нужно.

- [ ] **Step 2: Запустить — убедиться, что падает**

Run: `rtk cargo test -p pi-application mask`
Expected: FAIL (`todo!()`).

- [ ] **Step 3: Реализация**

```rust
    pub fn arm(&self, bundle: &EnvBundle) {
        let mut secrets: Vec<(String, String)> = bundle
            .vars
            .iter()
            .filter(|(_, value)| value.len() >= MASK_MIN_LEN)
            .map(|(key, value)| (format!("***{key}***"), value.clone()))
            .collect();
        secrets.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
        if let Ok(mut held) = self.secrets.lock() {
            *held = secrets;
        }
    }

    fn masked(&self, line: &str) -> String {
        let held = match self.secrets.lock() {
            Ok(held) => held,
            Err(_) => return line.to_string(),
        };
        let mut out = line.to_string();
        for (mask, value) in held.iter() {
            if out.contains(value.as_str()) {
                out = out.replace(value.as_str(), mask);
            }
        }
        out
    }
```

- [ ] **Step 4: Прогнать тесты**

Run: `rtk cargo test -p pi-application mask`
Expected: PASS (3 теста).

- [ ] **Step 5: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(app): secret-masking log sink"
```

---

### Task 9: Деплой-поток: env-inject + masking + health-гейт + ingress; wiring агента

**Files:**
- Modify: `crates/application/src/deploy.rs`
- Modify: `crates/bin/src/agent/config.rs` (`[cloudflared]`)
- Modify: `crates/bin/src/agent/state.rs` (wiring)
- Modify: `crates/bin/src/agent/http.rs` (тестовый `state_with`)
- Modify: `dev/agent.toml`

- [ ] **Step 1: Обновить `DeployProject` — поля и конструктор**

В `crates/application/src/deploy.rs`:

```rust
use pi_domain::contracts::{
    Clock, ContainerRuntime, DeploymentHistory, EnvFileWriter, HealthGate, Ingress, LogSink,
    OverrideStore, ProjectRepository, SecretStore, Source,
};
// ...
use crate::mask::MaskingSink;
```

Поля структуры (после `overrides`):

```rust
    secrets: Arc<dyn SecretStore>,
    env_files: Arc<dyn EnvFileWriter>,
    health: Arc<dyn HealthGate>,
    ingress: Arc<dyn Ingress>,
```

Сигнатура `new` (тот же порядок):

```rust
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
```

- [ ] **Step 2: Вставить masking-цепочку в `execute`**

Заменить начало `execute` (создание tail/log):

```rust
        let tail = TailSink::new(Arc::clone(&sink), LOG_TAIL_LINES);
        let masker = MaskingSink::new(tail.clone());
        let log: Arc<dyn LogSink> = masker.clone();
```

(цепочка: стадии пишут в `masker` → маскирует → `tail` хранит уже замаскированный хвост для БД → форвардит в SSE-hub; `FinishGuard` остаётся на исходном `sink`). Вызов стадий:

```rust
        let result = self.run_stages(&config, &git_ref, log.clone(), &masker).await;
```

- [ ] **Step 3: Новые стадии в `run_stages`**

```rust
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
        self.health.check(config, project.host_port, log.clone()).await?;

        // §11: route hostname only when configured
        if let Some(hostname) = &config.hostname {
            self.ingress.upsert(hostname, project.host_port, log.clone()).await?;
        }

        Ok(fetched.commit_sha)
    }
```

- [ ] **Step 4: Обновить тестовую обвязку deploy.rs**

В `mod tests`: расширить `Mocks` и хелперы:

```rust
    use pi_domain::contracts::{
        MockClock, MockContainerRuntime, MockDeploymentHistory, MockEnvFileWriter, MockHealthGate,
        MockIngress, MockOverrideStore, MockProjectRepository, MockSecretStore, MockSource,
    };
    use pi_domain::entities::EnvBundle;

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
```

Существующие тесты дополнить ожиданиями новых стадий:

- `happy_path_runs_all_stages_and_records_success`: добавить (в том же стиле через `stage_order`):

```rust
        let stage_order = Arc::clone(&order);
        m.secrets.expect_load().times(1).returning(move |_| {
            stage_order.lock().unwrap().push("secrets");
            Ok(EnvBundle::default())
        });
        // empty bundle -> .env must NOT be written
        m.env_files.expect_write().times(0);
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
```

и итоговый порядок:

```rust
        assert_eq!(
            *order.lock().unwrap(),
            vec!["started", "upsert", "fetch", "secrets", "override", "build", "up", "health", "ingress", "finished"]
        );
```

- `build_failure_records_failed_and_emits_finished_failed`: добавить `m.secrets.expect_load().returning(|_| Ok(EnvBundle::default()));`, `m.health.expect_check().times(0);`, `m.ingress.expect_upsert().times(0);`.
- `lock_released_after_execute_finishes`: добавить `m.secrets.expect_load().returning(|_| Ok(EnvBundle::default()));`, `m.health.expect_check().returning(|_, _, _| Ok(()));`, `m.ingress.expect_upsert().returning(|_, _, _| Ok(()));`.

- [ ] **Step 5: Новые тесты деплой-потока**

Добавить в `mod tests` (`deploy.rs`):

```rust
    fn secret_bundle() -> EnvBundle {
        let mut b = EnvBundle::default();
        b.vars.insert("DB_PASSWORD".into(), "hunter2-long".into());
        b
    }

    fn ok_pre_stages(m: &mut Mocks) {
        m.projects.expect_upsert().returning(|c| {
            Ok(Project { config: c.clone(), host_port: 8000, created_at: 1 })
        });
        m.source.expect_fetch().returning(|_, _, _| {
            Ok(FetchedSource { workdir: PathBuf::from("/wd"), commit_sha: SHA.into() })
        });
        m.overrides
            .expect_write()
            .returning(|_, _, _, _| Ok(PathBuf::from("/ov.yml")));
        m.history.expect_record_started().returning(|_| Ok(()));
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
        // the build step "leaks" the secret into its output
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
            .execute(permit, "dep-env".into(), sample_config(), DeployRef::Branch("main".into()), sink.clone())
            .await
            .unwrap();

        assert_eq!(result.status, DeploymentStatus::Success);
        assert!(result.log_tail.contains(".env injected (1 keys)"));
        assert!(result.log_tail.contains("***DB_PASSWORD***"), "tail: {}", result.log_tail);
        assert!(!result.log_tail.contains("hunter2-long"), "secret leaked into tail");
        let lines = sink.lines.lock().unwrap();
        assert!(lines.iter().any(|l| l.contains("***DB_PASSWORD***")));
        assert!(!lines.iter().any(|l| l.contains("hunter2-long")), "secret leaked into stream");
    }

    #[tokio::test]
    async fn health_gate_failure_fails_deploy_and_skips_ingress() {
        let mut m = mocks();
        ok_pre_stages(&mut m);
        m.secrets.expect_load().returning(|_| Ok(EnvBundle::default()));
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
            .execute(permit, "dep-hc".into(), sample_config(), DeployRef::Branch("main".into()), sink.clone())
            .await
            .unwrap_err();

        assert!(matches!(err, DomainError::HealthCheck(_)));
        assert_eq!(*sink.finished.lock().unwrap(), vec![DeploymentStatus::Failed]);
    }

    #[tokio::test]
    async fn project_without_hostname_skips_ingress() {
        let mut m = mocks();
        ok_pre_stages(&mut m);
        m.secrets.expect_load().returning(|_| Ok(EnvBundle::default()));
        m.runtime.expect_build().returning(|_, _| Ok(()));
        m.runtime.expect_up().returning(|_, _| Ok(()));
        m.health.expect_check().returning(|_, _, _| Ok(()));
        m.ingress.expect_upsert().times(0);

        let mut config = sample_config();
        config.hostname = None;

        let deploy = build(m);
        let permit = deploy.try_begin("rateme").unwrap();
        let result = deploy
            .execute(permit, "dep-nh".into(), config, DeployRef::Branch("main".into()), CollectSink::new())
            .await
            .unwrap();
        assert_eq!(result.status, DeploymentStatus::Success);
    }
```

(добавить `use std::path::Path;` в тест-импорты; `Project`, `FetchedSource`, `DomainError` уже импортированы).

Run: `rtk cargo test -p pi-application` → PASS (бин пока не компилируется — это Step 6).

- [ ] **Step 6: `[cloudflared]` в agent.toml + wiring `build_state`**

`crates/bin/src/agent/config.rs` — добавить секцию:

```rust
#[derive(Debug, Deserialize)]
pub struct CloudflaredSection {
    /// Path to the locally-managed cloudflared config.yml (§11).
    pub config: PathBuf,
    /// Tunnel name for `cloudflared tunnel route dns`.
    pub tunnel: String,
    /// Command applying the config; no sudo needed under linger (§11).
    #[serde(default = "default_restart")]
    pub restart: Vec<String>,
}

fn default_restart() -> Vec<String> {
    ["systemctl", "--user", "restart", "cloudflared"]
        .map(String::from)
        .to_vec()
}
```

в `AgentConfig` — поле `pub cloudflared: Option<CloudflaredSection>,`. Тесты в `config.rs`:

```rust
    #[test]
    fn cloudflared_section_parses_with_default_restart() {
        let config = AgentConfig::parse(
            "[cloudflared]\nconfig = \"/var/lib/pi/cloudflared/config.yml\"\ntunnel = \"home\"",
        )
        .unwrap();
        let cf = config.cloudflared.unwrap();
        assert_eq!(cf.tunnel, "home");
        assert_eq!(cf.restart, vec!["systemctl", "--user", "restart", "cloudflared"]);
    }

    #[test]
    fn cloudflared_section_is_optional() {
        assert!(AgentConfig::parse("").unwrap().cloudflared.is_none());
    }
```

`crates/bin/src/agent/state.rs` — `build_state` (после создания `overrides`):

```rust
    let secrets = EncryptedFileStore::open(&config.data_dir).map_err(|e| anyhow::anyhow!("{e}"))?;
    let env_files = FsEnvFileWriter::new();
    let health = HybridHealthGate::new(runtime.clone());
    let ingress: Arc<dyn Ingress> = match &config.cloudflared {
        Some(cf) => CloudflaredIngress::new(cf.config.clone(), cf.tunnel.clone(), cf.restart.clone()),
        None => DisabledIngress::new(),
    };

    let deploy = DeployProject::new(
        source,
        runtime.clone(),
        projects.clone(),
        Arc::clone(&history),
        overrides,
        secrets,
        env_files,
        health,
        ingress,
        SystemClock::new(),
    );
```

с импортами `pi_infrastructure::{cloudflared::{CloudflaredIngress, DisabledIngress}, envfile::FsEnvFileWriter, health::HybridHealthGate, secrets::EncryptedFileStore}` и `pi_domain::contracts::Ingress`.

`dev/agent.toml` — добавить в конец:

```toml
# [cloudflared]                 # v0.2: auto-ingress; omit to disable
# config = "/var/lib/pi/cloudflared/config.yml"
# tunnel = "home"
```

- [ ] **Step 7: Починить тесты `http.rs`**

В `state_with` (crates/bin/src/agent/http.rs) — новые зависимости:

```rust
    use pi_infrastructure::cloudflared::DisabledIngress;
    use pi_infrastructure::envfile::FsEnvFileWriter;
    use pi_infrastructure::health::HybridHealthGate;
    use pi_infrastructure::secrets::EncryptedFileStore;
```

```rust
        let secrets = EncryptedFileStore::open(dir).unwrap();
        let deploy = DeployProject::new(
            source,
            Arc::clone(&runtime),
            projects.clone(),
            Arc::clone(&history),
            overrides,
            secrets,
            FsEnvFileWriter::new(),
            HybridHealthGate::with_interval(Arc::clone(&runtime), std::time::Duration::from_millis(10)),
            DisabledIngress::new(),
            SystemClock::new(),
        );
```

`ok_runtime()` должен проходить health-гейт мгновенно — docker-health ветка:

```rust
    fn ok_runtime() -> MockContainerRuntime {
        let mut runtime = MockContainerRuntime::new();
        runtime.expect_build().returning(|_, _| Ok(()));
        runtime.expect_up().returning(|_, _| Ok(()));
        runtime.expect_ps().returning(|_| {
            Ok(vec![pi_domain::entities::ServiceState {
                service: "web".into(),
                state: "running".into(),
                health: Some("healthy".into()),
            }])
        });
        runtime
    }
```

- [ ] **Step 8: Прогнать всё**

Run: `rtk cargo test --workspace`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(deploy): env injection, secret masking, health gate and ingress stages"
```

---

### Task 10: Use-cases `SendEnv` + `ListEnvKeys`

**Files:**
- Create: `crates/application/src/env.rs`
- Modify: `crates/application/src/lib.rs` (`pub mod env;`)

- [ ] **Step 1: Модуль с падающими тестами**

`crates/application/src/env.rs` (в `lib.rs` — `pub mod env;`):

```rust
use std::sync::Arc;

use pi_domain::contracts::{
    ContainerRuntime, EnvFileWriter, LogSink, OverrideStore, ProjectRepository, SecretStore,
    Source,
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
        Arc::new(SendEnv { secrets, projects, source, env_files, overrides, runtime })
    }

    pub async fn execute(
        &self,
        project: &str,
        bundle: EnvBundle,
        apply: bool,
        log: Arc<dyn LogSink>,
    ) -> Result<EnvSaved, DomainError> {
        todo!()
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
    use pi_domain::entities::{HealthcheckConfig, Project, ProjectConfig};
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
                healthcheck: HealthcheckConfig::default(),
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
        // no projects/env_files/runtime expectations: any call would panic

        let saved = build(m)
            .execute("rateme", bundle(), false, CollectSink::new())
            .await
            .unwrap();
        assert_eq!(saved, EnvSaved { keys: 2, applied: false });
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
            .withf(|p, s, hp, cp| p == "rateme" && s == "web" && *hp == 8000 && *cp == 3000)
            .times(1)
            .returning(|_, _, _, _| Ok(PathBuf::from("/ov/rateme.yml")));
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
        let saved = build(m).execute("rateme", bundle(), true, sink.clone()).await.unwrap();

        assert_eq!(saved, EnvSaved { keys: 2, applied: true });
        let lines = sink.lines.lock().unwrap();
        assert!(lines.iter().any(|l| l.contains("***DB_PASSWORD***")), "lines: {lines:?}");
        assert!(!lines.iter().any(|l| l.contains("hunter2-long")), "secret leaked");
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
        secrets.expect_load().withf(|p| p == "rateme").returning(|_| Ok(bundle()));
        let keys = ListEnvKeys::new(Arc::new(secrets)).execute("rateme").await.unwrap();
        assert_eq!(keys, vec!["DB_PASSWORD".to_string(), "PORT".to_string()]);
    }
}
```

- [ ] **Step 2: Запустить — убедиться, что падает**

Run: `rtk cargo test -p pi-application env`
Expected: FAIL (`todo!()` в `SendEnv::execute`; `list_env_keys_...` уже PASS).

- [ ] **Step 3: Реализация `SendEnv::execute`**

```rust
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
            return Ok(EnvSaved { keys, applied: false });
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
            .write(project, &config.service, registered.host_port, config.container_port)
            .await?;
        let stack = ComposeStack {
            project_name: config.name.clone(),
            workdir: workdir.clone(),
            compose_file: workdir.join(&config.compose_path),
            override_file,
        };
        self.runtime.up(&stack, log).await?;
        Ok(EnvSaved { keys, applied: true })
    }
```

- [ ] **Step 4: Прогнать тесты**

Run: `rtk cargo test -p pi-application env`
Expected: PASS (5 тестов).

- [ ] **Step 5: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(app): SendEnv and ListEnvKeys use-cases"
```

---

### Task 11: HTTP API агента — `PUT/GET /v1/projects/{name}/env`

**Files:**
- Modify: `crates/bin/src/proto.rs` (env-DTO)
- Modify: `crates/bin/src/agent/state.rs` (`AppState` + wiring use-cases)
- Modify: `crates/bin/src/agent/http.rs` (роуты + хендлеры + тесты)

- [ ] **Step 1: DTO**

`crates/bin/src/proto.rs` — добавить:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSendRequest {
    pub vars: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub apply: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSendResponse {
    pub saved_keys: usize,
    pub applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvKeysResponse {
    pub keys: Vec<String>,
}
```

- [ ] **Step 2: Падающие тесты HTTP**

В `mod tests` файла `crates/bin/src/agent/http.rs`:

```rust
    fn put_json(uri: &str, body: &serde_json::Value) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::put(uri)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn env_send_then_ls_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(dir.path(), Arc::new(ok_source()), Arc::new(ok_runtime())));

        let body = serde_json::json!({ "vars": { "DB_PASSWORD": "hunter2-long" }, "apply": false });
        let (status, json) = request(app.clone(), put_json("/v1/projects/rateme/env", &body)).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["saved_keys"], 1);
        assert_eq!(json["applied"], false);

        let (status, json) = request(app.clone(), get_req("/v1/projects/rateme/env")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["keys"], serde_json::json!(["DB_PASSWORD"]));
    }

    #[tokio::test]
    async fn env_send_rejects_bad_keys_and_multiline_values() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(dir.path(), Arc::new(ok_source()), Arc::new(ok_runtime())));

        let bad_key = serde_json::json!({ "vars": { "BAD-DASH": "x" } });
        let (status, _) = request(app.clone(), put_json("/v1/projects/rateme/env", &bad_key)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let bad_value = serde_json::json!({ "vars": { "OK": "line1\nline2" } });
        let (status, _) = request(app.clone(), put_json("/v1/projects/rateme/env", &bad_value)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn env_apply_for_unknown_project_is_404() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(dir.path(), Arc::new(ok_source()), Arc::new(ok_runtime())));
        let body = serde_json::json!({ "vars": { "A_KEY": "value-long-enough" }, "apply": true });
        let (status, _) = request(app, put_json("/v1/projects/ghost/env", &body)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn env_keys_for_project_without_env_is_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(dir.path(), Arc::new(ok_source()), Arc::new(ok_runtime())));
        let (status, json) = request(app, get_req("/v1/projects/ghost/env")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["keys"], serde_json::json!([]));
    }
```

Run: `rtk cargo test -p pi env_` → FAIL (нет роутов/стейта).

- [ ] **Step 3: `AppState` + wiring**

`crates/bin/src/agent/state.rs`:

```rust
use pi_application::env::{ListEnvKeys, SendEnv};
```

В `AppState` добавить:

```rust
    pub send_env: Arc<SendEnv>,
    pub env_keys: Arc<ListEnvKeys>,
```

`source`, `runtime`, `overrides`, `secrets`, `env_files` нужны и деплою, и `SendEnv` — конкретные `Arc` клонируются перед передачей. Итоговый `build_state` целиком:

```rust
pub fn build_state(config: &AgentConfig) -> anyhow::Result<AppState> {
    std::fs::create_dir_all(&config.data_dir)?;
    let db = Db::open(&config.data_dir.join("state.db")).map_err(|e| anyhow::anyhow!("{e}"))?;

    let projects = SqliteProjectRepo::new(db.clone(), config.port_min, config.port_max);
    let history: Arc<dyn DeploymentHistory> = SqliteHistory::new(db);
    let source = GitSource::new(&config.data_dir);
    let runtime = DockerComposeRuntime::new();
    let overrides = FsOverrideStore::new(config.data_dir.join("overrides"));
    let secrets = EncryptedFileStore::open(&config.data_dir).map_err(|e| anyhow::anyhow!("{e}"))?;
    let env_files = FsEnvFileWriter::new();
    let health = HybridHealthGate::new(runtime.clone());
    let ingress: Arc<dyn Ingress> = match &config.cloudflared {
        Some(cf) => CloudflaredIngress::new(cf.config.clone(), cf.tunnel.clone(), cf.restart.clone()),
        None => DisabledIngress::new(),
    };

    let deploy = DeployProject::new(
        source.clone(),
        runtime.clone(),
        projects.clone(),
        Arc::clone(&history),
        overrides.clone(),
        secrets.clone(),
        Arc::clone(&env_files) as Arc<dyn pi_domain::contracts::EnvFileWriter>,
        health,
        ingress,
        SystemClock::new(),
    );
    let list = ListProjects::new(projects.clone(), runtime.clone());
    let send_env = SendEnv::new(
        secrets.clone(),
        projects,
        source,
        env_files,
        overrides,
        runtime,
    );
    let env_keys = ListEnvKeys::new(secrets);

    Ok(AppState {
        deploy,
        list,
        history,
        hub: DeployEventsHub::new(),
        ids: UuidGen::new(),
        send_env,
        env_keys,
    })
}
```

(клоны `Arc<КонкретныйТип>` коэрсятся в `Arc<dyn Трейт>` на позициях аргументов; явный `as` нужен только там, где компилятор не выведет сам). Аналогично дополнить тестовый `state_with` в `http.rs`:

```rust
        let send_env = SendEnv::new(
            secrets.clone(),
            projects.clone(),
            Arc::clone(&source),
            FsEnvFileWriter::new(),
            overrides.clone(),
            Arc::clone(&runtime),
        );
        let env_keys = ListEnvKeys::new(secrets.clone());
```

(для этого `source`/`overrides`/`secrets` в `state_with` передавать в `DeployProject::new` через `.clone()`/`Arc::clone`, а `send_env, env_keys` добавить в литерал `AppState`).

- [ ] **Step 4: Роуты и хендлеры**

`crates/bin/src/agent/http.rs`:

```rust
use axum::routing::{get, post, put};
use pi_domain::entities::{DeployRef, DeploymentStatus, EnvBundle, ProjectConfig};
use crate::proto::{
    DeployAccepted, DeployRequest, DeploymentDto, EnvKeysResponse, EnvSendRequest,
    EnvSendResponse, ProjectViewDto, VersionInfo,
};
```

В `router()`:

```rust
        .route("/v1/projects/{name}/env", put(send_env).get(env_keys))
```

Хендлеры:

```rust
/// `pi env send --apply` runs `up -d` synchronously; its output goes to the
/// agent log (journald), the CLI gets a compact JSON summary.
struct TracingSink;

impl pi_domain::contracts::LogSink for TracingSink {
    fn line(&self, line: &str) {
        tracing::info!("{line}");
    }
    fn finished(&self, _status: DeploymentStatus) {}
}

async fn send_env(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<EnvSendRequest>,
) -> Result<Json<EnvSendResponse>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    for (key, value) in &req.vars {
        if !pi_infrastructure::dotenv::is_valid_key(key) {
            return Err(ApiError(DomainError::Invalid(format!("invalid env key '{key}'"))));
        }
        if value.contains('\n') {
            return Err(ApiError(DomainError::Invalid(format!(
                "value of '{key}' contains a newline (multi-line values are unsupported)"
            ))));
        }
    }
    let bundle = EnvBundle { vars: req.vars };
    let saved = state
        .send_env
        .execute(&name, bundle, req.apply, Arc::new(TracingSink))
        .await
        .map_err(ApiError)?;
    Ok(Json(EnvSendResponse { saved_keys: saved.keys, applied: saved.applied }))
}

async fn env_keys(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<EnvKeysResponse>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    let keys = state.env_keys.execute(&name).await.map_err(ApiError)?;
    Ok(Json(EnvKeysResponse { keys }))
}
```

- [ ] **Step 5: Прогнать тесты**

Run: `rtk cargo test -p pi`
Expected: PASS (4 новых + все старые).

- [ ] **Step 6: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(agent): env endpoints (PUT/GET /v1/projects/{name}/env)"
```

---

### Task 12: `pi.toml` — `[env]` + `[healthcheck]`; healthcheck в wire-DTO

**Files:**
- Modify: `crates/bin/src/cli/pitoml.rs`
- Modify: `crates/bin/src/proto.rs` (`HealthcheckDto` в `ProjectDto`)

- [ ] **Step 1: Падающие тесты pitoml**

В `mod tests` файла `crates/bin/src/cli/pitoml.rs` (SAMPLE уже содержит `[healthcheck] path="/"` и `[env] file=".env"`):

```rust
    #[test]
    fn env_and_healthcheck_sections_are_parsed_with_defaults() {
        let parsed = PiToml::parse(SAMPLE).unwrap();
        assert_eq!(parsed.env.file, ".env");
        let config = parsed.to_project_config();
        assert_eq!(config.healthcheck.path.as_deref(), Some("/"));
        assert_eq!(config.healthcheck.expect, None);
        assert_eq!(config.healthcheck.timeout_secs, 60, "default budget");
    }

    #[test]
    fn missing_env_and_healthcheck_sections_fall_back_to_defaults() {
        let toml = SAMPLE
            .replace("[healthcheck]\npath = \"/\"\n", "")
            .replace("[env]\nfile = \".env\"\n", "");
        let parsed = PiToml::parse(&toml).unwrap();
        assert_eq!(parsed.env.file, ".env");
        let config = parsed.to_project_config();
        assert_eq!(config.healthcheck.path, None, "no path -> TCP probe");
        assert_eq!(config.healthcheck.timeout_secs, 60);
    }

    #[test]
    fn healthcheck_timeout_and_expect_are_validated() {
        let toml = SAMPLE.replace("path = \"/\"", "path = \"/\"\ntimeout = \"2m\"\nexpect = \"204\"");
        let config = PiToml::parse(&toml).unwrap().to_project_config();
        assert_eq!(config.healthcheck.timeout_secs, 120);
        assert_eq!(config.healthcheck.expect.as_deref(), Some("204"));

        let bad = SAMPLE.replace("path = \"/\"", "path = \"/\"\ntimeout = \"soon\"");
        assert!(PiToml::parse(&bad).is_err());
        let bad = SAMPLE.replace("path = \"/\"", "path = \"/\"\nexpect = \"ok\"");
        assert!(PiToml::parse(&bad).is_err());
    }

    #[test]
    fn parse_duration_secs_supports_s_m_and_bare_numbers() {
        assert_eq!(parse_duration_secs("60s").unwrap(), 60);
        assert_eq!(parse_duration_secs("2m").unwrap(), 120);
        assert_eq!(parse_duration_secs("90").unwrap(), 90);
        assert!(parse_duration_secs("soon").is_err());
    }
```

Run: `rtk cargo test -p pi pitoml` → FAIL.

- [ ] **Step 2: Реализация секций**

`crates/bin/src/cli/pitoml.rs`. В структуру `PiToml` добавить:

```rust
    #[serde(default)]
    pub healthcheck: HealthcheckSection,
    #[serde(default)]
    pub env: EnvSection,
```

Новые секции:

```rust
#[derive(Debug, Default, Deserialize)]
pub struct HealthcheckSection {
    pub path: Option<String>,
    /// "2xx" | "3xx" | exact code like "204".
    pub expect: Option<String>,
    /// "60s" | "2m" | bare seconds. Default 60s (§22).
    pub timeout: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EnvSection {
    /// Which local file `pi env send` reads (§12).
    #[serde(default = "default_env_file")]
    pub file: String,
}

impl Default for EnvSection {
    fn default() -> EnvSection {
        EnvSection { file: default_env_file() }
    }
}

fn default_env_file() -> String {
    ".env".into()
}

pub(crate) fn parse_duration_secs(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (digits, mult) = if let Some(d) = s.strip_suffix('m') {
        (d, 60)
    } else if let Some(d) = s.strip_suffix('s') {
        (d, 1)
    } else {
        (s, 1)
    };
    digits
        .trim()
        .parse::<u64>()
        .map(|n| n * mult)
        .map_err(|_| format!("invalid duration '{s}' (expected like \"60s\" or \"2m\")"))
}

fn validate_expect(expect: &str) -> Result<(), String> {
    let ok = matches!(expect, "2xx" | "3xx")
        || (expect.len() == 3 && expect.chars().all(|c| c.is_ascii_digit()));
    if ok {
        Ok(())
    } else {
        Err(format!("invalid [healthcheck].expect '{expect}' (use \"2xx\", \"3xx\" or a code like \"204\")"))
    }
}
```

В `PiToml::parse` после проверки `schema` добавить валидацию (ошибки — сразу при чтении конфига, §19):

```rust
        if let Some(timeout) = &parsed.healthcheck.timeout {
            parse_duration_secs(timeout).map_err(|e| anyhow::anyhow!("pi.toml [healthcheck]: {e}"))?;
        }
        if let Some(expect) = &parsed.healthcheck.expect {
            validate_expect(expect).map_err(|e| anyhow::anyhow!("pi.toml [healthcheck]: {e}"))?;
        }
```

В `to_project_config` заменить заглушку `healthcheck`:

```rust
            healthcheck: HealthcheckConfig {
                path: self.healthcheck.path.clone(),
                expect: self.healthcheck.expect.clone(),
                // already validated in parse(); fall back to the 60s default
                timeout_secs: self
                    .healthcheck
                    .timeout
                    .as_deref()
                    .and_then(|t| parse_duration_secs(t).ok())
                    .unwrap_or(60),
            },
```

(импорт: `use pi_domain::entities::{HealthcheckConfig, ProjectConfig};`).

- [ ] **Step 3: Healthcheck в `ProjectDto`**

`crates/bin/src/proto.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthcheckDto {
    pub path: Option<String>,
    pub expect: Option<String>,
    pub timeout_secs: Option<u64>,
}
```

В `ProjectDto` добавить поле (с дефолтом — v0.1-клиенты его не шлют):

```rust
    #[serde(default)]
    pub healthcheck: Option<HealthcheckDto>,
```

Конвертации:

```rust
// в From<ProjectDto> for ProjectConfig — заменить заглушку:
            healthcheck: dto
                .healthcheck
                .map(|h| HealthcheckConfig {
                    path: h.path,
                    expect: h.expect,
                    timeout_secs: h.timeout_secs.unwrap_or(60),
                })
                .unwrap_or_default(),

// в From<&ProjectConfig> for ProjectDto — добавить:
            healthcheck: Some(HealthcheckDto {
                path: config.healthcheck.path.clone(),
                expect: config.healthcheck.expect.clone(),
                timeout_secs: Some(config.healthcheck.timeout_secs),
            }),
```

Тест в `proto.rs` (новый `mod tests`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v01_deploy_request_without_healthcheck_still_deserializes() {
        let json = r#"{"project":{"name":"a","repo":"r","branch":"main","compose":"docker-compose.yml","service":"web","port":3000,"hostname":null},"ref":null}"#;
        let req: DeployRequest = serde_json::from_str(json).unwrap();
        let config: pi_domain::entities::ProjectConfig = req.project.into();
        assert_eq!(config.healthcheck.timeout_secs, 60);
    }

    #[test]
    fn healthcheck_roundtrips_through_dto() {
        let mut config: pi_domain::entities::ProjectConfig = ProjectDto {
            name: "a".into(),
            repo: "r".into(),
            branch: "main".into(),
            compose: "docker-compose.yml".into(),
            service: "web".into(),
            port: 3000,
            hostname: None,
            healthcheck: None,
        }
        .into();
        config.healthcheck.path = Some("/health".into());
        config.healthcheck.timeout_secs = 120;
        let dto = ProjectDto::from(&config);
        let back: pi_domain::entities::ProjectConfig = dto.into();
        assert_eq!(back.healthcheck, config.healthcheck);
    }
}
```

- [ ] **Step 4: Прогнать тесты**

Run: `rtk cargo test -p pi`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(cli): [env] and [healthcheck] pi.toml sections, healthcheck in wire DTO"
```

---

### Task 13: CLI — `pi env send` / `pi env ls`

**Files:**
- Modify: `crates/bin/src/cli/api.rs`
- Modify: `crates/bin/src/cli/commands.rs`
- Modify: `crates/bin/src/main.rs`

- [ ] **Step 1: API-клиент**

`crates/bin/src/cli/api.rs` — импорт DTO дополнить `EnvKeysResponse, EnvSendRequest, EnvSendResponse`; методы:

```rust
    pub async fn send_env(
        &self,
        project: &str,
        req: &EnvSendRequest,
    ) -> anyhow::Result<EnvSendResponse> {
        let resp = self
            .http
            .put(format!("{}/v1/projects/{project}/env", self.base))
            .json(req)
            .send()
            .await?;
        Ok(extract_error(resp).await?.json().await?)
    }

    pub async fn env_keys(&self, project: &str) -> anyhow::Result<EnvKeysResponse> {
        let resp = self
            .http
            .get(format!("{}/v1/projects/{project}/env", self.base))
            .send()
            .await?;
        Ok(extract_error(resp).await?.json().await?)
    }
```

- [ ] **Step 2: Команды**

`crates/bin/src/cli/commands.rs` — добавить (импорты: `std::path::PathBuf`, `crate::proto::EnvSendRequest`):

```rust
pub async fn env_send(
    file: Option<PathBuf>,
    apply: bool,
    server: Option<String>,
) -> anyhow::Result<()> {
    let pitoml = PiToml::load(Path::new("pi.toml"))?;
    let path = file.unwrap_or_else(|| PathBuf::from(&pitoml.env.file));
    let text = std::fs::read_to_string(&path).map_err(|e| {
        anyhow::anyhow!(
            "cannot read {}: {e} (set [env].file in pi.toml or pass --file)",
            path.display()
        )
    })?;
    let bundle = pi_infrastructure::dotenv::parse(&text)
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    if bundle.is_empty() {
        anyhow::bail!("{}: no variables found", path.display());
    }

    let profile = ClientConfig::load()?.select(server.as_deref())?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());
    let resp = api
        .send_env(&pitoml.project.name, &EnvSendRequest { vars: bundle.vars, apply })
        .await?;
    if resp.applied {
        eprintln!(
            "env saved ({} keys) and applied: `up -d` recreated affected services",
            resp.saved_keys
        );
    } else {
        eprintln!(
            "env saved ({} keys); applies on next deploy (or re-run with --apply)",
            resp.saved_keys
        );
    }
    Ok(())
}

pub async fn env_ls(server: Option<String>) -> anyhow::Result<()> {
    let pitoml = PiToml::load(Path::new("pi.toml"))?;
    let profile = ClientConfig::load()?.select(server.as_deref())?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());
    let resp = api.env_keys(&pitoml.project.name).await?;
    if resp.keys.is_empty() {
        println!("no env stored for project '{}'", pitoml.project.name);
    } else {
        for key in resp.keys {
            println!("{key}");
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Сабкоманды clap**

`crates/bin/src/main.rs` — в `enum Cmd` (после `Ls`):

```rust
    /// Project secrets: send/list the EnvBundle (reads ./pi.toml)
    Env {
        #[command(subcommand)]
        cmd: EnvCmd,
    },
```

```rust
#[derive(Subcommand)]
enum EnvCmd {
    /// Securely send a local env file to the agent (encrypted at rest)
    Send {
        /// Env file to read (default: [env].file from pi.toml)
        #[arg(long)]
        file: Option<PathBuf>,
        /// Apply immediately: re-inject .env and run `up -d`
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        server: Option<String>,
    },
    /// List stored env key names (values are never shown)
    Ls {
        #[arg(long)]
        server: Option<String>,
    },
}
```

В `match`:

```rust
        Cmd::Env { cmd: EnvCmd::Send { file, apply, server } } => {
            cli::commands::env_send(file, apply, server).await
        }
        Cmd::Env { cmd: EnvCmd::Ls { server } } => cli::commands::env_ls(server).await,
```

- [ ] **Step 4: Проверка**

Run: `rtk cargo test --workspace` → PASS; `rtk cargo run -p pi -- env --help` → показывает `send`/`ls`.

Опциональный smoke на Windows (без Pi): поднять агент `rtk cargo run -p pi -- agent run --config dev/agent.toml`, во втором терминале из каталога с `pi.toml` и `.env`:
`$env:PI_AGENT_URL='http://127.0.0.1:7700'; rtk cargo run -p pi -- env send; rtk cargo run -p pi -- env ls` → `saved`, затем список ключей.

- [ ] **Step 5: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(cli): pi env send / pi env ls"
```

---

### Task 14: Документация, версия, финальная проверка

**Files:**
- Modify: `docs/install-agent-v0.1.md`
- Modify: `Cargo.toml` (`version = "0.2.0"`)

- [ ] **Step 1: Обновить установочный гайд**

В `docs/install-agent-v0.1.md`:

1. В пример `/etc/pi/agent.toml` (раздел 2) добавить:

```toml
    # v0.2: авто-ingress через cloudflared (опционально; без секции — выключено)
    # [cloudflared]
    # config = "/var/lib/pi/cloudflared/config.yml"
    # tunnel = "home"
    # restart = ["systemctl", "--user", "restart", "cloudflared"]
```

2. Добавить новый раздел «2.1 Cloudflare Tunnel (один раз, до v0.5 — вручную)»:

```markdown
## 2.1 Cloudflare Tunnel (один раз; авто-бутстрап появится в v0.5)

    # linger: агент сможет рестартовать cloudflared без sudo (§11)
    sudo loginctl enable-linger pi-agent

    # логин и создание туннеля (выполняется под pi-agent)
    sudo mkdir -p /var/lib/pi/cloudflared
    sudo chown pi-agent:pi-agent /var/lib/pi/cloudflared
    sudo -u pi-agent env TUNNEL_ORIGIN_CERT=/var/lib/pi/cloudflared/cert.pem \
        cloudflared tunnel login
    sudo -u pi-agent env TUNNEL_ORIGIN_CERT=/var/lib/pi/cloudflared/cert.pem \
        cloudflared tunnel create home   # credentials json появится рядом

    # стартовый config.yml (ingress-правила дальше ведёт сам агент)
    sudo -u pi-agent tee /var/lib/pi/cloudflared/config.yml >/dev/null <<'EOF'
    tunnel: home
    credentials-file: /var/lib/pi/cloudflared/<TUNNEL_ID>.json
    ingress:
      - service: http_status:404
    EOF

    # user-юнит cloudflared под pi-agent (живёт независимо от pi-agent, §11)
    sudo -u pi-agent mkdir -p /var/lib/pi/.config/systemd/user
    sudo -u pi-agent tee /var/lib/pi/.config/systemd/user/cloudflared.service >/dev/null <<'EOF'
    [Unit]
    Description=cloudflared tunnel (managed by pi)

    [Service]
    ExecStart=/usr/local/bin/cloudflared --origincert /var/lib/pi/cloudflared/cert.pem \
        tunnel --config /var/lib/pi/cloudflared/config.yml run
    Restart=on-failure

    [Install]
    WantedBy=default.target
    EOF
    sudo -u pi-agent XDG_RUNTIME_DIR=/run/user/$(id -u pi-agent) \
        systemctl --user enable --now cloudflared

Примечание: у `pi-agent` нет home — задайте ему `HOME=/var/lib/pi`
(`sudo usermod -d /var/lib/pi pi-agent`) до этих шагов, иначе user-юниты не найдутся.
В `agent.toml` раскомментируйте `[cloudflared]` и перезапустите `pi-agent`.
```

3. Раздел «5. Ручные шаги v0.1» переписать: пункты про `.env` и cloudflared-правило удалить (закрывает v0.2 — `pi env send` и авто-ingress), остаётся только deploy-key для приватных репо.

4. Добавить раздел «7. Приёмка v0.2 (критерий §23: новый проект — без ручной настройки на Pi)»:

```markdown
## 7. Приёмка v0.2

1. В новом проекте: `pi env send` → `env saved (N keys)`; `pi env ls` —
   имена ключей без значений.
2. `pi deploy` → в логах `.env injected (N keys)`, `healthcheck: passed`,
   `ingress: routing <hostname> -> http://127.0.0.1:<port>`; страница открывается
   по `https://<hostname>` без ручных шагов на Pi.
3. В выводе деплоя и в `GET /v1/deployments/{id}` значения секретов заменены
   на `***KEY***` (проверить grep'ом по значению).
4. Повторный `pi deploy` без изменений: `ingress: ... already routed; cloudflared
   untouched` — туннель не рвётся (`journalctl --user-unit cloudflared` тих).
5. `pi env send --apply` с изменённым значением → сервис пересоздан, новое
   значение видно в контейнере (`docker exec <c> env | grep KEY`).
6. Сломать healthcheck (например, `expect = "500"`) → деплой `failed` с
   `health check failed: timed out...`, стек остаётся запущенным.
7. На Pi: `/var/lib/pi/secrets/<project>.env.age` не содержит плэйнтекста
   (`strings | grep <значение>` пуст), `secret.key` имеет права `0600`.
```

- [ ] **Step 2: Поднять версию workspace**

`Cargo.toml`: `version = "0.1.0"` → `version = "0.2.0"` (в `[workspace.package]`; `/v1/version` начнёт отдавать 0.2.0).

- [ ] **Step 3: Финальная проверка**

```bash
rtk cargo fmt --all
rtk cargo clippy --workspace --all-targets
rtk cargo test --workspace
```

Expected: fmt без диффа, clippy без warnings, все тесты PASS.

- [ ] **Step 4: Commit**

```bash
rtk git add -A && rtk git commit -m "docs: v0.2 install/acceptance guide; bump version to 0.2.0"
```

---

## Сверка со спекой (самопроверка плана)

| Требование v0.2 (§23) | Где в плане |
|---|---|
| `EncryptedFileStore` (age), ключ агента `0600` | Task 4 |
| `pi env send/ls`, `--apply` (§10) | Tasks 10, 11, 13 |
| `.env` в workdir, остаётся, `0600`, ре-инжект при apply (§10) | Tasks 5, 9, 10 |
| Маскировка секретов ≥6 симв. → `***KEY***` во всех потоках (§8.1) | Tasks 8, 9 (deploy: SSE + log_tail), 10 (apply) |
| `CloudflaredIngress`: upsert + `tunnel route dns` + diff-restart (§11) | Task 7, wiring в Task 9 |
| Health-check-гейт: docker → HTTP → TCP, таймаут из `[healthcheck]` (§8) | Tasks 6, 12, гейт в Task 9 |
| `pi env ls` — только имена ключей (§10) | Tasks 10, 11 |
| Деплой без `[cloudflared]` не ломается (dev/Windows) | Task 7 (`DisabledIngress`), Task 9 |
| Критерий: новый проект без ручной настройки на Pi | Task 14 (приёмка, чек-лист) |

Отложено сознательно: ре-инжект `.env` на `start/stop/restart` (§10) — сами lifecycle-команды появляются в v0.4; `pi rm` + DNS-инструкция — v0.4; бутстрап cloudflared в `pi agent setup` — v0.5.

