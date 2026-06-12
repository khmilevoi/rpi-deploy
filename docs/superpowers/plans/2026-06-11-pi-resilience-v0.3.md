# pi v0.3 (Устойчивость: CI-ready) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** GitHub Actions деплоит без присмотра (§23 v0.3): очередь latest-wins (глубина 1, `superseded`), поэтапные таймауты с убийством дочерних процессов, `pi deploy --cancel`, свип `queued|running → interrupted` при старте, глобальный build-семафор, GC диска (`image prune` после успеха + `builder prune` по порогу) + `pi gc`, ретеншен БД, CI-флаги `--host/--user/--key`, version-handshake warning и пример workflow в доках.

**Architecture:** расширяем слои v0.2 по тем же правилам (§5): в `domain` — новые статусы деплоя, ошибки (`Canceled`/`Timeout`/`Conflict`), `StageTimeouts` и контракты (`DeploymentHistory` — queued/mark_running/active/sweep; `ContainerRuntime` — prune; новый `DiskProbe`); в `application` — `RunGc`, рефакторинг `DeployProject` (cancel-token, таймауты, семафор, GC-стадия) и новый `DeployScheduler` (latest-wins очередь, владеет сериализацией вместо `DeployLocks`); в `infrastructure` — `kill_on_drop`, новые методы `SqliteHistory`/`DockerComposeRuntime`, `SysinfoDiskProbe`; в `bin` — конфиг (`[timeouts]`/`[gc]`/`build_concurrency`/`history_keep`), роуты `DELETE /v1/deployments/{id}`, `GET /v1/projects/{name}/deployments/active`, `POST /v1/gc`, свип при старте, CLI-флаги.

**Tech Stack:** как v0.2 + `tokio-util = "0.7"` (`CancellationToken`), `sysinfo = "0.33"` (процент занятости диска). Таймауты — `tokio::time::timeout`; семафор — `tokio::sync::Semaphore`.

**Спека:** `docs/superpowers/specs/2026-06-09-pi-deploy-tool-design.md` — §8.1 (устойчивость), §9.1 (handshake), §12 (`[timeouts]`), §16 (CI-флаги), §18 (ретеншен/свип), §23 v0.3 (границы скоупа).

---

## Скоуп v0.3 (что входит / что НЕ входит)

Входит (§23 v0.3):
- Очередь latest-wins: pending-слот глубины 1 на проект, вытесненный → `superseded` (§8.1).
- Поэтапные таймауты fetch/build/up: дефолты в `agent.toml` (2м/30м/5м), override `[timeouts]` в `pi.toml`; просроченный этап убивает дочерний процесс, деплой → `failed` с `timeout: <stage>` (§8.1).
- `pi deploy --cancel` → `DELETE /v1/deployments/{id}`, статус `canceled` (§8.1).
- Свип при старте агента: `queued|running` → `interrupted` (§8.1, §18).
- Глобальный build-семафор (размер 1, настраиваемый) (§8.1).
- GC: `docker image prune -f` после успешного деплоя; `docker builder prune` при диске ≥ порога (85%, настраиваемый); `pi gc` + `POST /v1/gc` (§8.1).
- Ретеншен БД: последние 50 деплоев на проект (настраиваемо), чистка после каждой вставки (§18).
- CI-флаги `pi deploy --host/--user/--key` (минуя клиентский конфиг) (§16).
- Version-handshake warning при расхождении версий CLI/агента (§9.1; ошибка на 404 уже есть с v0.1).
- Доки: пример GitHub Actions workflow.

НЕ входит (позже по роадмапу): вопрос «отменить?» при Ctrl+C во время follow (интерактив — v0.5, в v0.3 только явный `--cancel`); `pi doctor`/`stats`/lifecycle/`pi rm` (v0.4); `pi agent setup/update` (v0.5 — warning лишь подсказывает обновить бинарь); `stats_snapshots` и его ретеншен (таблица появится в v0.4); авто-резюм прерванных деплоев (§8.1 — явно «нет»).

Решения, зафиксированные планом (в рамках §22):
- **Сериализацию деплоев владеет `DeployScheduler`** (application), `DeployLocks`/`try_begin`/409 удаляются: submit никогда не отклоняет — он либо стартует, либо ставит/замещает pending. `DomainError::DeployInProgress` удаляется, добавляется `Conflict` (409 для «отмена уже завершённого»).
- **Жизненный цикл записи в БД:** `record_queued` (INSERT, status=`queued`, при сабмите) → `mark_running` (UPDATE + свежий `started_at`, при фактическом старте) → `record_finished` (любой терминальный статус). Ретеншен — внутри `record_queued` адаптера (keep — параметр конструктора `SqliteHistory`).
- **Отмена кооперативная:** `CancellationToken` + `tokio::select!` вокруг стадий; убийство дочерних процессов гарантирует `kill_on_drop(true)` в `process.rs` (оно же убивает процесс по таймауту стадии).
- **GC — стадия деплоя после ingress**: ошибки GC логируются и НЕ роняют успешный деплой; `pi gc` использует тот же use-case `RunGc`.
- **Порог диска** меряется по файловой системе `data_dir` (на Pi это та же SD-карта, что и docker).
- **Exit-коды CLI `pi deploy`:** `success` и `superseded` → 0 (latest wins: новый деплой уже несёт свежий ref, красный CI был бы ложным), остальные (`failed`/`canceled`/`interrupted`) → 1.
- **`pi deploy --cancel`** отменяет ВСЕ активные деплои проекта (queued + running) — id резолвятся через новый `GET /v1/projects/{name}/deployments/active`.
- **`docker builder prune`** — с `--filter until=24h` (константа адаптера; §22 требует настраиваемости только порога диска).

## Конвенции для исполнителя

- **Все команды** запускать с префиксом `rtk`: `rtk cargo test`, `rtk git add …`.
- Коммит-сообщения — conventional commits на английском; завершать трейлером `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Разработка на **Windows**, целевая платформа агента — Linux. Юнит-тесты обязаны проходить на Windows; интеграция с реальным docker — `#[ignore]`.
- Код и комментарии — на английском.
- Без `unwrap()`/`expect()` в use-cases/адаптерах — ошибки через `Result` (§19). В тестах `unwrap()` допустим.
- После каждого зелёного шага — коммит. Один таск = один коммит.

## File Structure

```
crates/
├─ domain/src/
│  ├─ entities.rs        # MOD: +Queued/Canceled/Interrupted/Superseded, StageTimeouts(+Overrides), ProjectConfig.timeouts
│  ├─ error.rs           # MOD: +Canceled, +Timeout{stage,secs}, +Conflict; −DeployInProgress (Task 8)
│  └─ contracts.rs       # MOD: DeploymentHistory (record_queued/mark_running/active/sweep_interrupted),
│                        #      ContainerRuntime (+prune_images/+prune_builder), +DiskProbe
├─ application/src/
│  ├─ gc.rs              # NEW: RunGc + GcReport (§8.1)
│  ├─ scheduler.rs       # NEW: DeployScheduler + DeployRunner + Submit/CancelOutcome (latest-wins, §8.1)
│  ├─ deploy.rs          # MOD: cancel-token, поэтапные таймауты, build-семафор, GC-стадия; permit удалён (Task 8)
│  ├─ locks.rs           # DEL (Task 8): сериализацией владеет scheduler
│  └─ lib.rs             # MOD: +gc, +scheduler, −locks
├─ infrastructure/src/
│  ├─ disk.rs            # NEW: SysinfoDiskProbe
│  ├─ process.rs         # MOD: kill_on_drop(true) — таймаут/отмена убивает дочерний процесс
│  ├─ history.rs         # MOD: новые методы + ретеншен (§18); SqliteHistory::new(db, keep)
│  ├─ docker.rs          # MOD: prune_images / prune_builder
│  └─ lib.rs             # MOD: +disk
└─ bin/src/
   ├─ duration.rs        # NEW: parse_duration_secs (перенос из pitoml — общий cli+agent)
   ├─ proto.rs           # MOD: +TimeoutsDto в ProjectDto, DeployAccepted.queued, +GcResponse
   ├─ agent/config.rs    # MOD: +build_concurrency, +history_keep, +[timeouts], +[gc]
   ├─ agent/state.rs     # MOD: wiring scheduler/RunGc/DiskProbe/таймаутов
   ├─ agent/http.rs      # MOD: submit вместо try_begin, DELETE /v1/deployments/{id},
   │                     #      GET /v1/projects/{name}/deployments/active, POST /v1/gc
   ├─ agent/run.rs       # MOD: свип при старте
   ├─ cli/config.rs      # MOD: +ConnectOpts (--server/--host/--user/--key)
   ├─ cli/api.rs         # MOD: +cancel_deployment/active_deployments/gc; −409-спецкейс
   ├─ cli/commands.rs    # MOD: deploy --cancel, gc, exit-коды, version warning, ConnectOpts
   ├─ cli/pitoml.rs      # MOD: +[timeouts]; parse_duration_secs → crate::duration
   └─ main.rs            # MOD: +mod duration, флаги --cancel/--host/--user/--key, pi gc
docs/ci-github-actions.md     # NEW: пример workflow (§23 v0.3)
docs/install-agent-v0.1.md    # MOD: новые опции agent.toml
dev/agent.toml                # MOD: примеры новых опций (закомментированы)
Cargo.toml                    # MOD: +tokio-util, +sysinfo; version 0.3.0 (Task 12)
```

---

### Task 1: Domain — новые статусы, ошибки, сущности таймаутов

**Files:**
- Modify: `crates/domain/src/entities.rs`
- Modify: `crates/domain/src/error.rs`
- Modify (компил-фоллаут — новое поле `ProjectConfig.timeouts`): `crates/application/src/deploy.rs`, `crates/application/src/env.rs`, `crates/infrastructure/src/git.rs`, `crates/bin/src/proto.rs`, `crates/bin/src/cli/pitoml.rs`

- [ ] **Step 1: Написать падающие тесты в `entities.rs`**

В `mod tests` файла `crates/domain/src/entities.rs` заменить тесты `status_roundtrips_through_str` и `terminal_statuses` и добавить тест таймаутов:

```rust
    #[test]
    fn status_roundtrips_through_str() {
        for s in [
            DeploymentStatus::Queued,
            DeploymentStatus::Running,
            DeploymentStatus::Success,
            DeploymentStatus::Failed,
            DeploymentStatus::Canceled,
            DeploymentStatus::Interrupted,
            DeploymentStatus::Superseded,
        ] {
            assert_eq!(s.as_str().parse::<DeploymentStatus>(), Ok(s));
        }
        assert_eq!("bogus".parse::<DeploymentStatus>(), Err(()));
    }

    #[test]
    fn terminal_statuses() {
        assert!(!DeploymentStatus::Queued.is_terminal());
        assert!(!DeploymentStatus::Running.is_terminal());
        for s in [
            DeploymentStatus::Success,
            DeploymentStatus::Failed,
            DeploymentStatus::Canceled,
            DeploymentStatus::Interrupted,
            DeploymentStatus::Superseded,
        ] {
            assert!(s.is_terminal(), "{s:?} must be terminal");
        }
    }

    #[test]
    fn stage_timeouts_defaults_match_spec_and_overrides_win() {
        let defaults = StageTimeouts::default();
        assert_eq!(defaults.fetch_secs, 120, "fetch 2m (§8.1)");
        assert_eq!(defaults.build_secs, 1800, "build 30m (§8.1)");
        assert_eq!(defaults.up_secs, 300, "up 5m (§8.1)");

        let overrides = StageTimeoutOverrides {
            build_secs: Some(3600),
            ..StageTimeoutOverrides::default()
        };
        let effective = defaults.with_overrides(&overrides);
        assert_eq!(effective.fetch_secs, 120, "no override -> default");
        assert_eq!(effective.build_secs, 3600, "override wins");
        assert_eq!(effective.up_secs, 300);
    }
```

- [ ] **Step 2: Запустить — убедиться, что падает**

Run: `rtk cargo test -p pi-domain`
Expected: FAIL — нет вариантов `Queued`/`Canceled`/…, нет `StageTimeouts`.

- [ ] **Step 3: Реализовать статусы и сущности**

В `crates/domain/src/entities.rs` заменить enum `DeploymentStatus` (и его impl'ы) на:

```rust
/// All deployment statuses (§18). Stored as strings in the DB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentStatus {
    Queued,
    Running,
    Success,
    Failed,
    Canceled,
    Interrupted,
    Superseded,
}

impl DeploymentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeploymentStatus::Queued => "queued",
            DeploymentStatus::Running => "running",
            DeploymentStatus::Success => "success",
            DeploymentStatus::Failed => "failed",
            DeploymentStatus::Canceled => "canceled",
            DeploymentStatus::Interrupted => "interrupted",
            DeploymentStatus::Superseded => "superseded",
        }
    }

    pub fn is_terminal(&self) -> bool {
        !matches!(self, DeploymentStatus::Queued | DeploymentStatus::Running)
    }
}

impl std::str::FromStr for DeploymentStatus {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(DeploymentStatus::Queued),
            "running" => Ok(DeploymentStatus::Running),
            "success" => Ok(DeploymentStatus::Success),
            "failed" => Ok(DeploymentStatus::Failed),
            "canceled" => Ok(DeploymentStatus::Canceled),
            "interrupted" => Ok(DeploymentStatus::Interrupted),
            "superseded" => Ok(DeploymentStatus::Superseded),
            _ => Err(()),
        }
    }
}
```

Комментарий над старым enum («Statuses for v0.1…») удалить. Рядом с `HealthcheckConfig` добавить:

```rust
/// Per-stage deploy timeouts (§8.1). Agent-wide defaults live in agent.toml.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageTimeouts {
    pub fetch_secs: u64,
    pub build_secs: u64,
    pub up_secs: u64,
}

impl Default for StageTimeouts {
    fn default() -> StageTimeouts {
        StageTimeouts {
            fetch_secs: 120,
            build_secs: 1800,
            up_secs: 300,
        }
    }
}

impl StageTimeouts {
    /// Project overrides from [timeouts] in pi.toml win over agent defaults (§12).
    pub fn with_overrides(&self, overrides: &StageTimeoutOverrides) -> StageTimeouts {
        StageTimeouts {
            fetch_secs: overrides.fetch_secs.unwrap_or(self.fetch_secs),
            build_secs: overrides.build_secs.unwrap_or(self.build_secs),
            up_secs: overrides.up_secs.unwrap_or(self.up_secs),
        }
    }
}

/// Optional per-project overrides ([timeouts] in pi.toml, §12).
/// Travels with ProjectConfig like HealthcheckConfig; not persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StageTimeoutOverrides {
    pub fetch_secs: Option<u64>,
    pub build_secs: Option<u64>,
    pub up_secs: Option<u64>,
}
```

В `ProjectConfig` добавить поле (после `healthcheck`):

```rust
    /// Stage timeout overrides ([timeouts] from pi.toml). Not persisted in DB.
    pub timeouts: StageTimeoutOverrides,
```

- [ ] **Step 4: Новые ошибки**

В `crates/domain/src/error.rs` добавить варианты (НЕ удалять `DeployInProgress` — он живёт до Task 8):

```rust
    #[error("deployment canceled")]
    Canceled,
    #[error("timeout: {stage} after {secs}s")]
    Timeout { stage: String, secs: u64 },
    #[error("conflict: {0}")]
    Conflict(String),
```

- [ ] **Step 5: Починить компил-фоллаут — литералы `ProjectConfig`**

Во все литералы `ProjectConfig { … }` добавить `timeouts: StageTimeoutOverrides::default(),` (+импорт `StageTimeoutOverrides`):
- `crates/application/src/deploy.rs` — `sample_config()`;
- `crates/application/src/env.rs` — `registered()`;
- `crates/infrastructure/src/git.rs` — `cfg()` в `mod integration`;
- `crates/bin/src/proto.rs` — `impl From<ProjectDto> for ProjectConfig` (поле `timeouts: StageTimeoutOverrides::default()` — DTO-маппинг придёт в Task 2);
- `crates/bin/src/cli/pitoml.rs` — `to_project_config()` (то же временное значение).

- [ ] **Step 6: Прогнать тесты воркспейса**

Run: `rtk cargo test`
Expected: PASS (все крейты).

- [ ] **Step 7: Commit**

```bash
rtk git add -A && rtk git commit -m "feat(domain): deployment statuses, stage timeouts and errors for v0.3

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

### Task 2: Конфиг-поверхности — agent.toml (timeouts/gc/retention/семафор) и pi.toml `[timeouts]`

**Files:**
- Create: `crates/bin/src/duration.rs`
- Modify: `crates/bin/src/main.rs` (только `mod duration;`)
- Modify: `crates/bin/src/cli/pitoml.rs`
- Modify: `crates/bin/src/agent/config.rs`
- Modify: `crates/bin/src/proto.rs`
- Modify: `dev/agent.toml`

- [ ] **Step 1: Перенести `parse_duration_secs` в общий модуль**

Создать `crates/bin/src/duration.rs` — функция и её тест переезжают из `pitoml.rs` без изменений логики:

```rust
/// "60s" | "2m" | bare seconds -> seconds. Shared by pi.toml and agent.toml.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_secs_supports_s_m_and_bare_numbers() {
        assert_eq!(parse_duration_secs("60s").unwrap(), 60);
        assert_eq!(parse_duration_secs("2m").unwrap(), 120);
        assert_eq!(parse_duration_secs("90").unwrap(), 90);
        assert!(parse_duration_secs("soon").is_err());
    }
}
```

В `main.rs` добавить `mod duration;` (рядом с `mod agent;`). В `pitoml.rs`: удалить функцию и её тест, добавить `use crate::duration::parse_duration_secs;`.

Run: `rtk cargo test -p pi` → PASS.

- [ ] **Step 2: Падающие тесты agent.toml**

В `mod tests` файла `crates/bin/src/agent/config.rs` добавить:

```rust
    #[test]
    fn v03_defaults_for_resilience_options() {
        let config = AgentConfig::parse("").unwrap();
        assert_eq!(config.build_concurrency, 1, "build semaphore size (§8.1)");
        assert_eq!(config.history_keep, 50, "deployments kept per project (§18)");
        assert_eq!(config.gc.disk_threshold_percent, 85, "§8.1");
        let t = config.stage_timeouts().unwrap();
        assert_eq!((t.fetch_secs, t.build_secs, t.up_secs), (120, 1800, 300));
    }

    #[test]
    fn timeouts_section_overrides_defaults_and_is_validated() {
        let config =
            AgentConfig::parse("[timeouts]\nfetch = \"5m\"\nbuild = \"45m\"\nup = \"90s\"")
                .unwrap();
        let t = config.stage_timeouts().unwrap();
        assert_eq!((t.fetch_secs, t.build_secs, t.up_secs), (300, 2700, 90));
        assert!(
            AgentConfig::parse("[timeouts]\nbuild = \"soon\"").is_err(),
            "bad duration must fail at load"
        );
    }

    #[test]
    fn gc_and_concurrency_sections_parse() {
        let config = AgentConfig::parse(
            "build_concurrency = 2\nhistory_keep = 10\n[gc]\ndisk_threshold_percent = 90",
        )
        .unwrap();
        assert_eq!(config.build_concurrency, 2);
        assert_eq!(config.history_keep, 10);
        assert_eq!(config.gc.disk_threshold_percent, 90);
    }
```

Run: `rtk cargo test -p pi agent::config` → FAIL (полей нет).

- [ ] **Step 3: Реализовать секции AgentConfig**

В `crates/bin/src/agent/config.rs`: импорты `use pi_domain::entities::StageTimeouts;` и `use crate::duration::parse_duration_secs;`. В struct `AgentConfig` добавить поля:

```rust
    #[serde(default = "default_build_concurrency")]
    pub build_concurrency: usize,
    #[serde(default = "default_history_keep")]
    pub history_keep: usize,
    #[serde(default)]
    pub timeouts: TimeoutsSection,
    #[serde(default)]
    pub gc: GcSection,
```

И типы/дефолты:

```rust
/// [timeouts] in agent.toml — agent-wide stage timeout defaults (§8.1).
#[derive(Debug, Default, Deserialize)]
pub struct TimeoutsSection {
    pub fetch: Option<String>,
    pub build: Option<String>,
    pub up: Option<String>,
}

/// [gc] in agent.toml (§8.1).
#[derive(Debug, Deserialize)]
pub struct GcSection {
    #[serde(default = "default_disk_threshold")]
    pub disk_threshold_percent: u8,
}

impl Default for GcSection {
    fn default() -> GcSection {
        GcSection {
            disk_threshold_percent: default_disk_threshold(),
        }
    }
}

fn default_build_concurrency() -> usize {
    1
}
fn default_history_keep() -> usize {
    50
}
fn default_disk_threshold() -> u8 {
    85
}
```

В `impl AgentConfig` добавить метод:

```rust
    /// Stage timeout defaults: spec values overridden by [timeouts] (§8.1).
    pub fn stage_timeouts(&self) -> anyhow::Result<StageTimeouts> {
        let mut t = StageTimeouts::default();
        let parse = |field: &str, value: &Option<String>| -> anyhow::Result<Option<u64>> {
            match value {
                Some(s) => parse_duration_secs(s)
                    .map(Some)
                    .map_err(|e| anyhow::anyhow!("agent.toml [timeouts].{field}: {e}")),
                None => Ok(None),
            }
        };
        if let Some(secs) = parse("fetch", &self.timeouts.fetch)? {
            t.fetch_secs = secs;
        }
        if let Some(secs) = parse("build", &self.timeouts.build)? {
            t.build_secs = secs;
        }
        if let Some(secs) = parse("up", &self.timeouts.up)? {
            t.up_secs = secs;
        }
        Ok(t)
    }
```

В `parse()` валидировать при загрузке (плохой duration падает сразу):

```rust
    pub fn parse(text: &str) -> anyhow::Result<AgentConfig> {
        let config: AgentConfig = toml::from_str(text)?;
        config.stage_timeouts()?;
        Ok(config)
    }
```

Run: `rtk cargo test -p pi agent::config` → PASS.

- [ ] **Step 4: Падающие тесты pi.toml `[timeouts]` и DTO**

В `mod tests` файла `crates/bin/src/cli/pitoml.rs`:

```rust
    #[test]
    fn timeouts_section_maps_to_overrides_and_is_validated() {
        let toml = SAMPLE.replace(
            "[healthcheck]",
            "[timeouts]\nfetch = \"3m\"\nup = \"120s\"\n\n[healthcheck]",
        );
        let config = PiToml::parse(&toml).unwrap().to_project_config();
        assert_eq!(config.timeouts.fetch_secs, Some(180));
        assert_eq!(config.timeouts.build_secs, None, "not set -> agent default");
        assert_eq!(config.timeouts.up_secs, Some(120));

        let bad =
            SAMPLE.replace("[healthcheck]", "[timeouts]\nbuild = \"soon\"\n\n[healthcheck]");
        assert!(PiToml::parse(&bad).is_err());
    }

    #[test]
    fn missing_timeouts_section_means_no_overrides() {
        let config = PiToml::parse(SAMPLE).unwrap().to_project_config();
        assert_eq!(config.timeouts, Default::default());
    }
```

В `mod tests` файла `crates/bin/src/proto.rs`:

```rust
    #[test]
    fn timeouts_roundtrip_through_dto_and_default_when_absent() {
        let json = r#"{"project":{"name":"a","repo":"r","branch":"main","compose":"docker-compose.yml","service":"web","port":3000,"hostname":null},"ref":null}"#;
        let req: DeployRequest = serde_json::from_str(json).unwrap();
        let config: ProjectConfig = req.project.into();
        assert_eq!(config.timeouts, Default::default(), "v0.2 payloads still work");

        let mut config = config;
        config.timeouts.build_secs = Some(3600);
        let dto = ProjectDto::from(&config);
        let back: ProjectConfig = dto.into();
        assert_eq!(back.timeouts.build_secs, Some(3600));
    }
```

Run: `rtk cargo test -p pi` → FAIL.

- [ ] **Step 5: Реализовать `[timeouts]` в PiToml и TimeoutsDto**

`crates/bin/src/cli/pitoml.rs` — в struct `PiToml` добавить:

```rust
    #[serde(default)]
    pub timeouts: TimeoutsSection,
```

И секцию:

```rust
/// [timeouts] in pi.toml — per-project stage overrides (§12, §8.1).
#[derive(Debug, Default, Deserialize)]
pub struct TimeoutsSection {
    pub fetch: Option<String>,
    pub build: Option<String>,
    pub up: Option<String>,
}
```

В `PiToml::parse` (рядом с валидацией healthcheck) добавить:

```rust
        for (field, value) in [
            ("fetch", &parsed.timeouts.fetch),
            ("build", &parsed.timeouts.build),
            ("up", &parsed.timeouts.up),
        ] {
            if let Some(timeout) = value {
                parse_duration_secs(timeout)
                    .map_err(|e| anyhow::anyhow!("pi.toml [timeouts].{field}: {e}"))?;
            }
        }
```

В `to_project_config()` заменить заглушку из Task 1 (импорт `StageTimeoutOverrides` из `pi_domain::entities`):

```rust
            timeouts: StageTimeoutOverrides {
                fetch_secs: self
                    .timeouts
                    .fetch
                    .as_deref()
                    .and_then(|t| parse_duration_secs(t).ok()),
                build_secs: self
                    .timeouts
                    .build
                    .as_deref()
                    .and_then(|t| parse_duration_secs(t).ok()),
                up_secs: self
                    .timeouts
                    .up
                    .as_deref()
                    .and_then(|t| parse_duration_secs(t).ok()),
            },
```

`crates/bin/src/proto.rs` — в `ProjectDto` добавить поле:

```rust
    #[serde(default)]
    pub timeouts: Option<TimeoutsDto>,
```

Тип:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutsDto {
    pub fetch_secs: Option<u64>,
    pub build_secs: Option<u64>,
    pub up_secs: Option<u64>,
}
```

В `From<ProjectDto> for ProjectConfig` заменить заглушку из Task 1 (импорт `StageTimeoutOverrides`):

```rust
            timeouts: dto
                .timeouts
                .map(|t| StageTimeoutOverrides {
                    fetch_secs: t.fetch_secs,
                    build_secs: t.build_secs,
                    up_secs: t.up_secs,
                })
                .unwrap_or_default(),
```

В `From<&ProjectConfig> for ProjectDto`:

```rust
            timeouts: Some(TimeoutsDto {
                fetch_secs: config.timeouts.fetch_secs,
                build_secs: config.timeouts.build_secs,
                up_secs: config.timeouts.up_secs,
            }),
```

В тестовых литералах `ProjectDto { … }` (proto.rs) добавить `timeouts: None,`.

- [ ] **Step 6: Обновить `dev/agent.toml`**

Добавить в конец:

```toml
# build_concurrency = 1          # global build semaphore size (§8.1)
# history_keep = 50              # deployments kept per project (§18)

# [timeouts]                     # agent-wide stage timeout defaults (§8.1)
# fetch = "2m"
# build = "30m"
# up = "5m"

# [gc]
# disk_threshold_percent = 85    # builder prune above this disk usage (§8.1)
```

- [ ] **Step 7: Прогнать тесты и закоммитить**

Run: `rtk cargo test -p pi`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(bin): agent.toml and pi.toml options for timeouts, gc, retention, build concurrency"
```

(трейлер `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>` — как в конвенциях, добавлять во все коммиты.)

---

### Task 3: `process.rs` — kill_on_drop (таймаут/отмена убивает дочерний процесс)

**Files:**
- Modify: `crates/infrastructure/src/process.rs`

- [ ] **Step 1: Smoke-тест**

В `mod tests` файла `crates/infrastructure/src/process.rs` добавить:

```rust
    #[tokio::test]
    async fn dropping_run_streamed_future_kills_the_child() {
        // Long-running cross-platform child process.
        let mut cmd;
        #[cfg(windows)]
        {
            cmd = tokio::process::Command::new("ping");
            cmd.args(["-n", "30", "127.0.0.1"]);
        }
        #[cfg(not(windows))]
        {
            cmd = tokio::process::Command::new("sleep");
            cmd.arg("30");
        }

        let sink = Arc::new(VecSink(Mutex::new(vec![])));
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            run_streamed(cmd, sink),
        )
        .await;
        assert!(result.is_err(), "child must outlive the timeout");
        // kill_on_drop: the dropped future kills the child. The observable
        // effect is the test (and the test process exit) not hanging ~30s.
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
```

Run: `rtk cargo test -p pi-infrastructure process` → тест пройдёт и до фикса (timeout сам по себе вернёт Err) — это smoke против регрессий. Сама правка поведенческая: без `kill_on_drop` осиротевший `docker build`/`git fetch` жил бы после таймаута стадии (§8.1 «убивает дочерний процесс»).

- [ ] **Step 2: Реализовать**

В `run_streamed` после `cmd.stdout(...).stderr(...).stdin(Stdio::null());` добавить:

```rust
    // §8.1: a timed-out or canceled stage must not leave the child running —
    // dropping this future (tokio::time::timeout / select!) kills the process.
    cmd.kill_on_drop(true);
```

В `run_capture` после `cmd.stdin(Stdio::null());` добавить `cmd.kill_on_drop(true);`.

- [ ] **Step 3: Прогнать тесты и закоммитить**

Run: `rtk cargo test -p pi-infrastructure`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "fix(infra): kill child processes when a deploy stage future is dropped"
```

---

### Task 4: `DeploymentHistory` — record_queued / mark_running / active / sweep + ретеншен

**Files:**
- Modify: `crates/domain/src/contracts.rs`
- Modify: `crates/infrastructure/src/history.rs`
- Modify (механический фоллаут): `crates/application/src/deploy.rs`, `crates/bin/src/agent/state.rs`, `crates/bin/src/agent/http.rs` (тестовый `state_with`)

- [ ] **Step 1: Обновить контракт**

В `crates/domain/src/contracts.rs` заменить трейт `DeploymentHistory` на:

```rust
/// Deployment history (§6, §18).
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait DeploymentHistory: Send + Sync {
    /// INSERT the deployment row (normally status Queued). The adapter prunes
    /// old terminal rows of the project beyond its retention right after (§18).
    async fn record_queued(&self, deployment: &Deployment) -> Result<(), DomainError>;
    /// Queued -> Running; refreshes started_at to the actual start moment.
    async fn mark_running(&self, id: &str, started_at: i64) -> Result<(), DomainError>;
    async fn record_finished<'a>(
        &self,
        id: &str,
        status: DeploymentStatus,
        commit_sha: Option<&'a str>,
        finished_at: i64,
        log_tail: &str,
    ) -> Result<(), DomainError>;
    async fn get(&self, id: &str) -> Result<Option<Deployment>, DomainError>;
    /// Non-terminal deployments of a project (queued/running), newest first.
    async fn active(&self, project: &str) -> Result<Vec<Deployment>, DomainError>;
    /// Crash-recovery sweep at agent start (§8.1): queued/running -> interrupted.
    /// Returns the number of rows swept.
    async fn sweep_interrupted(&self, finished_at: i64) -> Result<u64, DomainError>;
}
```

- [ ] **Step 2: Падающие тесты SqliteHistory**

В `mod tests` файла `crates/infrastructure/src/history.rs`: конструктор в хелпере получает keep (`history(&dir)` → `SqliteHistory::new(Db::open(..).unwrap(), 50)`), добавить тесты:

```rust
    fn queued(id: &str, started_at: i64) -> Deployment {
        Deployment {
            id: id.into(),
            project: "rateme".into(),
            git_ref: "main".into(),
            commit_sha: None,
            status: DeploymentStatus::Queued,
            started_at,
            finished_at: None,
            log_tail: String::new(),
        }
    }

    #[tokio::test]
    async fn queued_then_mark_running_updates_status_and_started_at() {
        let dir = tempfile::tempdir().unwrap();
        let h = history(&dir);
        h.record_queued(&queued("d1", 100)).await.unwrap();
        assert_eq!(
            h.get("d1").await.unwrap().unwrap().status,
            DeploymentStatus::Queued
        );

        h.mark_running("d1", 150).await.unwrap();
        let d = h.get("d1").await.unwrap().unwrap();
        assert_eq!(d.status, DeploymentStatus::Running);
        assert_eq!(d.started_at, 150, "started_at refreshed to actual start");
    }

    #[tokio::test]
    async fn mark_running_missing_or_not_queued_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let h = history(&dir);
        assert!(matches!(
            h.mark_running("missing", 1).await.unwrap_err(),
            DomainError::NotFound(_)
        ));
        h.record_queued(&queued("d1", 100)).await.unwrap();
        h.mark_running("d1", 150).await.unwrap();
        assert!(
            matches!(
                h.mark_running("d1", 160).await.unwrap_err(),
                DomainError::NotFound(_)
            ),
            "already running -> cannot mark again"
        );
    }

    #[tokio::test]
    async fn active_returns_queued_and_running_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let h = history(&dir);
        h.record_queued(&queued("d1", 100)).await.unwrap();
        h.mark_running("d1", 110).await.unwrap();
        h.record_queued(&queued("d2", 120)).await.unwrap();
        h.record_queued(&queued("d3", 90)).await.unwrap();
        h.record_finished("d3", DeploymentStatus::Failed, None, 95, "")
            .await
            .unwrap();

        let active = h.active("rateme").await.unwrap();
        let ids: Vec<&str> = active.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, vec!["d2", "d1"], "newest first, terminal excluded");
        assert!(h.active("ghost").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn sweep_marks_queued_and_running_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        let h = history(&dir);
        h.record_queued(&queued("d1", 100)).await.unwrap();
        h.mark_running("d1", 110).await.unwrap();
        h.record_queued(&queued("d2", 120)).await.unwrap();
        h.record_queued(&queued("d3", 90)).await.unwrap();
        h.record_finished("d3", DeploymentStatus::Success, Some("abc"), 95, "")
            .await
            .unwrap();

        let swept = h.sweep_interrupted(200).await.unwrap();
        assert_eq!(swept, 2);
        for id in ["d1", "d2"] {
            let d = h.get(id).await.unwrap().unwrap();
            assert_eq!(d.status, DeploymentStatus::Interrupted);
            assert_eq!(d.finished_at, Some(200));
        }
        assert_eq!(
            h.get("d3").await.unwrap().unwrap().status,
            DeploymentStatus::Success,
            "terminal rows untouched"
        );
        assert_eq!(h.sweep_interrupted(300).await.unwrap(), 0, "idempotent");
    }

    #[tokio::test]
    async fn retention_prunes_old_terminal_rows_but_never_active_ones() {
        let dir = tempfile::tempdir().unwrap();
        let h = SqliteHistory::new(Db::open(&dir.path().join("state.db")).unwrap(), 2);
        // d1 (oldest, terminal), d2 (running), d3/d4 (terminal, newest)
        for (id, at) in [("d1", 10), ("d2", 20), ("d3", 30)] {
            h.record_queued(&queued(id, at)).await.unwrap();
        }
        h.record_finished("d1", DeploymentStatus::Success, None, 11, "")
            .await
            .unwrap();
        h.mark_running("d2", 21).await.unwrap();
        h.record_finished("d3", DeploymentStatus::Failed, None, 31, "")
            .await
            .unwrap();

        // the insert that triggers pruning (keep = 2 newest by started_at)
        h.record_queued(&queued("d4", 40)).await.unwrap();

        assert!(h.get("d1").await.unwrap().is_none(), "old terminal pruned");
        assert!(
            h.get("d2").await.unwrap().is_some(),
            "running row survives retention even though it is old"
        );
        assert!(h.get("d3").await.unwrap().is_some());
        assert!(h.get("d4").await.unwrap().is_some());
    }
```

Run: `rtk cargo test -p pi-infrastructure history` → FAIL (методов нет).

- [ ] **Step 3: Реализовать SqliteHistory**

`crates/infrastructure/src/history.rs`:

```rust
pub struct SqliteHistory {
    db: Db,
    keep_per_project: usize,
}

impl SqliteHistory {
    /// keep_per_project — retention (§18): newest N rows per project survive,
    /// older terminal rows are deleted after each insert.
    pub fn new(db: Db, keep_per_project: usize) -> Arc<SqliteHistory> {
        Arc::new(SqliteHistory {
            db,
            keep_per_project,
        })
    }
}
```

`record_started` переименовать в `record_queued`; после INSERT — чистка в той же `db.call`:

```rust
    async fn record_queued(&self, deployment: &Deployment) -> Result<(), DomainError> {
        let d = deployment.clone();
        let keep = self.keep_per_project;
        self.db
            .call(move |conn| {
                conn.execute(
                    "INSERT INTO deployments (id, project, git_ref, commit_sha, status, started_at, finished_at, log_tail)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        d.id,
                        d.project,
                        d.git_ref,
                        d.commit_sha,
                        d.status.as_str(),
                        d.started_at,
                        d.finished_at,
                        d.log_tail
                    ],
                )
                .map_err(storage_err)?;
                // §18: retention after each insert; active rows never pruned.
                conn.execute(
                    "DELETE FROM deployments
                     WHERE project = ?1
                       AND status NOT IN ('queued', 'running')
                       AND id NOT IN (
                           SELECT id FROM deployments
                           WHERE project = ?1
                           ORDER BY started_at DESC, id DESC
                           LIMIT ?2
                       )",
                    params![d.project, keep as i64],
                )
                .map_err(storage_err)?;
                Ok(())
            })
            .await
    }

    async fn mark_running(&self, id: &str, started_at: i64) -> Result<(), DomainError> {
        let (id, id_for_error) = (id.to_string(), id.to_string());
        self.db
            .call(move |conn| {
                let rows = conn
                    .execute(
                        "UPDATE deployments SET status='running', started_at=?2
                         WHERE id=?1 AND status='queued'",
                        params![id, started_at],
                    )
                    .map_err(storage_err)?;
                if rows == 0 {
                    return Err(DomainError::NotFound(format!(
                        "queued deployment {id_for_error}"
                    )));
                }
                Ok(())
            })
            .await
    }

    async fn active(&self, project: &str) -> Result<Vec<Deployment>, DomainError> {
        let project = project.to_string();
        self.db
            .call(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, project, git_ref, commit_sha, status, started_at, finished_at, log_tail
                         FROM deployments
                         WHERE project = ?1 AND status IN ('queued', 'running')
                         ORDER BY started_at DESC, id DESC",
                    )
                    .map_err(storage_err)?;
                let rows = stmt
                    .query_map(params![project], row_to_deployment)
                    .map_err(storage_err)?;
                rows.collect::<Result<Vec<_>, _>>().map_err(storage_err)
            })
            .await
    }

    async fn sweep_interrupted(&self, finished_at: i64) -> Result<u64, DomainError> {
        self.db
            .call(move |conn| {
                let rows = conn
                    .execute(
                        "UPDATE deployments SET status='interrupted', finished_at=?1
                         WHERE status IN ('queued', 'running')",
                        params![finished_at],
                    )
                    .map_err(storage_err)?;
                Ok(rows as u64)
            })
            .await
    }
```

- [ ] **Step 4: Механический фоллаут**

- `crates/application/src/deploy.rs`: `self.history.record_started(&deployment)` → `self.history.record_queued(&deployment)`; в тестах `expect_record_started` → `expect_record_queued` (поведение пока прежнее — статус Running при вставке; полный жизненный цикл queued→running придёт в Task 7).
- `crates/bin/src/agent/state.rs`: `SqliteHistory::new(db)` → `SqliteHistory::new(db, config.history_keep)`.
- `crates/bin/src/agent/http.rs` (тест `state_with`): `SqliteHistory::new(db.clone())` → `SqliteHistory::new(db.clone(), 50)`.
- Проверить остатки: `rtk grep record_started` — должно быть пусто (комментарий в http.rs про «record_started hasn't committed» переписать на `record_queued`).

- [ ] **Step 5: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(infra): history queue lifecycle, active list, startup sweep and retention"
```

---

### Task 5: `ContainerRuntime` — prune_images / prune_builder

**Files:**
- Modify: `crates/domain/src/contracts.rs`
- Modify: `crates/infrastructure/src/docker.rs`

- [ ] **Step 1: Контракт**

В трейт `ContainerRuntime` (contracts.rs) добавить:

```rust
    /// `docker image prune -f` — dangling images only; build cache stays (§8.1).
    async fn prune_images(&self, log: Arc<dyn LogSink>) -> Result<(), DomainError>;
    /// `docker builder prune -f` with an age filter — frees build cache when
    /// the disk crosses the GC threshold (§8.1).
    async fn prune_builder(&self, log: Arc<dyn LogSink>) -> Result<(), DomainError>;
```

- [ ] **Step 2: Падающие тесты арг-билдеров**

В `mod tests` файла `crates/infrastructure/src/docker.rs`:

```rust
    #[test]
    fn prune_args_shapes() {
        assert_eq!(prune_images_args(), vec!["image", "prune", "-f"]);
        assert_eq!(
            prune_builder_args(),
            vec![
                "builder".to_string(),
                "prune".to_string(),
                "-f".to_string(),
                "--filter".to_string(),
                format!("until={BUILDER_PRUNE_MAX_AGE}"),
            ]
        );
    }
```

Run: `rtk cargo test -p pi-infrastructure docker` → FAIL.

- [ ] **Step 3: Реализовать**

В `crates/infrastructure/src/docker.rs`:

```rust
/// Age filter for `docker builder prune` (§8.1): recent cache survives so
/// rebuilds stay fast; only the disk threshold is configurable (§22).
pub(crate) const BUILDER_PRUNE_MAX_AGE: &str = "24h";

pub(crate) fn prune_images_args() -> Vec<&'static str> {
    vec!["image", "prune", "-f"]
}

pub(crate) fn prune_builder_args() -> Vec<String> {
    vec![
        "builder".to_string(),
        "prune".to_string(),
        "-f".to_string(),
        "--filter".to_string(),
        format!("until={BUILDER_PRUNE_MAX_AGE}"),
    ]
}
```

В `impl ContainerRuntime for DockerComposeRuntime`:

```rust
    async fn prune_images(&self, log: Arc<dyn LogSink>) -> Result<(), DomainError> {
        log.line("docker image prune -f ...");
        let mut cmd = Command::new("docker");
        cmd.args(prune_images_args());
        run_streamed(cmd, log).await.map_err(DomainError::Runtime)
    }

    async fn prune_builder(&self, log: Arc<dyn LogSink>) -> Result<(), DomainError> {
        log.line(&format!(
            "docker builder prune -f --filter until={BUILDER_PRUNE_MAX_AGE} ..."
        ));
        let mut cmd = Command::new("docker");
        cmd.args(prune_builder_args());
        run_streamed(cmd, log).await.map_err(DomainError::Runtime)
    }
```

`MockContainerRuntime` получает методы автоматически (automock). Компил-фоллаута в других крейтах нет — у моков методы опциональны, пока их не зовут.

- [ ] **Step 4: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(infra): docker image and builder prune in ContainerRuntime"
```

---

### Task 6: `DiskProbe` + use-case `RunGc`

**Files:**
- Modify: `crates/domain/src/contracts.rs`
- Create: `crates/infrastructure/src/disk.rs`
- Modify: `crates/infrastructure/src/lib.rs`
- Create: `crates/application/src/gc.rs`
- Modify: `crates/application/src/lib.rs`
- Modify: `Cargo.toml` (workspace), `crates/infrastructure/Cargo.toml`

- [ ] **Step 1: Контракт DiskProbe**

В `crates/domain/src/contracts.rs` добавить:

```rust
/// Disk fill probe for the GC threshold decision (§8.1). v1 — sysinfo.
#[cfg_attr(feature = "mocks", automock)]
pub trait DiskProbe: Send + Sync {
    /// Used space of the filesystem holding the agent data dir, percent 0..=100.
    fn used_percent(&self) -> Result<u8, DomainError>;
}
```

- [ ] **Step 2: Падающие тесты RunGc**

Создать `crates/application/src/gc.rs` сразу с тестами (реализация — следующий шаг; чтобы тест «упал», можно начать с файла, где есть только `mod tests`, и убедиться, что не компилируется):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CollectSink;
    use pi_domain::contracts::{MockContainerRuntime, MockDiskProbe};
    use std::sync::Arc;

    #[tokio::test]
    async fn below_threshold_prunes_images_only() {
        let mut runtime = MockContainerRuntime::new();
        runtime
            .expect_prune_images()
            .times(1)
            .returning(|_| Ok(()));
        runtime.expect_prune_builder().times(0);
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(60));

        let report = RunGc::new(Arc::new(runtime), Arc::new(disk), 85)
            .execute(CollectSink::new())
            .await
            .unwrap();
        assert_eq!(
            report,
            GcReport {
                disk_used_percent: 60,
                builder_pruned: false
            }
        );
    }

    #[tokio::test]
    async fn at_or_above_threshold_also_prunes_builder() {
        let mut runtime = MockContainerRuntime::new();
        runtime
            .expect_prune_images()
            .times(1)
            .returning(|_| Ok(()));
        runtime
            .expect_prune_builder()
            .times(1)
            .returning(|_| Ok(()));
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(85));

        let sink = CollectSink::new();
        let report = RunGc::new(Arc::new(runtime), Arc::new(disk), 85)
            .execute(sink.clone())
            .await
            .unwrap();
        assert!(report.builder_pruned);
        let lines = sink.lines.lock().unwrap();
        assert!(
            lines.iter().any(|l| l.contains("85%")),
            "threshold decision must be logged: {lines:?}"
        );
    }

    #[tokio::test]
    async fn prune_images_error_propagates() {
        let mut runtime = MockContainerRuntime::new();
        runtime
            .expect_prune_images()
            .returning(|_| Err(pi_domain::error::DomainError::Runtime("docker down".into())));
        runtime.expect_prune_builder().times(0);
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().times(0);

        let err = RunGc::new(Arc::new(runtime), Arc::new(disk), 85)
            .execute(CollectSink::new())
            .await
            .unwrap_err();
        assert!(matches!(err, pi_domain::error::DomainError::Runtime(_)));
    }
}
```

В `crates/application/src/lib.rs` добавить `pub mod gc;`. `test_support` сейчас под `#[cfg(test)]` — он уже доступен тестам.

Run: `rtk cargo test -p pi-application gc` → FAIL (нет `RunGc`).

- [ ] **Step 3: Реализовать RunGc**

В начало `crates/application/src/gc.rs` (над `mod tests`):

```rust
use std::sync::Arc;

use pi_domain::contracts::{ContainerRuntime, DiskProbe, LogSink};
use pi_domain::error::DomainError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcReport {
    pub disk_used_percent: u8,
    pub builder_pruned: bool,
}

/// Disk GC (§8.1): dangling images always; build cache only above the disk
/// threshold. Runs as a post-success deploy stage and behind `pi gc`.
pub struct RunGc {
    runtime: Arc<dyn ContainerRuntime>,
    disk: Arc<dyn DiskProbe>,
    disk_threshold_percent: u8,
}

impl RunGc {
    pub fn new(
        runtime: Arc<dyn ContainerRuntime>,
        disk: Arc<dyn DiskProbe>,
        disk_threshold_percent: u8,
    ) -> Arc<RunGc> {
        Arc::new(RunGc {
            runtime,
            disk,
            disk_threshold_percent,
        })
    }

    pub async fn execute(&self, log: Arc<dyn LogSink>) -> Result<GcReport, DomainError> {
        self.runtime.prune_images(Arc::clone(&log)).await?;
        let used = self.disk.used_percent()?;
        let builder_pruned = used >= self.disk_threshold_percent;
        if builder_pruned {
            log.line(&format!(
                "disk {used}% >= {}% threshold - pruning build cache",
                self.disk_threshold_percent
            ));
            self.runtime.prune_builder(Arc::clone(&log)).await?;
        } else {
            log.line(&format!(
                "disk {used}% < {}% threshold - keeping build cache",
                self.disk_threshold_percent
            ));
        }
        Ok(GcReport {
            disk_used_percent: used,
            builder_pruned,
        })
    }
}
```

Run: `rtk cargo test -p pi-application gc` → PASS.

- [ ] **Step 4: SysinfoDiskProbe**

В workspace `Cargo.toml` добавить `sysinfo = "0.33"` в `[workspace.dependencies]`; в `crates/infrastructure/Cargo.toml` — `sysinfo = { workspace = true }`.

Создать `crates/infrastructure/src/disk.rs`:

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pi_domain::contracts::DiskProbe;
use pi_domain::error::DomainError;
use sysinfo::Disks;

/// Used-space probe for the filesystem holding the agent data dir (§8.1).
pub struct SysinfoDiskProbe {
    path: PathBuf,
}

impl SysinfoDiskProbe {
    pub fn new(path: &Path) -> Arc<SysinfoDiskProbe> {
        Arc::new(SysinfoDiskProbe {
            path: path.to_path_buf(),
        })
    }
}

impl DiskProbe for SysinfoDiskProbe {
    fn used_percent(&self) -> Result<u8, DomainError> {
        // canonicalize so relative dev paths (./.dev-data) match mount points
        let path = std::fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone());
        let disks = Disks::new_with_refreshed_list();
        let disk = disks
            .list()
            .iter()
            .filter(|d| path.starts_with(d.mount_point()))
            .max_by_key(|d| d.mount_point().as_os_str().len())
            .ok_or_else(|| {
                DomainError::Storage(format!("no disk found for {}", path.display()))
            })?;
        let total = disk.total_space();
        if total == 0 {
            return Err(DomainError::Storage("disk reports zero total space".into()));
        }
        let used = total.saturating_sub(disk.available_space());
        Ok(((used * 100) / total) as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn used_percent_of_current_dir_is_a_sane_percentage() {
        let probe = SysinfoDiskProbe::new(Path::new("."));
        let used = probe.used_percent().unwrap();
        assert!(used <= 100, "got {used}");
    }
}
```

В `crates/infrastructure/src/lib.rs` добавить `pub mod disk;` (по алфавиту — после `pub mod cloudflared;`... фактически между `dotenv` и `docker` по списку: вставить `pub mod disk;` после `pub mod cloudflared;`).

- [ ] **Step 5: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(app): RunGc use-case with sysinfo disk probe"
```

---

### Task 7: `DeployProject` — отмена, поэтапные таймауты, build-семафор, GC-стадия

**Files:**
- Modify: `crates/application/Cargo.toml`, `Cargo.toml` (workspace: +`tokio-util`)
- Modify: `crates/application/src/deploy.rs`
- Modify: `crates/bin/Cargo.toml` (+`tokio-util`)
- Modify: `crates/bin/src/agent/http.rs`, `crates/bin/src/agent/state.rs`

- [ ] **Step 1: Зависимости**

Workspace `Cargo.toml`: в `[workspace.dependencies]` добавить `tokio-util = "0.7"`.
`crates/application/Cargo.toml` — `[dependencies]`: добавить `tokio = { workspace = true }` и `tokio-util = { workspace = true }` (tokio из dev-deps убрать — он теперь обычная зависимость).
`crates/bin/Cargo.toml` — `[dependencies]`: добавить `tokio-util = { workspace = true }`.

- [ ] **Step 2: Новая структура DeployProject**

В `crates/application/src/deploy.rs`:

Импорты: добавить

```rust
use pi_domain::entities::StageTimeouts;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::gc::RunGc;
```

Поляструктуры (после `clock`):

```rust
    gc: Arc<RunGc>,
    timeouts: StageTimeouts,
    /// §8.1: global build semaphore — parallel builds OOM the Pi.
    build_sem: Semaphore,
    locks: Arc<DeployLocks>,
```

Конструктор: добавить параметры `gc: Arc<RunGc>, timeouts: StageTimeouts, build_slots: usize` (последними) и в литерал — `gc, timeouts, build_sem: Semaphore::new(build_slots),`.

Хелпер таймаута (свободная функция над `impl DeployProject`):

```rust
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
```

- [ ] **Step 3: Новый `execute` (queued→running, select на отмену)**

Заменить тело `execute` (сигнатура: `permit` остаётся до Task 8, добавляется `cancel`):

```rust
    pub async fn execute(
        &self,
        permit: DeployPermit,
        deployment_id: String,
        config: ProjectConfig,
        git_ref: DeployRef,
        sink: Arc<dyn LogSink>,
        cancel: CancellationToken,
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

        let started_at = self.clock.now_unix();
        self.history.mark_running(&deployment.id, started_at).await?;
        deployment.status = DeploymentStatus::Running;
        deployment.started_at = started_at;

        // §8.1: cooperative cancellation — dropping the stages future kills
        // running child processes (kill_on_drop in the process adapter).
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
```

ВАЖНО: `FinishGuard` по-прежнему шлёт `finished(Failed)` только при панике/раннем `?` — это корректный дефолт.

- [ ] **Step 4: `run_stages` — таймауты, семафор, GC**

Заменить участки `run_stages`:

```rust
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

        // §8.1: post-success GC; its failure must not fail a deployed stack
        if let Err(err) = self.gc.execute(log.clone()).await {
            log.line(&format!("gc skipped: {err}"));
        }

        Ok(fetched.commit_sha)
    }
```

- [ ] **Step 5: Обновить существующие тесты deploy.rs**

В `mod tests`:

1. Struct `Mocks` — добавить поля `pub gc_runtime: MockContainerRuntime, pub disk: MockDiskProbe` (импорт `MockDiskProbe` из `pi_domain::contracts`); в `mocks()` — дефолты:

```rust
        let mut gc_runtime = MockContainerRuntime::new();
        gc_runtime.expect_prune_images().returning(|_| Ok(()));
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(10));
```

(поля `gc_runtime`, `disk` — в литерал `Mocks`).

2. `build(m)`:

```rust
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
```

3. Все вызовы `deploy.execute(permit, …, sink)` получают последним аргументом `CancellationToken::new()` (импорт `tokio_util::sync::CancellationToken`).

4. `happy_path_runs_all_stages_and_records_success`: заменить `expect_record_started` на:

```rust
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
            .withf(|id, at| id == "dep-1" && *at == 100)
            .times(1)
            .returning(move |_, _| {
                stage_order.lock().unwrap().push("running");
                Ok(())
            });
```

и в `mocks()` для этого теста переопределить `gc_runtime` (вместо дефолта) с пушем порядка:

```rust
        m.gc_runtime.checkpoint(); // сбросить дефолт из mocks()
        let stage_order = Arc::clone(&order);
        m.gc_runtime
            .expect_prune_images()
            .times(1)
            .returning(move |_| {
                stage_order.lock().unwrap().push("gc");
                Ok(())
            });
```

Ожидаемый порядок:

```rust
        assert_eq!(
            *order.lock().unwrap(),
            vec![
                "queued", "running", "upsert", "fetch", "secrets", "override", "build", "up",
                "health", "ingress", "gc", "finished"
            ]
        );
```

5. `ok_pre_stages(&mut Mocks)` — добавить:

```rust
        m.history.expect_record_queued().returning(|_| Ok(()));
        m.history.expect_mark_running().returning(|_, _| Ok(()));
```

(и убрать прежний `expect_record_started`). В `build_failure_records_failed_and_emits_finished_failed` и `lock_released_after_execute_finishes` — то же самое точечно.

- [ ] **Step 6: Новые тесты**

Добавить в `mod tests` deploy.rs:

```rust
    /// Source whose fetch never completes — for timeout/cancel tests.
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
    async fn expired_fetch_stage_fails_with_timeout_and_stage_name() {
        let mut m = mocks();
        m.projects.expect_upsert().returning(|c| {
            Ok(Project {
                config: c.clone(),
                host_port: 8000,
                created_at: 1,
            })
        });
        m.history.expect_record_queued().returning(|_| Ok(()));
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
            fetch_secs: 0, // expires immediately
            ..StageTimeouts::default()
        };
        let deploy = build_with_source(m, Arc::new(HangingSource), timeouts);
        let sink = CollectSink::new();
        let permit = deploy.try_begin("rateme").unwrap();
        let err = deploy
            .execute(
                permit,
                "dep-t".into(),
                sample_config(),
                DeployRef::Branch("main".into()),
                sink.clone(),
                tokio_util::sync::CancellationToken::new(),
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
    async fn cancel_token_marks_deployment_canceled_and_frees_lock() {
        let mut m = mocks();
        m.projects.expect_upsert().returning(|c| {
            Ok(Project {
                config: c.clone(),
                host_port: 8000,
                created_at: 1,
            })
        });
        m.history.expect_record_queued().returning(|_| Ok(()));
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
        let permit = deploy.try_begin("rateme").unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let task = tokio::spawn({
            let deploy = Arc::clone(&deploy);
            let sink = sink.clone();
            let cancel = cancel.clone();
            async move {
                deploy
                    .execute(
                        permit,
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
        assert!(
            deploy.try_begin("rateme").is_ok(),
            "lock must be free after canceled deploy"
        );
    }

    /// ContainerRuntime that records the max number of concurrent builds.
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
        m.history.expect_record_queued().returning(|_| Ok(()));
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
            1, // §8.1: build semaphore of size 1
        );

        let mut config_a = sample_config();
        config_a.name = "a".into();
        config_a.hostname = None;
        let mut config_b = sample_config();
        config_b.name = "b".into();
        config_b.hostname = None;

        let permit_a = deploy.try_begin("a").unwrap();
        let permit_b = deploy.try_begin("b").unwrap();
        let (ra, rb) = tokio::join!(
            deploy.execute(
                permit_a,
                "dep-a".into(),
                config_a,
                DeployRef::Branch("main".into()),
                CollectSink::new(),
                tokio_util::sync::CancellationToken::new(),
            ),
            deploy.execute(
                permit_b,
                "dep-b".into(),
                config_b,
                DeployRef::Branch("main".into()),
                CollectSink::new(),
                tokio_util::sync::CancellationToken::new(),
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
        let permit = deploy.try_begin("rateme").unwrap();
        let result = deploy
            .execute(
                permit,
                "dep-gc".into(),
                sample_config(),
                DeployRef::Branch("main".into()),
                CollectSink::new(),
                tokio_util::sync::CancellationToken::new(),
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
```

- [ ] **Step 7: Обновить bin (http.rs + state.rs)**

`crates/bin/src/agent/http.rs` — в `create_deployment` спавн получает токен (хранение токена и cancel-роут придут в Task 8/9):

```rust
    tokio::spawn(async move {
        let cancel = tokio_util::sync::CancellationToken::new();
        if let Err(err) = deploy.execute(permit, id, config, git_ref, sink, cancel).await {
            tracing::warn!("deploy failed: {err}");
        }
    });
```

Тестовый `state_with`: в `ok_runtime()` добавить

```rust
        runtime.expect_prune_images().returning(|_| Ok(()));
        runtime.expect_prune_builder().returning(|_| Ok(()));
```

и в `state_with` построить gc и передать новые аргументы:

```rust
        let mut disk = pi_domain::contracts::MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(10));
        let gc = pi_application::gc::RunGc::new(Arc::clone(&runtime), Arc::new(disk), 85);
        let deploy = DeployProject::new(
            source.clone(),
            Arc::clone(&runtime),
            projects.clone(),
            Arc::clone(&history),
            overrides.clone(),
            secrets.clone(),
            FsEnvFileWriter::new(),
            health,
            DisabledIngress::new(),
            SystemClock::new(),
            gc,
            pi_domain::entities::StageTimeouts::default(),
            1,
        );
```

ВНИМАНИЕ: `GatedSource` в тесте 409 блокирует fetch → с дефолтным fetch-таймаутом 120s тест не страдает.

`crates/bin/src/agent/state.rs` — wiring:

```rust
    use pi_application::gc::RunGc;
    use pi_infrastructure::disk::SysinfoDiskProbe;

    let disk = SysinfoDiskProbe::new(&config.data_dir);
    let gc = RunGc::new(runtime.clone(), disk, config.gc.disk_threshold_percent);

    let deploy = DeployProject::new(
        source.clone(),
        runtime.clone(),
        projects.clone(),
        Arc::clone(&history),
        overrides.clone(),
        secrets.clone(),
        Arc::clone(&env_files),
        health,
        ingress,
        SystemClock::new(),
        Arc::clone(&gc),
        config.stage_timeouts()?,
        config.build_concurrency,
    );
```

(`gc` пока живёт только внутри `build_state` — в `AppState` она попадёт в Task 10, поэтому уже сейчас передаём в `DeployProject` именно `Arc::clone(&gc)`.)

- [ ] **Step 8: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS (все крейты; в т.ч. http-тесты bin).

```bash
rtk git add -A && rtk git commit -m "feat(app): cancellation, stage timeouts, build semaphore and post-success gc in deploy"
```

---

### Task 8: `DeployScheduler` — очередь latest-wins (pending-слот глубины 1, superseded)

**Files:**
- Create: `crates/application/src/scheduler.rs`
- Modify: `crates/application/src/lib.rs` (`+pub mod scheduler;`, `−pub mod locks;`)
- Delete: `crates/application/src/locks.rs`
- Modify: `crates/application/src/deploy.rs` (убрать permit/try_begin/locks)
- Modify: `crates/domain/src/error.rs` (−`DeployInProgress`)
- Modify: `crates/bin/src/proto.rs` (`DeployAccepted.queued`)
- Modify: `crates/bin/src/agent/http.rs`, `crates/bin/src/agent/state.rs`, `crates/bin/src/cli/api.rs`

- [ ] **Step 1: Написать scheduler.rs с падающими тестами**

Создать `crates/application/src/scheduler.rs`:

```rust
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pi_domain::contracts::{Clock, DeploymentHistory, LogSink};
use pi_domain::entities::{DeployRef, Deployment, DeploymentStatus, ProjectConfig};
use pi_domain::error::DomainError;
use tokio_util::sync::CancellationToken;

use crate::deploy::DeployProject;

/// Runs one deployment to completion (it records running/terminal statuses
/// itself). Implemented by DeployProject; tests substitute a controllable fake.
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
    /// Project was idle — deploy started right away.
    Started,
    /// Took the empty pending slot (depth 1, §8.1).
    Queued,
    /// Replaced an older queued deploy; that one is now `superseded`.
    QueuedReplacing { superseded_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelOutcome {
    /// Was waiting in the pending slot; removed and recorded as canceled.
    CanceledQueued,
    /// Currently running; cancel signal sent — the runner records the result.
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

/// Latest-wins deploy queue (§8.1): one running + one pending per project,
/// a newer submit displaces the pending one (`superseded`). Owns per-project
/// serialization (in memory only — nothing to clean up after a crash).
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

    /// Drives one project's deploys: runs the current one, then promotes the
    /// pending one (if any) until the slot drains.
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
            // errors are already recorded in history and streamed to the sink
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
```

Тесты в том же файле:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CollectSink;
    use pi_domain::contracts::{MockClock, MockDeploymentHistory};
    use pi_domain::entities::HealthcheckConfig;
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
            timeouts: Default::default(),
        }
    }

    /// Runner gated by a zero-permit semaphore: a run "finishes" only when the
    /// test adds a permit; cancellation finishes it immediately.
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
            .submit("d1".into(), config("a"), DeployRef::Branch("main".into()), CollectSink::new())
            .await
            .unwrap();
        wait_until("d1 started", || runner.started_ids() == vec!["d1"]).await;

        let outcome = scheduler
            .submit("d2".into(), config("a"), DeployRef::Branch("main".into()), CollectSink::new())
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
            .submit("d1".into(), config("a"), DeployRef::Branch("main".into()), CollectSink::new())
            .await
            .unwrap();
        wait_until("d1 started", || runner.started_ids() == vec!["d1"]).await;
        let d2_sink = CollectSink::new();
        scheduler
            .submit("d2".into(), config("a"), DeployRef::Branch("main".into()), d2_sink.clone())
            .await
            .unwrap();

        let outcome = scheduler
            .submit("d3".into(), config("a"), DeployRef::Branch("main".into()), CollectSink::new())
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
            vec![DeploymentStatus::Superseded],
            "the displaced CLI follower must see a terminal event"
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
            .submit("d1".into(), config("a"), DeployRef::Branch("main".into()), CollectSink::new())
            .await
            .unwrap();
        wait_until("d1 started", || runner.started_ids() == vec!["d1"]).await;
        let d2_sink = CollectSink::new();
        scheduler
            .submit("d2".into(), config("a"), DeployRef::Branch("main".into()), d2_sink.clone())
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
            .submit("d1".into(), config("a"), DeployRef::Branch("main".into()), CollectSink::new())
            .await
            .unwrap();
        wait_until("d1 started", || runner.started_ids() == vec!["d1"]).await;
        scheduler
            .submit("d2".into(), config("a"), DeployRef::Branch("main".into()), CollectSink::new())
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
            .submit("d1".into(), config("a"), DeployRef::Branch("main".into()), CollectSink::new())
            .await
            .unwrap();
        runner.gate.add_permits(1);
        wait_until("d1 finished", || {
            runner.finished_count.load(Ordering::SeqCst) == 1
        })
        .await;

        let outcome = scheduler
            .submit("d2".into(), config("a"), DeployRef::Branch("main".into()), CollectSink::new())
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
```

В `crates/application/src/lib.rs`: `pub mod scheduler;`. ПРИМЕЧАНИЕ: `test_support` сейчас `#[cfg(test)] pub mod` — он доступен. `DeployProject::execute` ещё со старой сигнатурой — компиляция упадёт; следующий шаг чинит.

- [ ] **Step 2: Убрать permit/locks из DeployProject**

`crates/application/src/deploy.rs`:
- удалить `use crate::locks::{DeployLocks, DeployPermit};`, поле `locks`, метод `try_begin`, строку `locks: DeployLocks::new(),` из конструктора;
- из сигнатуры `execute` удалить параметр `permit` и строку `let _permit = permit;` (сериализацию гарантирует scheduler);
- удалить вызов `self.history.record_queued(&deployment).await?;` и оставить только `mark_running` (запись queued теперь делает scheduler). Литерал `Deployment` создавать сразу со статусом Running:

```rust
        let started_at = self.clock.now_unix();
        self.history.mark_running(&deployment_id, started_at).await?;
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
```

- Удалить файл `crates/application/src/locks.rs` и `pub mod locks;` из lib.rs.

Тесты deploy.rs: убрать `permit` из всех вызовов `execute`, удалить тесты `try_begin_twice_returns_deploy_in_progress` и `lock_released_after_execute_finishes` (сериализация теперь покрыта тестами scheduler), из остальных убрать `deploy.try_begin(...)`; в `happy_path` убрать ожидание `record_queued` (остаётся `mark_running`; порядок начинается с "running"); в `ok_pre_stages` и остальных — убрать `expect_record_queued`. Проверка «lock must be free» в cancel-тесте заменяется на отсутствие (lock больше не существует).

- [ ] **Step 3: Удалить `DeployInProgress`, добавить маппинг `Conflict`**

`crates/domain/src/error.rs`: удалить вариант `DeployInProgress`.
`crates/bin/src/agent/http.rs` (`impl IntoResponse for ApiError`): заменить строку `DomainError::DeployInProgress(_) => StatusCode::CONFLICT,` на `DomainError::Conflict(_) => StatusCode::CONFLICT,`.
`crates/bin/src/cli/api.rs` (`deploy`): удалить блок

```rust
        if resp.status() == reqwest::StatusCode::CONFLICT {
            anyhow::bail!("deploy of this project is already in progress on the agent");
        }
```

(409 на POST больше не существует — submit всегда принимает).

- [ ] **Step 4: bin — submit через scheduler**

`crates/bin/src/proto.rs` — `DeployAccepted`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployAccepted {
    pub deployment_id: String,
    /// true when the deploy waits behind an active one (latest wins, §8.1).
    #[serde(default)]
    pub queued: bool,
}
```

`crates/bin/src/agent/state.rs`:

```rust
use pi_application::scheduler::{DeployRunner, DeployScheduler};

#[derive(Clone)]
pub struct AppState {
    pub scheduler: Arc<DeployScheduler>,
    pub list: Arc<ListProjects>,
    pub history: Arc<dyn DeploymentHistory>,
    pub hub: Arc<DeployEventsHub>,
    pub ids: Arc<dyn IdGen>,
    pub send_env: Arc<SendEnv>,
    pub env_keys: Arc<ListEnvKeys>,
}
```

В `build_state` после создания `deploy`:

```rust
    let scheduler = DeployScheduler::new(
        deploy as Arc<dyn DeployRunner>,
        Arc::clone(&history),
        SystemClock::new(),
    );
```

и в литерале `AppState` — `scheduler` вместо `deploy`.

`crates/bin/src/agent/http.rs` — `create_deployment` (вместо try_begin/spawn):

```rust
    let git_ref = DeployRef::parse(req.git_ref.as_deref().unwrap_or(&config.branch));

    let deployment_id = state.ids.new_id();
    let sink = state.hub.register(&deployment_id);
    let outcome = state
        .scheduler
        .submit(deployment_id.clone(), config, git_ref, sink)
        .await
        .map_err(ApiError)?;
    let queued = !matches!(outcome, pi_application::scheduler::SubmitOutcome::Started);

    Ok((
        StatusCode::ACCEPTED,
        Json(DeployAccepted {
            deployment_id,
            queued,
        }),
    )
        .into_response())
```

(импорты `DeployProject`/`tokio_util` в http.rs больше не нужны — `tokio::spawn` из хендлера ушёл; `tracing::warn` для ошибок living в scheduler не нужен — runner пишет в history/sink.)

Тестовый `state_with`: построить scheduler поверх deploy и положить в AppState:

```rust
        let scheduler = pi_application::scheduler::DeployScheduler::new(
            deploy as Arc<dyn pi_application::scheduler::DeployRunner>,
            Arc::clone(&history),
            SystemClock::new(),
        );
        AppState {
            scheduler,
            list,
            history,
            hub: DeployEventsHub::new(),
            ids: UuidGen::new(),
            send_env,
            env_keys,
        }
```

- [ ] **Step 5: Переписать http-тест 409 на очередь**

В `crates/bin/src/agent/http.rs` тест `concurrent_deploy_of_same_project_is_409` заменить на:

```rust
    #[tokio::test]
    async fn concurrent_deploys_queue_with_latest_wins() {
        struct GatedSource(Arc<tokio::sync::Notify>);

        #[async_trait::async_trait]
        impl Source for GatedSource {
            fn workdir(&self, project_name: &str) -> std::path::PathBuf {
                std::env::temp_dir().join(project_name)
            }

            async fn fetch(
                &self,
                p: &ProjectConfig,
                _r: &DeployRef,
                _l: Arc<dyn LogSink>,
            ) -> Result<FetchedSource, DomainError> {
                self.0.notified().await;
                Ok(FetchedSource {
                    workdir: std::env::temp_dir().join(&p.name),
                    commit_sha: SHA.into(),
                })
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let gate = Arc::new(tokio::sync::Notify::new());
        let app = router(state_with(
            dir.path(),
            Arc::new(GatedSource(Arc::clone(&gate))),
            Arc::new(ok_runtime()),
        ));

        // 1st: starts; 2nd: queued; 3rd: queued replacing the 2nd
        let (status, json) = request(
            app.clone(),
            post_json("/v1/deployments", &deploy_body("rateme")),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(json["queued"], false);

        let (status, json) = request(
            app.clone(),
            post_json("/v1/deployments", &deploy_body("rateme")),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{json}");
        assert_eq!(json["queued"], true);
        let superseded_id = json["deployment_id"].as_str().unwrap().to_string();

        let (status, json) = request(
            app.clone(),
            post_json("/v1/deployments", &deploy_body("rateme")),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(json["queued"], true);

        // the displaced deployment is terminal `superseded` in history
        let (status, json) = request(
            app.clone(),
            get_req(&format!("/v1/deployments/{superseded_id}")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "superseded");

        gate.notify_one(); // release the running deploy
        gate.notify_one(); // and the promoted pending one
    }
```

ПРИМЕЧАНИЕ: тест `deploy_end_to_end_with_mocked_docker` остаётся как есть — после Task 8 запись queued создаётся ДО ответа 202, так что ветка «404 is OK briefly» больше не срабатывает, но и не мешает.

- [ ] **Step 6: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(app): latest-wins deploy scheduler replaces per-project locks"
```

---

### Task 9: Cancel API + свип при старте + `pi deploy --cancel`

**Files:**
- Modify: `crates/bin/src/agent/http.rs`
- Modify: `crates/bin/src/agent/run.rs`
- Modify: `crates/bin/src/cli/api.rs`
- Modify: `crates/bin/src/cli/commands.rs`
- Modify: `crates/bin/src/main.rs`

- [ ] **Step 1: Падающие http-тесты**

В `mod tests` файла `crates/bin/src/agent/http.rs` добавить (хелпер `delete_req` — рядом с `get_req`):

```rust
    fn delete_req(uri: &str) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::delete(uri)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn cancel_unknown_deployment_is_404() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let (status, _) = request(app, delete_req("/v1/deployments/nope")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn cancel_finished_deployment_is_409() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let (status, json) = request(
            app.clone(),
            post_json("/v1/deployments", &deploy_body("rateme")),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let id = json["deployment_id"].as_str().unwrap().to_string();

        // wait for terminal status
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            assert!(tokio::time::Instant::now() < deadline, "deploy hung");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let (_, json) = request(app.clone(), get_req(&format!("/v1/deployments/{id}"))).await;
            if json["status"] == "success" {
                break;
            }
        }

        let (status, json) = request(app.clone(), delete_req(&format!("/v1/deployments/{id}"))).await;
        assert_eq!(status, StatusCode::CONFLICT, "{json}");
    }

    #[tokio::test]
    async fn cancel_running_deployment_marks_it_canceled() {
        struct HangingSource;

        #[async_trait::async_trait]
        impl Source for HangingSource {
            fn workdir(&self, project_name: &str) -> std::path::PathBuf {
                std::env::temp_dir().join(project_name)
            }
            async fn fetch(
                &self,
                _p: &ProjectConfig,
                _r: &DeployRef,
                _l: Arc<dyn LogSink>,
            ) -> Result<FetchedSource, DomainError> {
                std::future::pending().await
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(HangingSource),
            Arc::new(ok_runtime()),
        ));

        let (status, json) = request(
            app.clone(),
            post_json("/v1/deployments", &deploy_body("rateme")),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let id = json["deployment_id"].as_str().unwrap().to_string();

        // the running deploy is visible via the active list
        let (status, json) = request(
            app.clone(),
            get_req("/v1/projects/rateme/deployments/active"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json[0]["id"], id.as_str(), "{json}");

        let (status, json) =
            request(app.clone(), delete_req(&format!("/v1/deployments/{id}"))).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["status"], "canceling");

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            assert!(
                tokio::time::Instant::now() < deadline,
                "cancel did not land"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let (_, json) = request(app.clone(), get_req(&format!("/v1/deployments/{id}"))).await;
            if json["status"] == "canceled" {
                break;
            }
        }
    }
```

Run: `rtk cargo test -p pi agent::http` → FAIL (роутов нет).

- [ ] **Step 2: Роуты и хендлеры**

В `router()`:

```rust
        .route(
            "/v1/deployments/{id}",
            get(get_deployment).delete(cancel_deployment),
        )
        .route(
            "/v1/projects/{name}/deployments/active",
            get(active_deployments),
        )
```

Хендлеры:

```rust
/// DELETE /v1/deployments/{id} (§8.1, §9.1): queued — removed immediately,
/// running — the cancel token is signalled and the runner records `canceled`.
async fn cancel_deployment(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use pi_application::scheduler::CancelOutcome;
    match state.scheduler.cancel(&id).await.map_err(ApiError)? {
        CancelOutcome::CanceledQueued => Ok(Json(serde_json::json!({ "status": "canceled" }))),
        CancelOutcome::CancelRequested => Ok(Json(serde_json::json!({ "status": "canceling" }))),
        CancelOutcome::NotActive => match state.history.get(&id).await.map_err(ApiError)? {
            Some(d) => Err(ApiError(DomainError::Conflict(format!(
                "deployment {id} already finished ({})",
                d.status.as_str()
            )))),
            None => Err(ApiError(DomainError::NotFound(format!("deployment {id}")))),
        },
    }
}

/// Active (queued/running) deployments of a project — used by `pi deploy --cancel`.
async fn active_deployments(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Vec<DeploymentDto>>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    let list = state.history.active(&name).await.map_err(ApiError)?;
    Ok(Json(list.into_iter().map(Into::into).collect()))
}
```

Run: `rtk cargo test -p pi agent::http` → PASS.

- [ ] **Step 3: Свип при старте**

`crates/bin/src/agent/run.rs` — после `let state = build_state(&config)?;`:

```rust
    // §8.1: crash recovery — whatever was queued/running when the previous
    // process died can never finish; mark it before accepting new work.
    let now = pi_infrastructure::sys::SystemClock::new().now_unix();
    let swept = state
        .history
        .sweep_interrupted(now)
        .await
        .map_err(|e| anyhow::anyhow!("startup sweep: {e}"))?;
    if swept > 0 {
        tracing::warn!("marked {swept} unfinished deployment(s) as interrupted (agent restart)");
    }
```

(импорт `use pi_domain::contracts::Clock;` для `now_unix`). Поведение `sweep_interrupted` покрыто инфра-тестами Task 4; сам call-site проверяется вручную при приёмке (перезапуск агента во время деплоя).

- [ ] **Step 4: CLI — ApiClient + `pi deploy --cancel` + exit-коды**

`crates/bin/src/cli/api.rs` (импортировать `DeploymentDto` из `crate::proto`):

```rust
    pub async fn active_deployments(&self, project: &str) -> anyhow::Result<Vec<DeploymentDto>> {
        let resp = self
            .http
            .get(format!(
                "{}/v1/projects/{project}/deployments/active",
                self.base
            ))
            .send()
            .await?;
        Ok(extract_error(resp).await?.json().await?)
    }

    /// Returns the agent's cancel decision: "canceled" | "canceling".
    pub async fn cancel_deployment(&self, id: &str) -> anyhow::Result<String> {
        let resp = self
            .http
            .delete(format!("{}/v1/deployments/{id}", self.base))
            .send()
            .await?;
        let json: serde_json::Value = extract_error(resp).await?.json().await?;
        Ok(json["status"].as_str().unwrap_or("unknown").to_string())
    }
```

`crates/bin/src/cli/commands.rs` — новая команда:

```rust
/// `pi deploy --cancel` (§8.1): cancels ALL active deploys of the project —
/// the queued one (if any) and the running one.
pub async fn deploy_cancel(server: Option<String>) -> anyhow::Result<()> {
    let pitoml = PiToml::load(Path::new("pi.toml"))?;
    let project_name = pitoml.project.name.clone();

    let profile = ClientConfig::load()?.select(server.as_deref())?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let active = api.active_deployments(&project_name).await?;
    if active.is_empty() {
        eprintln!("no active deployment for '{project_name}' - nothing to cancel");
        return Ok(());
    }
    for d in active {
        let decision = api.cancel_deployment(&d.id).await?;
        eprintln!("deployment {} ({}): {decision}", d.id, d.status);
    }
    Ok(())
}
```

В `deploy()` — сообщение об очереди и новые exit-коды (заменить хвост функции после `api.deploy(&req).await?`):

```rust
    let accepted = api.deploy(&req).await?;
    if accepted.queued {
        eprintln!(
            "deployment {} queued behind the active deploy (latest wins); waiting...",
            accepted.deployment_id
        );
    } else {
        eprintln!(
            "deployment {} started; streaming logs:",
            accepted.deployment_id
        );
    }

    let status = api
        .follow_logs(&accepted.deployment_id, |line| println!("{line}"))
        .await?;
    eprintln!("deploy finished: {status}");
    if status == "superseded" {
        // latest wins (§8.1): a newer deploy carries a fresher ref — green CI
        eprintln!("note: a newer deploy request replaced this one - not an error");
    }
    if status != "success" && status != "superseded" {
        drop(tunnel);
        std::process::exit(1);
    }
    Ok(())
```

`crates/bin/src/main.rs` — флаг:

```rust
    /// Deploy current project (reads ./pi.toml)
    Deploy {
        /// Branch or commit-sha (default — branch from pi.toml)
        #[arg(long = "ref", conflicts_with = "cancel")]
        git_ref: Option<String>,
        /// Cancel the active deploy(s) of the current project instead
        #[arg(long)]
        cancel: bool,
        /// Server profile from ~/.config/pi/config.toml
        #[arg(long)]
        server: Option<String>,
    },
```

Диспатч:

```rust
        Cmd::Deploy {
            git_ref,
            cancel,
            server,
        } => {
            if cancel {
                cli::commands::deploy_cancel(server).await
            } else {
                cli::commands::deploy(git_ref, server).await
            }
        }
```

- [ ] **Step 5: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(bin): cancel API, startup sweep and pi deploy --cancel"
```

---

### Task 10: `POST /v1/gc` + команда `pi gc`

**Files:**
- Modify: `crates/bin/src/agent/state.rs` (AppState.gc)
- Modify: `crates/bin/src/agent/http.rs`
- Modify: `crates/bin/src/proto.rs` (GcResponse)
- Modify: `crates/bin/src/cli/api.rs`, `crates/bin/src/cli/commands.rs`, `crates/bin/src/main.rs`

- [ ] **Step 1: Падающий http-тест**

В `mod tests` http.rs (хелпер POST без тела):

```rust
    fn post_empty(uri: &str) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::post(uri)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn gc_endpoint_reports_disk_and_prune_decision() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let (status, json) = request(app, post_empty("/v1/gc")).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["disk_used_percent"], 10, "MockDiskProbe in state_with");
        assert_eq!(json["builder_pruned"], false);
    }
```

Run: `rtk cargo test -p pi agent::http gc` → FAIL.

- [ ] **Step 2: AppState.gc + роут**

`crates/bin/src/agent/state.rs`: в `AppState` добавить `pub gc: Arc<RunGc>,` (импорт `use pi_application::gc::RunGc;`); в `build_state` передать в DeployProject клон и положить gc в литерал AppState:

```rust
    let deploy = DeployProject::new(
        /* ... */
        Arc::clone(&gc),
        config.stage_timeouts()?,
        config.build_concurrency,
    );
    /* ... */
    Ok(AppState {
        scheduler,
        list,
        history,
        hub: DeployEventsHub::new(),
        ids: UuidGen::new(),
        send_env,
        env_keys,
        gc,
    })
```

То же в тестовом `state_with` (gc уже строится там с Task 7 — добавить `Arc::clone(&gc)` в DeployProject и `gc` в AppState).

`crates/bin/src/proto.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcResponse {
    pub disk_used_percent: u8,
    pub builder_pruned: bool,
}
```

`crates/bin/src/agent/http.rs`: роут `.route("/v1/gc", post(run_gc))` и хендлер (вывод prune уходит в журнал агента через существующий `TracingSink`):

```rust
/// POST /v1/gc (§8.1): same RunGc as the post-deploy stage, on demand.
async fn run_gc(State(state): State<AppState>) -> Result<Json<GcResponse>, ApiError> {
    let report = state
        .gc
        .execute(Arc::new(TracingSink))
        .await
        .map_err(ApiError)?;
    Ok(Json(GcResponse {
        disk_used_percent: report.disk_used_percent,
        builder_pruned: report.builder_pruned,
    }))
}
```

(добавить `GcResponse` в импорт из `crate::proto`).

Run: `rtk cargo test -p pi agent::http` → PASS.

- [ ] **Step 3: CLI `pi gc`**

`crates/bin/src/cli/api.rs`:

```rust
    pub async fn gc(&self) -> anyhow::Result<GcResponse> {
        let resp = self
            .http
            .post(format!("{}/v1/gc", self.base))
            .send()
            .await?;
        Ok(extract_error(resp).await?.json().await?)
    }
```

(импортировать `GcResponse`).

`crates/bin/src/cli/commands.rs` (pi.toml не нужен — операция на весь агент):

```rust
pub async fn gc(server: Option<String>) -> anyhow::Result<()> {
    let profile = ClientConfig::load()?.select(server.as_deref())?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let resp = api.gc().await?;
    eprintln!(
        "gc done: disk {}% used; build cache pruned: {}",
        resp.disk_used_percent,
        if resp.builder_pruned { "yes" } else { "no" }
    );
    Ok(())
}
```

`crates/bin/src/main.rs`:

```rust
    /// Prune docker images and build cache on the agent (§8.1)
    Gc {
        #[arg(long)]
        server: Option<String>,
    },
```

и `Cmd::Gc { server } => cli::commands::gc(server).await,`.

- [ ] **Step 4: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(bin): pi gc command backed by POST /v1/gc"
```

---

### Task 11: CI-флаги `--host/--user/--key` + version-handshake warning

**Files:**
- Modify: `crates/bin/src/cli/config.rs` (ConnectOpts)
- Modify: `crates/bin/src/main.rs` (flatten во все remote-команды + clap-тесты)
- Modify: `crates/bin/src/cli/commands.rs` (resolve + warning)

- [ ] **Step 1: Падающие тесты**

В `mod tests` файла `crates/bin/src/cli/config.rs`:

```rust
    #[test]
    fn connect_opts_with_host_bypass_the_config_file() {
        let opts = ConnectOpts {
            server: None,
            host: Some("203.0.113.7".into()),
            user: Some("pi".into()),
            key: Some("./deploy_key".into()),
        };
        let profile = opts.resolve().unwrap();
        assert_eq!(profile.host, "203.0.113.7");
        assert_eq!(profile.user, "pi");
        assert_eq!(profile.key.as_deref(), Some("./deploy_key"));
    }
```

В `crates/bin/src/main.rs` — новый `mod tests` в конце файла:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn deploy_host_requires_user() {
        assert!(Cli::try_parse_from(["pi", "deploy", "--host", "203.0.113.7"]).is_err());
    }

    #[test]
    fn deploy_ci_flags_parse() {
        let cli = Cli::try_parse_from([
            "pi", "deploy", "--host", "203.0.113.7", "--user", "pi", "--key", "./k",
        ])
        .unwrap();
        match cli.cmd {
            Cmd::Deploy { connect, .. } => {
                assert_eq!(connect.host.as_deref(), Some("203.0.113.7"));
                assert_eq!(connect.user.as_deref(), Some("pi"));
                assert_eq!(connect.key.as_deref(), Some("./k"));
            }
            _ => panic!("expected deploy"),
        }
    }

    #[test]
    fn server_flag_conflicts_with_host() {
        assert!(Cli::try_parse_from([
            "pi", "deploy", "--server", "home", "--host", "203.0.113.7", "--user", "pi",
        ])
        .is_err());
    }
}
```

В `mod tests` файла `crates/bin/src/cli/commands.rs`:

```rust
    #[test]
    fn version_mismatch_produces_warning_only_on_difference() {
        assert!(version_mismatch_warning("0.3.0", "0.3.0").is_none());
        let warning = version_mismatch_warning("0.3.0", "0.2.0").unwrap();
        assert!(warning.contains("0.3.0") && warning.contains("0.2.0"));
    }
```

Run: `rtk cargo test -p pi` → FAIL.

- [ ] **Step 2: ConnectOpts**

В `crates/bin/src/cli/config.rs`:

```rust
/// Connection selection shared by all remote commands (§16): a profile from
/// the client config (--server / PI_SERVER / default), or a direct
/// --host/--user/--key triple for CI that bypasses the config file entirely.
#[derive(Debug, clap::Args)]
pub struct ConnectOpts {
    /// Server profile from ~/.config/pi/config.toml
    #[arg(long, conflicts_with = "host")]
    pub server: Option<String>,
    /// Direct SSH host (CI mode; the client config file is not read)
    #[arg(long, requires = "user")]
    pub host: Option<String>,
    /// SSH login user for --host
    #[arg(long, requires = "host")]
    pub user: Option<String>,
    /// SSH private key path for --host
    #[arg(long, requires = "host")]
    pub key: Option<String>,
}

impl ConnectOpts {
    pub fn resolve(&self) -> anyhow::Result<ServerProfile> {
        if let (Some(host), Some(user)) = (&self.host, &self.user) {
            return Ok(ServerProfile {
                host: host.clone(),
                user: user.clone(),
                key: self.key.clone(),
            });
        }
        ClientConfig::load()?.select(self.server.as_deref())
    }
}
```

- [ ] **Step 3: Flatten во все команды**

`crates/bin/src/main.rs`: во всех вариантах (`Deploy`, `Ls`, `EnvCmd::Send`, `EnvCmd::Ls`, `Gc`) заменить поле `server: Option<String>` на:

```rust
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
```

Диспатч передаёт `connect` вместо `server` (пример для Deploy):

```rust
        Cmd::Deploy {
            git_ref,
            cancel,
            connect,
        } => {
            if cancel {
                cli::commands::deploy_cancel(connect).await
            } else {
                cli::commands::deploy(git_ref, connect).await
            }
        }
```

`crates/bin/src/cli/commands.rs`: сигнатуры `deploy`, `deploy_cancel`, `env_send`, `env_ls`, `ls`, `gc` меняют `server: Option<String>` на `connect: ConnectOpts` (импорт `crate::cli::config::ConnectOpts`), а строка

```rust
    let profile = ClientConfig::load()?.select(server.as_deref())?;
```

везде заменяется на:

```rust
    let profile = connect.resolve()?;
```

(импорт `ClientConfig` в commands.rs после этого не нужен).

- [ ] **Step 4: Version-handshake warning**

В `crates/bin/src/cli/commands.rs`:

```rust
/// §9.1: differing CLI/agent binary versions are a warning, not an error.
fn version_mismatch_warning(cli_version: &str, agent_version: &str) -> Option<String> {
    (cli_version != agent_version).then(|| {
        format!(
            "warning: CLI v{cli_version} and agent v{agent_version} differ - \
rebuild/update the agent on the Pi (`pi agent update` ships in v0.5)"
        )
    })
}
```

В `deploy()` после `eprintln!("agent {} (api {})", ...)`:

```rust
    if let Some(warning) = version_mismatch_warning(env!("CARGO_PKG_VERSION"), &version.version) {
        eprintln!("{warning}");
    }
```

- [ ] **Step 5: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(cli): ci connection flags and version handshake warning"
```

---

### Task 12: Доки (GitHub Actions), версия 0.3.0, финальная проверка

**Files:**
- Create: `docs/ci-github-actions.md`
- Modify: `docs/install-agent-v0.1.md`
- Modify: `Cargo.toml` (workspace version)

- [ ] **Step 1: Пример GitHub Actions workflow**

Создать `docs/ci-github-actions.md`:

````markdown
# CI: деплой из GitHub Actions (v0.3)

`pi deploy` готов к CI (§23 v0.3): очередь latest-wins переживает два пуша
подряд без ретраев, поэтапные таймауты не дают джобе зависнуть, а флаги
`--host/--user/--key` не требуют клиентского конфига на раннере.

## Секреты репозитория (Settings → Secrets → Actions)

| Secret | Что это |
|---|---|
| `PI_HOST` | хост/IP Pi, доступный раннеру по SSH |
| `PI_USER` | логин-юзер Pi (НЕ сервис-юзер `pi-agent`) |
| `PI_SSH_KEY` | приватный ключ; его pubkey — в `authorized_keys` этого юзера |

## .github/workflows/deploy.yml

```yaml
name: deploy

on:
  push:
    branches: [main]

concurrency:
  group: deploy-production
  cancel-in-progress: true # экономит минуты; очередь агента всё равно latest-wins

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      # ... тесты проекта ...

  deploy:
    needs: test
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install pi CLI
        # бинарные релизы + install.sh появятся в v0.5; пока — из исходников.
        # Ускорение: actions/cache на ~/.cargo/bin по хешу ревизии тулы.
        run: cargo install --git https://github.com/khmilevoi/pi --locked pi

      - name: Prepare SSH
        run: |
          mkdir -p ~/.ssh
          ssh-keyscan -H "${{ secrets.PI_HOST }}" >> ~/.ssh/known_hosts
          install -m 600 /dev/null ~/.ssh/deploy_key
          printf '%s\n' "${{ secrets.PI_SSH_KEY }}" > ~/.ssh/deploy_key

      - name: Deploy
        run: |
          pi deploy \
            --ref "$GITHUB_SHA" \
            --host "${{ secrets.PI_HOST }}" \
            --user "${{ secrets.PI_USER }}" \
            --key ~/.ssh/deploy_key
```

## Почему именно так

- **`--ref "$GITHUB_SHA"`** — деплоится ровно протестированный коммит, а не
  «main на момент деплоя».
- **`ssh-keyscan` обязателен**: SSH-туннель ходит с `BatchMode=yes` — без
  known_hosts соединение молча упадёт на проверке host key.
- **Два пуша подряд**: деплой первого может завершиться `superseded` — CLI
  выходит с кодом 0 и джоба остаётся зелёной (latest wins, §8.1). Красные
  статусы: `failed`, `canceled`, `interrupted`.
- **Секреты проекта** не шлются из CI на каждый деплой: bundle уже хранится на
  Pi (`pi env send` делается вручную при смене значений, §10).
- **Зависший build** убьёт поэтапный таймаут агента (`timeout: build`, дефолт
  30 минут) — джоба упадёт с понятной причиной, а не по таймауту раннера.
- **Отмена из CI не нужна**: новый пуш сам вытеснит ожидающий деплой; для
  ручной отмены с рабочей машины есть `pi deploy --cancel`.
````

- [ ] **Step 2: Обновить док установки**

В `docs/install-agent-v0.1.md` в heredoc с `/etc/pi/agent.toml` добавить новые опции (закомментированными, после `# port_max = 8999`):

```toml
    # build_concurrency = 1        # global build semaphore (§8.1)
    # history_keep = 50            # deployments kept per project (§18)
    # [timeouts]                   # stage timeout defaults (§8.1)
    # fetch = "2m"
    # build = "30m"
    # up = "5m"
    # [gc]
    # disk_threshold_percent = 85  # builder prune above this disk usage (§8.1)
```

И добавить в конец документа раздел приёмки v0.3:

```markdown
## Приёмка v0.3 (CI-ready, §23)

1. Два `pi deploy` подряд: второй отвечает `queued`; третий вытесняет второй
   (`superseded`), деплоится самый свежий.
2. `pi deploy --cancel` во время build → деплой `canceled`, docker-процесс убит.
3. Перезапуск агента во время деплоя (`sudo systemctl restart pi-agent`) →
   деплой помечен `interrupted` (виден в `GET /v1/deployments/{id}`).
4. `pi gc` отвечает процентом диска; после успешного деплоя в journald виден
   `docker image prune`.
5. Деплой из GitHub Actions по `docs/ci-github-actions.md` проходит без
   клиентского конфига на раннере.
```

- [ ] **Step 3: Версия 0.3.0**

В корневом `Cargo.toml` (`[workspace.package]`): `version = "0.2.0"` → `version = "0.3.0"`. После этого version-handshake warning (Task 11) сработает против агента v0.2 — это ожидаемо.

- [ ] **Step 4: Финальная верификация**

```bash
rtk cargo fmt --all
rtk cargo clippy --workspace --all-targets
rtk cargo test
```

Expected: fmt без диффа, clippy без предупреждений, все тесты PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add -A && rtk git commit -m "docs: GitHub Actions deploy example; release v0.3.0"
```

---

## Соответствие критерию v0.3 (§23)

| Требование спеки | Где в плане |
|---|---|
| Очередь latest-wins (pending-слот 1, `superseded`) §8.1 | Task 8 |
| Поэтапные таймауты + убийство дочернего процесса §8.1 | Task 1, 2, 3, 7 |
| `pi deploy --cancel` → `DELETE /v1/deployments/{id}` §8.1, §9.1 | Task 7 (token), 8 (scheduler.cancel), 9 (API+CLI) |
| Свип `queued/running → interrupted` при старте §8.1, §18 | Task 4 (метод), 9 (call-site) |
| Build-семафор (=1, настраиваемый) §8.1 | Task 2 (конфиг), 7 (семафор) |
| GC диска + `pi gc` §8.1 | Task 5, 6, 7 (стадия), 10 (endpoint+CLI) |
| Ретеншен БД (50/проект, чистка после вставки) §18 | Task 2 (конфиг), 4 (адаптер) |
| Неинтерактивные флаги `--host/--user/--key` §16 | Task 11 |
| Version-handshake warning §9.1 | Task 11 |
| Пример GitHub Actions workflow | Task 12 |

**Критерий готовности:** GitHub Actions деплоит без присмотра — workflow из
`docs/ci-github-actions.md` зелёный на двух пушах подряд, зависший build
падает по таймауту, упавший агент после рестарта честно показывает
`interrupted`.
