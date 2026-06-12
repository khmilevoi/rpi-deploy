# pi v0.4 (Операционка) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** проблема диагностируется штатными командами без ssh-археологии (§23 v0.4): `pi logs`, `pi stats`, `pi start|stop|restart`, `pi rm` (+DNS-инструкция), `pi status`, `pi doctor`, `pi agent status|logs` с ssh-фолбэком для мёртвого агента, rolling-файлы логов агента.

**Architecture:** расширяем слои v0.3 по тем же правилам (§5): в `domain` — сущности метрик/диагностики (`ServiceStats`/`StatsReport`/`DiagnosticCheck`/`LifecycleAction`/`AgentOverview`) и контракты (`ContainerRuntime` +logs/stats/lifecycle/down, `Ingress.remove`, `SecretStore.remove`, `Source.cleanup`, `OverrideStore.remove`, `ProjectRepository.remove`, `DeploymentHistory` +latest/remove_project, новые `StatsProvider` и `SystemProbe`); в `application` — `StreamLogs`, `GetStats`, `ControlLifecycle`, `RemoveProject`, `RunDiagnostics`, `AgentStatus`; в `infrastructure` — методы `DockerComposeRuntime`, `CompositeStats` (sysinfo), `HostSystemProbe` (инжектируемый `ProbeRunner`), удаление артефактов; в `bin` — rolling-логи (`tracing-appender` + `[logs]` в agent.toml + ретеншен), новые эндпоинты (`/v1/stats`, `/v1/status`, `/v1/doctor`, lifecycle, `DELETE /v1/projects/{name}`, SSE-логи проекта и агента), CLI-команды и `SshExec`-фолбэк.

**Tech Stack:** как v0.3 + `tracing-appender = "0.2"` (rolling-файлы), `time = "0.3"` (RFC3339/даты для логов). Метрики хоста — `sysinfo` (уже в дереве с v0.3).

**Спека:** `docs/superpowers/specs/2026-06-12-pi-operations-v0.4-design.md` (дизайн v0.4); базовая — `docs/superpowers/specs/2026-06-09-pi-deploy-tool-design.md` §6, §7, §11, §14, §16, §18, §23 v0.4.

---

## Скоуп v0.4 (что входит / что НЕ входит)

Входит (дизайн §1):
- `pi logs <project> [-f] [--tail N]` — стрим логов контейнеров, маскировка секретов (дефолт tail 100).
- `pi stats [project]` — live-метрики хоста и проектов (`CompositeStats`), `--json`.
- `pi start|stop|restart <project>` — `compose start/stop/restart`, без rebuild и записи в историю.
- `pi rm <project> [--volumes] [--yes]` — контейнеры (`down`, волюмы только с `--volumes`), ingress-правило, workdir, секреты, deploy-key, override, порт и записи БД; DNS-инструкция; подтверждение имени без `--yes`; `Conflict` при активном деплое.
- `pi status` — обзор агента/хоста, `--json`.
- `pi doctor` — клиентские чеки (ssh, агент, версии) + `GET /v1/doctor`; exit 1 при FAIL.
- `pi agent status|logs [-f] [--since] [--tail]` — через API, ssh-фолбэк (`systemctl status` / `journalctl`) при мёртвом агенте.
- Rolling-логи агента: `tracing-appender` daily, `[logs]` (dir, retention_days=14), чистка старых при старте.

НЕ входит (дизайн §1): `stats_snapshots`/история метрик (отменено решением спеки); `pi agent setup/update/uninstall`, `pi setup`, `pi init`, install.sh — v0.5; интерактивный «отменить?» на Ctrl+C — v0.5; клиентские rolling-логи.

Решения, зафиксированные планом (в рамках дизайна):
- **`ContainerRuntime.stats/logs/lifecycle/down` работают по `-p <project>` без compose-файлов** — compose v2 находит контейнеры по label проекта; `ComposeStack` для этих операций не нужен (уточнение `stats(projects)` из дизайна §4 до per-project `stats(project_name)`; агрегацию по проектам делает `CompositeStats`).
- **`StatsProvider.report(Vec<String>)`** возвращает `last_deploy: None`; заполняет `GetStats` через `DeploymentHistory::latest`.
- **Метрики сервисов** — join `docker compose ps --format json` (Name→Service) с `docker stats --no-stream --format json` (Name→CPU/Mem).
- **SSE проектных/агентских логов**: продьюсер пишет в `ChannelSink` (mpsc), обрыв SSE-клиента дропает стрим → `AbortOnDrop` убивает задачу → `kill_on_drop` убивает `docker compose logs -f`.
- **`since` у `/v1/agent/logs`** — unix-секунды; строки tracing начинаются с RFC3339-таймштампа, RFC3339 UTC сравнивается лексикографически.
- **`parse_duration_secs` получает суффикс `h`** (нужен `pi agent logs --since 2h`).
- **Фолбэк `pi agent logs`** — `journalctl -u pi-agent --since=@<unix>`; `pi agent status` — `systemctl status pi-agent --no-pager`.
- **Lifecycle/rm выполняются синхронно в HTTP-запросе** (вывод — в журнал агента через `TracingSink`, CLI получает JSON-результат, как у `pi gc`).
- **`pi doctor` дисковый чек** использует `gc.disk_threshold_percent` (дизайн §2.5).

## Конвенции для исполнителя

- **Все команды** запускать с префиксом `rtk`: `rtk cargo test`, `rtk git add …`.
- Коммит-сообщения — conventional commits на английском; завершать трейлером `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Разработка на **Windows**, целевая платформа агента — Linux. Юнит-тесты обязаны проходить на Windows; интеграция с реальным docker/systemd — `#[ignore]`.
- Код и комментарии — на английском.
- Без `unwrap()`/`expect()` в use-cases/адаптерах — ошибки через `Result` (§19). В тестах `unwrap()` допустим.
- После каждого зелёного шага — коммит. Один таск = один коммит.

## File Structure

```
crates/
├─ domain/src/
│  ├─ entities.rs        # MOD: +ServiceStats, ProjectStats, HostStats, StatsReport,
│  │                     #      DiagnosticCheck/Report, LifecycleAction, AgentOverview (Task 1)
│  └─ contracts.rs       # MOD: ContainerRuntime +logs/stats/lifecycle/down (Task 2),
│                        #      DeploymentHistory +latest/remove_project (Task 3),
│                        #      ProjectRepository +remove (Task 4),
│                        #      SecretStore/OverrideStore +remove, Source +cleanup (Task 5),
│                        #      Ingress +remove (Task 6), +StatsProvider (Task 8), +SystemProbe (Task 12)
├─ application/src/
│  ├─ logs.rs            # NEW: StreamLogs (Task 7)
│  ├─ stats.rs           # NEW: GetStats (Task 9)
│  ├─ lifecycle.rs       # NEW: ControlLifecycle (Task 10)
│  ├─ remove.rs          # NEW: RemoveProject + RemoveReport (Task 11)
│  ├─ diagnostics.rs     # NEW: RunDiagnostics + AgentStatus (Task 13)
│  └─ lib.rs             # MOD: +logs, +stats, +lifecycle, +remove, +diagnostics
├─ infrastructure/src/
│  ├─ docker.rs          # MOD: logs/stats/lifecycle/down + парсеры (Task 2)
│  ├─ history.rs         # MOD: latest + remove_project (Task 3)
│  ├─ repo.rs            # MOD: remove (Task 4)
│  ├─ secrets.rs         # MOD: remove (Task 5)
│  ├─ overrides.rs       # MOD: remove (Task 5)
│  ├─ git.rs             # MOD: cleanup (Task 5)
│  ├─ cloudflared.rs     # MOD: remove + remove_ingress_rule (Task 6)
│  ├─ stats.rs           # NEW: CompositeStats (Task 8)
│  ├─ probe.rs           # NEW: ProbeRunner + SystemRunner + HostSystemProbe (Task 12)
│  └─ lib.rs             # MOD: +stats, +probe
└─ bin/src/
   ├─ duration.rs        # MOD: суффикс 'h' (Task 14)
   ├─ agent/logfile.rs   # NEW: tail/since/prune/follow rolling-файлов (Task 14)
   ├─ agent/config.rs    # MOD: +[logs] (Task 14)
   ├─ agent/run.rs       # MOD: init tracing с file-слоем, prune при старте (Task 14)
   ├─ agent/mod.rs       # MOD: +mod logfile
   ├─ proto.rs           # MOD: +Stats/Doctor/AgentStatus/Lifecycle/Remove DTO (Task 15)
   ├─ agent/state.rs     # MOD: wiring новых use-cases + log_dir (Task 15)
   ├─ agent/http.rs      # MOD: +6 эндпоинтов + SSE логов (Tasks 15, 16)
   ├─ cli/ssh.rs         # NEW: SshExec (Task 17)
   ├─ cli/mod.rs         # MOD: +mod ssh
   ├─ cli/tunnel.rs      # MOD: expand_home -> pub(crate) (Task 17)
   ├─ cli/api.rs         # MOD: +stats/status/doctor/lifecycle/remove + stream_sse (Task 17)
   ├─ cli/commands.rs    # MOD: +logs/stats/lifecycle/rm/status (Task 18), +doctor/agent_* (Task 19)
   └─ main.rs            # MOD: tracing-реструктуризация, новые сабкоманды (Tasks 14, 18, 19)
Cargo.toml                    # MOD: +tracing-appender, +time; version 0.4.0 (Task 20)
dev/agent.toml                # MOD: пример [logs] (Task 20)
docs/install-agent-v0.1.md    # MOD: новые опции/команды (Task 20)
README.md                     # MOD: статус v0.4 (Task 20)
```

---

### Task 1: Domain — сущности операционки

**Files:**
- Modify: `crates/domain/src/entities.rs`

- [ ] **Step 1: Написать падающие тесты**

В `mod tests` файла `crates/domain/src/entities.rs` добавить:

```rust
    #[test]
    fn lifecycle_action_roundtrips_through_str() {
        for a in [
            LifecycleAction::Start,
            LifecycleAction::Stop,
            LifecycleAction::Restart,
        ] {
            assert_eq!(a.as_str().parse::<LifecycleAction>(), Ok(a));
        }
        assert_eq!("bogus".parse::<LifecycleAction>(), Err(()));
    }

    #[test]
    fn diagnostic_report_all_passed() {
        let pass = DiagnosticCheck {
            name: "docker daemon".into(),
            passed: true,
            detail: "27.0".into(),
            hint: None,
        };
        let fail = DiagnosticCheck {
            name: "cloudflared unit".into(),
            passed: false,
            detail: "inactive".into(),
            hint: Some("systemctl --user start cloudflared".into()),
        };
        assert!(DiagnosticReport {
            checks: vec![pass.clone()]
        }
        .all_passed());
        assert!(!DiagnosticReport {
            checks: vec![pass, fail]
        }
        .all_passed());
        assert!(
            DiagnosticReport::default().all_passed(),
            "no checks - nothing failed"
        );
    }
```

- [ ] **Step 2: Запустить — убедиться, что падает**

Run: `rtk cargo test -p pi-domain`
Expected: FAIL — нет `LifecycleAction`, `DiagnosticCheck`, `DiagnosticReport`.

- [ ] **Step 3: Реализовать сущности**

В `crates/domain/src/entities.rs` (после `ComposeStack`) добавить:

```rust
/// Live container metrics of one compose service (`pi stats`, v0.4 design §4).
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceStats {
    pub service: String,
    pub cpu_percent: f64,
    pub mem_used_bytes: u64,
    pub mem_limit_bytes: u64,
}

/// Per-project slice of `pi stats`. last_deploy is filled by the GetStats
/// use-case from DeploymentHistory, not by the stats provider.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectStats {
    pub project: String,
    pub services: Vec<ServiceStats>,
    pub last_deploy: Option<Deployment>,
}

/// Host metrics (sysinfo + DiskProbe).
#[derive(Debug, Clone, PartialEq)]
pub struct HostStats {
    pub cpu_percent: f64,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
    pub disk_used_percent: u8,
    pub uptime_secs: u64,
}

/// Full `pi stats` payload.
#[derive(Debug, Clone, PartialEq)]
pub struct StatsReport {
    pub host: HostStats,
    pub projects: Vec<ProjectStats>,
}

/// One PASS/FAIL check of `pi doctor` (§14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
    /// How to fix; only meaningful on failed checks.
    pub hint: Option<String>,
}

/// `pi doctor` result (§14).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiagnosticReport {
    pub checks: Vec<DiagnosticCheck>,
}

impl DiagnosticReport {
    pub fn all_passed(&self) -> bool {
        self.checks.iter().all(|c| c.passed)
    }
}

/// `pi start|stop|restart` (§16). Maps 1:1 to compose subcommands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
    Start,
    Stop,
    Restart,
}

impl LifecycleAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            LifecycleAction::Start => "start",
            LifecycleAction::Stop => "stop",
            LifecycleAction::Restart => "restart",
        }
    }
}

impl std::str::FromStr for LifecycleAction {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "start" => Ok(LifecycleAction::Start),
            "stop" => Ok(LifecycleAction::Stop),
            "restart" => Ok(LifecycleAction::Restart),
            _ => Err(()),
        }
    }
}

/// `pi status` summary (v0.4 design §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOverview {
    pub version: String,
    pub uptime_secs: u64,
    pub disk_used_percent: u8,
    pub projects: usize,
    pub active_deployments: usize,
}
```

- [ ] **Step 4: Прогнать тесты и закоммитить**

Run: `rtk cargo test -p pi-domain`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(domain): stats, diagnostics and lifecycle entities for v0.4"
```

(трейлер `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>` — добавлять во все коммиты, как в конвенциях.)

---

### Task 2: `ContainerRuntime` — logs / stats / lifecycle / down

**Files:**
- Modify: `crates/domain/src/contracts.rs`
- Modify: `crates/infrastructure/src/docker.rs`

- [ ] **Step 1: Расширить контракт**

В `crates/domain/src/contracts.rs` в трейт `ContainerRuntime` добавить (после `prune_builder`); импорт `LifecycleAction`, `ServiceStats` из `crate::entities`:

```rust
    /// `docker compose logs` of the project (v0.4). With `follow` the future
    /// completes only when interrupted — dropping it kills the child
    /// (kill_on_drop in the process adapter).
    async fn logs(
        &self,
        project_name: &str,
        tail: usize,
        follow: bool,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError>;
    /// Live CPU/memory per service (`docker stats --no-stream`).
    async fn stats(&self, project_name: &str) -> Result<Vec<ServiceStats>, DomainError>;
    /// `docker compose start|stop|restart` — no rebuild, not a deploy.
    async fn lifecycle(
        &self,
        project_name: &str,
        action: LifecycleAction,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError>;
    /// `docker compose down` for `pi rm`; named volumes die only with the flag.
    async fn down(
        &self,
        project_name: &str,
        remove_volumes: bool,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError>;
```

- [ ] **Step 2: Падающие тесты арг-билдеров и парсеров**

В `mod tests` файла `crates/infrastructure/src/docker.rs` добавить:

```rust
    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn logs_lifecycle_down_args_shapes() {
        use pi_domain::entities::LifecycleAction;
        assert_eq!(
            logs_args("rateme", 100, false),
            strings(&["compose", "-p", "rateme", "logs", "--tail", "100"])
        );
        assert_eq!(
            logs_args("rateme", 50, true),
            strings(&["compose", "-p", "rateme", "logs", "--tail", "50", "-f"])
        );
        assert_eq!(
            lifecycle_args("rateme", LifecycleAction::Restart),
            strings(&["compose", "-p", "rateme", "restart"])
        );
        assert_eq!(
            down_args("rateme", false),
            strings(&["compose", "-p", "rateme", "down", "--remove-orphans"])
        );
        assert_eq!(
            down_args("rateme", true),
            strings(&[
                "compose",
                "-p",
                "rateme",
                "down",
                "--remove-orphans",
                "--volumes"
            ])
        );
    }

    #[test]
    fn parse_docker_sizes_and_percents() {
        assert_eq!(parse_percent("0.50%"), Some(0.5));
        assert_eq!(parse_percent("nope"), None);
        assert_eq!(parse_size("512B"), Some(512));
        assert_eq!(parse_size("1.5KiB"), Some(1536));
        assert_eq!(parse_size("12.5MiB"), Some(13_107_200));
        assert_eq!(parse_size("1.9GiB"), Some(2_040_109_465));
        assert_eq!(parse_size("2MB"), Some(2_000_000));
        assert_eq!(parse_size("weird"), None);
        assert_eq!(
            parse_mem_usage("12.5MiB / 1.9GiB"),
            Some((13_107_200, 2_040_109_465))
        );
    }

    #[test]
    fn parse_stats_json_joins_services_by_container_name() {
        let ps = concat!(
            r#"{"Name":"rateme-web-1","Service":"web","State":"running"}"#,
            "\n",
            r#"{"Name":"rateme-db-1","Service":"db","State":"running"}"#,
            "\n",
        );
        let stats = concat!(
            r#"{"Name":"rateme-web-1","CPUPerc":"1.25%","MemUsage":"100MiB / 1GiB"}"#,
            "\n",
            r#"{"Name":"rateme-db-1","CPUPerc":"0.00%","MemUsage":"50MiB / 1GiB"}"#,
            "\n",
            r#"{"Name":"other-app-1","CPUPerc":"9.99%","MemUsage":"1MiB / 1GiB"}"#,
            "\n",
        );
        let out = parse_stats_json(ps, stats);
        assert_eq!(out.len(), 2, "foreign containers are ignored");
        assert_eq!(out[0].service, "web");
        assert_eq!(out[0].cpu_percent, 1.25);
        assert_eq!(out[0].mem_used_bytes, 100 * 1024 * 1024);
        assert_eq!(out[0].mem_limit_bytes, 1024 * 1024 * 1024);
        assert_eq!(out[1].service, "db");
    }

    #[test]
    fn container_names_reads_both_ndjson_and_array() {
        let ndjson = "{\"Name\":\"a-web-1\",\"Service\":\"web\"}\n";
        assert_eq!(container_names(ndjson), vec!["a-web-1".to_string()]);
        let array = r#"[{"Name":"a-web-1","Service":"web"}]"#;
        assert_eq!(container_names(array), vec!["a-web-1".to_string()]);
    }
```

Run: `rtk cargo test -p pi-infrastructure docker` → FAIL (функций нет).

- [ ] **Step 3: Реализовать парсеры и арг-билдеры**

В `crates/infrastructure/src/docker.rs` (импорты: добавить `LifecycleAction`, `ServiceStats` в `use pi_domain::entities::{...}`, `use std::collections::HashMap;`):

```rust
/// One JSON document per line (modern compose/docker) or a legacy array.
pub(crate) fn json_lines(output: &str) -> Vec<serde_json::Value> {
    let trimmed = output.trim_start();
    if trimmed.starts_with('[') {
        return serde_json::from_str::<Vec<serde_json::Value>>(trimmed).unwrap_or_default();
    }
    output
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

pub(crate) fn logs_args(project: &str, tail: usize, follow: bool) -> Vec<String> {
    let mut args = vec![
        "compose".to_string(),
        "-p".to_string(),
        project.to_string(),
        "logs".to_string(),
        "--tail".to_string(),
        tail.to_string(),
    ];
    if follow {
        args.push("-f".to_string());
    }
    args
}

pub(crate) fn lifecycle_args(project: &str, action: LifecycleAction) -> Vec<String> {
    vec![
        "compose".to_string(),
        "-p".to_string(),
        project.to_string(),
        action.as_str().to_string(),
    ]
}

pub(crate) fn down_args(project: &str, remove_volumes: bool) -> Vec<String> {
    let mut args = vec![
        "compose".to_string(),
        "-p".to_string(),
        project.to_string(),
        "down".to_string(),
        "--remove-orphans".to_string(),
    ];
    if remove_volumes {
        args.push("--volumes".to_string());
    }
    args
}

pub(crate) fn parse_percent(s: &str) -> Option<f64> {
    s.trim().strip_suffix('%')?.trim().parse().ok()
}

/// "12.5MiB" / "1.9GiB" / "2MB" / "512B" -> bytes.
pub(crate) fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let split = s
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let num: f64 = num.trim().parse().ok()?;
    let mult: f64 = match unit.trim() {
        "B" | "" => 1.0,
        "kB" | "KB" => 1e3,
        "KiB" => 1024.0,
        "MB" => 1e6,
        "MiB" => 1024.0 * 1024.0,
        "GB" => 1e9,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TB" => 1e12,
        "TiB" => 1024.0_f64.powi(4),
        _ => return None,
    };
    Some((num * mult) as u64)
}

/// docker stats MemUsage: "<used> / <limit>".
pub(crate) fn parse_mem_usage(s: &str) -> Option<(u64, u64)> {
    let (used, limit) = s.split_once('/')?;
    Some((parse_size(used)?, parse_size(limit)?))
}

/// Container names of the project from `compose ps --format json`.
pub(crate) fn container_names(ps_output: &str) -> Vec<String> {
    json_lines(ps_output)
        .iter()
        .filter_map(|v| Some(v.get("Name")?.as_str()?.to_string()))
        .collect()
}

/// Joins `compose ps` (Name -> Service) with `docker stats` (Name -> CPU/Mem).
pub(crate) fn parse_stats_json(ps_output: &str, stats_output: &str) -> Vec<ServiceStats> {
    let services: HashMap<String, String> = json_lines(ps_output)
        .iter()
        .filter_map(|v| {
            Some((
                v.get("Name")?.as_str()?.to_string(),
                v.get("Service")?.as_str()?.to_string(),
            ))
        })
        .collect();
    json_lines(stats_output)
        .iter()
        .filter_map(|v| {
            let name = v.get("Name")?.as_str()?;
            let service = services.get(name)?.clone();
            let cpu_percent = parse_percent(v.get("CPUPerc")?.as_str()?)?;
            let (mem_used_bytes, mem_limit_bytes) =
                parse_mem_usage(v.get("MemUsage")?.as_str()?)?;
            Some(ServiceStats {
                service,
                cpu_percent,
                mem_used_bytes,
                mem_limit_bytes,
            })
        })
        .collect()
}
```

Заменить тело `parse_ps_json` на переиспользование `json_lines` (поведение прежнее, тесты v0.1 живут):

```rust
pub(crate) fn parse_ps_json(output: &str) -> Vec<ServiceState> {
    json_lines(output).iter().filter_map(service_state).collect()
}
```

- [ ] **Step 4: Реализовать методы рантайма**

В `impl ContainerRuntime for DockerComposeRuntime` добавить:

```rust
    async fn logs(
        &self,
        project_name: &str,
        tail: usize,
        follow: bool,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError> {
        let mut cmd = Command::new("docker");
        cmd.args(logs_args(project_name, tail, follow));
        run_streamed(cmd, log).await.map_err(DomainError::Runtime)
    }

    async fn stats(&self, project_name: &str) -> Result<Vec<ServiceStats>, DomainError> {
        let mut ps = Command::new("docker");
        ps.args(["compose", "-p", project_name, "ps", "--format", "json"]);
        let ps_out = run_capture(ps).await.map_err(DomainError::Runtime)?;
        let names = container_names(&ps_out);
        if names.is_empty() {
            return Ok(vec![]);
        }
        let mut st = Command::new("docker");
        st.args(["stats", "--no-stream", "--format", "json"]);
        st.args(&names);
        let stats_out = run_capture(st).await.map_err(DomainError::Runtime)?;
        Ok(parse_stats_json(&ps_out, &stats_out))
    }

    async fn lifecycle(
        &self,
        project_name: &str,
        action: LifecycleAction,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError> {
        log.line(&format!("docker compose {} ...", action.as_str()));
        let mut cmd = Command::new("docker");
        cmd.args(lifecycle_args(project_name, action));
        run_streamed(cmd, log).await.map_err(DomainError::Runtime)
    }

    async fn down(
        &self,
        project_name: &str,
        remove_volumes: bool,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError> {
        log.line(&format!(
            "docker compose down{} ...",
            if remove_volumes { " --volumes" } else { "" }
        ));
        let mut cmd = Command::new("docker");
        cmd.args(down_args(project_name, remove_volumes));
        run_streamed(cmd, log).await.map_err(DomainError::Runtime)
    }
```

`MockContainerRuntime` получает методы автоматически (automock); фоллаута в других крейтах нет, пока методы не зовут.

- [ ] **Step 5: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(infra): container logs, stats, lifecycle and down in ContainerRuntime"
```

---

### Task 3: `DeploymentHistory` — latest / remove_project

**Files:**
- Modify: `crates/domain/src/contracts.rs`
- Modify: `crates/infrastructure/src/history.rs`

- [ ] **Step 1: Контракт**

В трейт `DeploymentHistory` (contracts.rs) добавить после `active`:

```rust
    /// Newest deployment of the project by started_at (for `pi stats`).
    async fn latest(&self, project: &str) -> Result<Option<Deployment>, DomainError>;
    /// Deletes all history rows of the project (`pi rm`). Idempotent.
    async fn remove_project(&self, project: &str) -> Result<(), DomainError>;
```

- [ ] **Step 2: Падающие тесты**

В `mod tests` файла `crates/infrastructure/src/history.rs` (хелперы `history(&dir)` / `queued(id, at)` уже есть):

```rust
    #[tokio::test]
    async fn latest_returns_newest_row_by_started_at() {
        let dir = tempfile::tempdir().unwrap();
        let h = history(&dir);
        h.record_queued(&queued("d1", 100)).await.unwrap();
        h.record_finished("d1", DeploymentStatus::Success, Some("abc"), 110, "")
            .await
            .unwrap();
        h.record_queued(&queued("d2", 200)).await.unwrap();

        let latest = h.latest("rateme").await.unwrap().unwrap();
        assert_eq!(latest.id, "d2");
        assert!(h.latest("ghost").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn remove_project_deletes_all_rows_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let h = history(&dir);
        h.record_queued(&queued("d1", 100)).await.unwrap();
        h.record_finished("d1", DeploymentStatus::Failed, None, 110, "")
            .await
            .unwrap();
        h.record_queued(&queued("d2", 200)).await.unwrap();

        h.remove_project("rateme").await.unwrap();
        assert!(h.get("d1").await.unwrap().is_none());
        assert!(h.get("d2").await.unwrap().is_none());
        assert!(h.active("rateme").await.unwrap().is_empty());

        h.remove_project("rateme").await.unwrap(); // idempotent
    }
```

Run: `rtk cargo test -p pi-infrastructure history` → FAIL.

- [ ] **Step 3: Реализовать**

В `impl DeploymentHistory for SqliteHistory` добавить:

```rust
    async fn latest(&self, project: &str) -> Result<Option<Deployment>, DomainError> {
        let project = project.to_string();
        self.db
            .call(move |conn| {
                conn.query_row(
                    "SELECT id, project, git_ref, commit_sha, status, started_at, finished_at, log_tail
                     FROM deployments
                     WHERE project = ?1
                     ORDER BY started_at DESC, id DESC
                     LIMIT 1",
                    params![project],
                    row_to_deployment,
                )
                .optional()
                .map_err(storage_err)
            })
            .await
    }

    async fn remove_project(&self, project: &str) -> Result<(), DomainError> {
        let project = project.to_string();
        self.db
            .call(move |conn| {
                conn.execute("DELETE FROM deployments WHERE project = ?1", params![project])
                    .map_err(storage_err)?;
                Ok(())
            })
            .await
    }
```

- [ ] **Step 4: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(infra): history latest and remove_project"
```

---

### Task 4: `ProjectRepository.remove`

**Files:**
- Modify: `crates/domain/src/contracts.rs`
- Modify: `crates/infrastructure/src/repo.rs`

- [ ] **Step 1: Контракт**

В трейт `ProjectRepository` добавить после `list`:

```rust
    /// Deletes the project row; its host port becomes free for reallocation.
    /// Idempotent — removing a missing project is Ok.
    async fn remove(&self, name: &str) -> Result<(), DomainError>;
```

- [ ] **Step 2: Падающий тест**

В `mod tests` файла `crates/infrastructure/src/repo.rs`:

```rust
    #[tokio::test]
    async fn remove_frees_the_port_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir, 8000, 8999);
        repo.upsert(&cfg("a")).await.unwrap(); // 8000
        repo.upsert(&cfg("b")).await.unwrap(); // 8001

        repo.remove("a").await.unwrap();
        assert!(repo.get("a").await.unwrap().is_none());
        assert_eq!(
            repo.upsert(&cfg("c")).await.unwrap().host_port,
            8000,
            "freed port is reused"
        );

        repo.remove("a").await.unwrap(); // idempotent
        assert_eq!(repo.list().await.unwrap().len(), 2);
    }
```

Run: `rtk cargo test -p pi-infrastructure repo` → FAIL.

- [ ] **Step 3: Реализовать**

В `impl ProjectRepository for SqliteProjectRepo`:

```rust
    async fn remove(&self, name: &str) -> Result<(), DomainError> {
        let name = name.to_string();
        self.db
            .call(move |conn| {
                conn.execute("DELETE FROM projects WHERE name = ?1", params![name])
                    .map_err(storage_err)?;
                Ok(())
            })
            .await
    }
```

- [ ] **Step 4: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(infra): project repository remove with port release"
```

---

### Task 5: Удаление артефактов — `SecretStore.remove`, `OverrideStore.remove`, `Source.cleanup`

**Files:**
- Modify: `crates/domain/src/contracts.rs`
- Modify: `crates/infrastructure/src/secrets.rs`
- Modify: `crates/infrastructure/src/overrides.rs`
- Modify: `crates/infrastructure/src/git.rs`
- Modify (компил-фоллаут — ручные `impl Source` в тестах): `crates/bin/src/agent/http.rs`

- [ ] **Step 1: Контракты**

В contracts.rs добавить методы:

В `SecretStore` после `load`:

```rust
    /// Deletes the stored bundle of the project (`pi rm`). Missing bundle is Ok.
    async fn remove(&self, project: &str) -> Result<(), DomainError>;
```

В `OverrideStore` после `write`:

```rust
    /// Deletes the override file of the project (`pi rm`). Missing file is Ok.
    async fn remove(&self, project: &str) -> Result<(), DomainError>;
```

В `Source` после `fetch`:

```rust
    /// Removes the project workdir and its deploy key (`pi rm`). Idempotent.
    async fn cleanup(&self, project_name: &str) -> Result<(), DomainError>;
```

- [ ] **Step 2: Падающие тесты адаптеров**

В `mod tests` файла `crates/infrastructure/src/secrets.rs` добавить:

```rust
    #[tokio::test]
    async fn remove_deletes_bundle_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::open(dir.path()).unwrap();
        let mut bundle = EnvBundle::default();
        bundle.vars.insert("KEY".into(), "value-long".into());
        store.save("rateme", &bundle).await.unwrap();

        store.remove("rateme").await.unwrap();
        assert!(
            store.load("rateme").await.unwrap().is_empty(),
            "missing bundle loads as empty"
        );
        store.remove("rateme").await.unwrap(); // idempotent
        assert!(store.remove("../evil").await.is_err(), "path traversal");
    }
```

В `mod tests` файла `crates/infrastructure/src/overrides.rs`:

```rust
    #[tokio::test]
    async fn remove_deletes_override_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsOverrideStore::new(dir.path().join("overrides"));
        let path = store.write("rateme", "web", 8000, 3000).await.unwrap();
        assert!(path.exists());

        store.remove("rateme").await.unwrap();
        assert!(!path.exists());
        store.remove("rateme").await.unwrap(); // idempotent
    }
```

В `mod tests` файла `crates/infrastructure/src/git.rs` (юнит-секция, не `mod integration`):

```rust
    #[tokio::test]
    async fn cleanup_removes_workdir_and_keys_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let source = GitSource::new(dir.path());
        let workdir = dir.path().join("workdirs").join("rateme");
        let keys = dir.path().join("keys").join("rateme");
        std::fs::create_dir_all(&workdir).unwrap();
        std::fs::create_dir_all(&keys).unwrap();
        std::fs::write(workdir.join("f.txt"), "x").unwrap();
        std::fs::write(keys.join("id_ed25519"), "k").unwrap();

        source.cleanup("rateme").await.unwrap();
        assert!(!workdir.exists());
        assert!(!keys.exists());
        source.cleanup("rateme").await.unwrap(); // idempotent
    }
```

Run: `rtk cargo test -p pi-infrastructure` → FAIL (методов нет).

- [ ] **Step 3: Реализовать адаптеры**

`crates/infrastructure/src/secrets.rs` — в `impl SecretStore for EncryptedFileStore`:

```rust
    async fn remove(&self, project: &str) -> Result<(), DomainError> {
        let path = self.bundle_path(project)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(secrets_err(format!("remove {}: {e}", path.display()))),
        }
    }
```

`crates/infrastructure/src/overrides.rs` — в `impl OverrideStore for FsOverrideStore`:

```rust
    async fn remove(&self, project: &str) -> Result<(), DomainError> {
        let path = self.dir.join(format!("{project}.yml"));
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(DomainError::Storage(format!("override remove: {e}"))),
        }
    }
```

`crates/infrastructure/src/git.rs` — в `impl Source for GitSource`:

```rust
    async fn cleanup(&self, project_name: &str) -> Result<(), DomainError> {
        for dir in [
            self.workdirs.join(project_name),
            self.keys.join(project_name),
        ] {
            match tokio::fs::remove_dir_all(&dir).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(DomainError::Source(format!(
                        "cleanup {}: {e}",
                        dir.display()
                    )))
                }
            }
        }
        Ok(())
    }
```

- [ ] **Step 4: Починить компил-фоллаут**

В `crates/bin/src/agent/http.rs` (mod tests) у ручных имплементаций `Source` — `GatedSource` и `HangingSource` — добавить:

```rust
            async fn cleanup(&self, _project_name: &str) -> Result<(), DomainError> {
                Ok(())
            }
```

Проверить остальные: `rtk grep "impl Source for"` — должны быть только `git.rs` и два тестовых в `http.rs`.

- [ ] **Step 5: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(infra): project artifact removal in secrets, overrides and git source"
```

---

### Task 6: `Ingress.remove` — cloudflared

**Files:**
- Modify: `crates/domain/src/contracts.rs`
- Modify: `crates/infrastructure/src/cloudflared.rs`

- [ ] **Step 1: Контракт**

В трейт `Ingress` (contracts.rs) добавить после `upsert`:

```rust
    /// Removes the hostname rule; restarts cloudflared only on diff. The DNS
    /// record stays — `cloudflared tunnel route dns` cannot delete it (§11).
    async fn remove(&self, hostname: &str, log: Arc<dyn LogSink>) -> Result<(), DomainError>;
```

- [ ] **Step 2: Падающие тесты**

В `mod tests` файла `crates/infrastructure/src/cloudflared.rs` добавить:

```rust
    #[test]
    fn remove_rule_deletes_matching_hostname_only() {
        let mut d = doc(BASE);
        let changed = remove_ingress_rule(&mut d, "old.example.com").unwrap();
        assert!(changed);
        let rules = d.get("ingress").unwrap().as_sequence().unwrap();
        assert_eq!(rules.len(), 1, "only the catch-all stays");
        assert!(rules[0].get("hostname").is_none());
    }

    #[test]
    fn remove_rule_is_noop_when_absent() {
        let mut d = doc(BASE);
        assert!(!remove_ingress_rule(&mut d, "ghost.example.com").unwrap());
        assert_eq!(d, doc(BASE));
        let mut no_ingress = doc("tunnel: home\n");
        assert!(!remove_ingress_rule(&mut no_ingress, "a.example.com").unwrap());
    }

    #[tokio::test]
    async fn remove_with_missing_config_or_rule_is_ok_and_skips_restart() {
        let dir = tempfile::tempdir().unwrap();
        // missing config file: nothing to remove
        let ingress = CloudflaredIngress::new(
            dir.path().join("config.yml"),
            "home".into(),
            vec!["pi-test-no-such-binary".into()],
        );
        ingress
            .remove("a.example.com", CollectSink::new())
            .await
            .unwrap();

        // config exists but the rule is absent: no diff -> no restart
        let path = dir.path().join("config.yml");
        std::fs::write(&path, BASE).unwrap();
        let ingress = CloudflaredIngress::new(
            path.clone(),
            "home".into(),
            vec!["pi-test-no-such-binary".into()],
        );
        ingress
            .remove("ghost.example.com", CollectSink::new())
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), BASE);
    }

    #[tokio::test]
    async fn failed_restart_on_remove_restores_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(&path, BASE).unwrap();
        let ingress = CloudflaredIngress::new(
            path.clone(),
            "home".into(),
            vec!["pi-test-no-such-binary".into()],
        );
        // restart binary does not exist -> Err; config must be rolled back so
        // a `pi rm` retry diffs again instead of silently passing
        let err = ingress
            .remove("old.example.com", CollectSink::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Ingress(_)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), BASE);
    }
```

Run: `rtk cargo test -p pi-infrastructure cloudflared` → FAIL.

- [ ] **Step 3: Реализовать**

В `crates/infrastructure/src/cloudflared.rs`:

Добавить функцию (рядом с `upsert_ingress_rule`):

```rust
/// Removes the hostname rule. Ok(false) when the rule (or the whole ingress
/// list) is absent — `pi rm` retries must be idempotent.
pub(crate) fn remove_ingress_rule(
    doc: &mut serde_yaml::Value,
    hostname: &str,
) -> Result<bool, String> {
    let map = doc
        .as_mapping_mut()
        .ok_or("config.yml: top level must be a mapping")?;
    let Some(rules) = map
        .get_mut(serde_yaml::Value::from("ingress"))
        .and_then(|v| v.as_sequence_mut())
    else {
        return Ok(false);
    };
    let before = rules.len();
    rules.retain(|r| r.get("hostname").and_then(|h| h.as_str()) != Some(hostname));
    Ok(rules.len() != before)
}
```

Выделить рестарт из `route_dns_and_restart` в отдельный метод (и вызвать его оттуда):

```rust
    async fn restart_cloudflared(&self, log: &Arc<dyn LogSink>) -> Result<(), DomainError> {
        let (program, args) = self
            .restart
            .split_first()
            .ok_or_else(|| ingress_err("empty cloudflared restart command"))?;
        let mut restart_cmd = Command::new(program);
        restart_cmd.args(args);
        run_capture(restart_cmd)
            .await
            .map_err(|e| ingress_err(format!("restart cloudflared: {e}")))?;
        log.line("ingress: cloudflared restarted");
        Ok(())
    }
```

(в `route_dns_and_restart` заменить дублирующийся блок рестарта на `self.restart_cloudflared(log).await`.)

В `impl Ingress for CloudflaredIngress` добавить:

```rust
    async fn remove(&self, hostname: &str, log: Arc<dyn LogSink>) -> Result<(), DomainError> {
        let text = match tokio::fs::read_to_string(&self.config_path).await {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log.line("ingress: no cloudflared config - nothing to remove");
                return Ok(());
            }
            Err(e) => {
                return Err(ingress_err(format!(
                    "cannot read {}: {e}",
                    self.config_path.display()
                )))
            }
        };
        let mut doc: serde_yaml::Value = serde_yaml::from_str(&text).map_err(ingress_err)?;
        let changed = remove_ingress_rule(&mut doc, hostname).map_err(ingress_err)?;
        if !changed {
            log.line(&format!(
                "ingress: no rule for {hostname}; cloudflared untouched"
            ));
            return Ok(());
        }
        let updated = serde_yaml::to_string(&doc).map_err(ingress_err)?;
        tokio::fs::write(&self.config_path, updated)
            .await
            .map_err(ingress_err)?;
        // Same rollback contract as upsert: a persisted change with a failed
        // restart would make the retry see "no diff" and never re-apply (§11).
        if let Err(err) = self.restart_cloudflared(&log).await {
            if let Err(restore) = tokio::fs::write(&self.config_path, &text).await {
                return Err(ingress_err(format!(
                    "{err}; additionally failed to restore {}: {restore}",
                    self.config_path.display()
                )));
            }
            return Err(err);
        }
        log.line(&format!(
            "ingress: rule for {hostname} removed; delete its DNS record in the Cloudflare dashboard"
        ));
        Ok(())
    }
```

В `impl Ingress for DisabledIngress` добавить:

```rust
    async fn remove(&self, hostname: &str, log: Arc<dyn LogSink>) -> Result<(), DomainError> {
        log.line(&format!(
            "ingress: [cloudflared] is not configured in agent.toml; \
             remove the route for {hostname} manually"
        ));
        Ok(())
    }
```

- [ ] **Step 4: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(infra): ingress rule removal with diff-restart and rollback"
```

---

### Task 7: Application — `StreamLogs`

**Files:**
- Create: `crates/application/src/logs.rs`
- Modify: `crates/application/src/lib.rs`

- [ ] **Step 1: Падающие тесты**

Создать `crates/application/src/logs.rs` сразу с тестами:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CollectSink;
    use pi_domain::contracts::{
        LogSink, MockContainerRuntime, MockProjectRepository, MockSecretStore,
    };
    use pi_domain::entities::{EnvBundle, Project, ProjectConfig};
    use pi_domain::error::DomainError;
    use std::sync::Arc;

    fn registered(name: &str) -> Project {
        Project {
            config: ProjectConfig {
                name: name.into(),
                repo: "r".into(),
                branch: "main".into(),
                compose_path: "docker-compose.yml".into(),
                service: "web".into(),
                container_port: 3000,
                hostname: None,
                healthcheck: Default::default(),
                timeouts: Default::default(),
            },
            host_port: 8000,
            created_at: 0,
        }
    }

    #[tokio::test]
    async fn unknown_project_is_not_found_and_runtime_untouched() {
        let mut projects = MockProjectRepository::new();
        projects.expect_get().returning(|_| Ok(None));
        let mut runtime = MockContainerRuntime::new();
        runtime.expect_logs().times(0);
        let secrets = MockSecretStore::new();

        let err = StreamLogs::new(Arc::new(projects), Arc::new(secrets), Arc::new(runtime))
            .execute("ghost", 100, false, CollectSink::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::NotFound(_)));
    }

    #[tokio::test]
    async fn logs_are_masked_with_project_secrets_and_params_forwarded() {
        let mut projects = MockProjectRepository::new();
        projects
            .expect_get()
            .returning(|_| Ok(Some(registered("rateme"))));
        let mut secrets = MockSecretStore::new();
        secrets.expect_load().returning(|_| {
            let mut bundle = EnvBundle::default();
            bundle
                .vars
                .insert("DB_PASSWORD".into(), "supersecretvalue".into());
            Ok(bundle)
        });
        let mut runtime = MockContainerRuntime::new();
        runtime
            .expect_logs()
            .withf(|p, tail, follow, _| p == "rateme" && *tail == 42 && *follow)
            .returning(|_, _, _, sink| {
                sink.line("connecting with supersecretvalue");
                Ok(())
            });

        let sink = CollectSink::new();
        StreamLogs::new(Arc::new(projects), Arc::new(secrets), Arc::new(runtime))
            .execute("rateme", 42, true, sink.clone())
            .await
            .unwrap();
        let lines = sink.lines.lock().unwrap();
        assert_eq!(lines[0], "connecting with ***DB_PASSWORD***");
    }
}
```

В `crates/application/src/lib.rs` добавить `pub mod logs;` (по алфавиту — после `pub mod list;`).

Run: `rtk cargo test -p pi-application logs` → FAIL (нет `StreamLogs`).

- [ ] **Step 2: Реализовать**

В начало `crates/application/src/logs.rs` (над `mod tests`):

```rust
use std::sync::Arc;

use pi_domain::contracts::{ContainerRuntime, LogSink, ProjectRepository, SecretStore};
use pi_domain::error::DomainError;

use crate::mask::MaskingSink;

/// Default --tail for `pi logs` and `pi agent logs` (v0.4 design §3).
pub const DEFAULT_LOG_TAIL: usize = 100;

/// `pi logs <project>` (§7 StreamLogs): container logs through the secret
/// masker — the agent never streams raw secret values (§8.1).
pub struct StreamLogs {
    projects: Arc<dyn ProjectRepository>,
    secrets: Arc<dyn SecretStore>,
    runtime: Arc<dyn ContainerRuntime>,
}

impl StreamLogs {
    pub fn new(
        projects: Arc<dyn ProjectRepository>,
        secrets: Arc<dyn SecretStore>,
        runtime: Arc<dyn ContainerRuntime>,
    ) -> Arc<StreamLogs> {
        Arc::new(StreamLogs {
            projects,
            secrets,
            runtime,
        })
    }

    /// NotFound check separated so the HTTP layer can 404 before streaming.
    pub async fn ensure_project(&self, project: &str) -> Result<(), DomainError> {
        self.projects
            .get(project)
            .await?
            .map(|_| ())
            .ok_or_else(|| DomainError::NotFound(format!("project '{project}'")))
    }

    pub async fn execute(
        &self,
        project: &str,
        tail: usize,
        follow: bool,
        sink: Arc<dyn LogSink>,
    ) -> Result<(), DomainError> {
        self.ensure_project(project).await?;
        let masker = MaskingSink::new(sink);
        masker.arm(&self.secrets.load(project).await?);
        self.runtime.logs(project, tail, follow, masker).await
    }
}
```

Run: `rtk cargo test -p pi-application logs` → PASS.

- [ ] **Step 3: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(app): StreamLogs use-case with secret masking"
```

---

### Task 8: `StatsProvider` + `CompositeStats`

**Files:**
- Modify: `crates/domain/src/contracts.rs`
- Create: `crates/infrastructure/src/stats.rs`
- Modify: `crates/infrastructure/src/lib.rs`

- [ ] **Step 1: Контракт**

В `crates/domain/src/contracts.rs` добавить (импорт `StatsReport` в `use crate::entities::{...}`):

```rust
/// Aggregates container metrics (from ContainerRuntime) with host metrics
/// (§6). last_deploy is filled by the GetStats use-case, not the provider.
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait StatsProvider: Send + Sync {
    async fn report(&self, projects: Vec<String>) -> Result<StatsReport, DomainError>;
}
```

- [ ] **Step 2: Падающий тест CompositeStats**

Создать `crates/infrastructure/src/stats.rs` с тестами:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use pi_domain::contracts::{MockContainerRuntime, MockDiskProbe};
    use pi_domain::entities::ServiceStats;
    use pi_domain::error::DomainError;

    #[tokio::test]
    async fn report_collects_per_project_and_host_metrics() {
        let mut runtime = MockContainerRuntime::new();
        runtime
            .expect_stats()
            .withf(|p| p == "a")
            .returning(|_| {
                Ok(vec![ServiceStats {
                    service: "web".into(),
                    cpu_percent: 1.5,
                    mem_used_bytes: 1024,
                    mem_limit_bytes: 2048,
                }])
            });
        runtime
            .expect_stats()
            .withf(|p| p == "b")
            .returning(|_| Err(DomainError::Runtime("stack is down".into())));
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(42));

        let provider = CompositeStats::new(Arc::new(runtime), Arc::new(disk));
        let report = provider
            .report(vec!["a".to_string(), "b".to_string()])
            .await
            .unwrap();

        assert_eq!(report.projects.len(), 2);
        assert_eq!(report.projects[0].services.len(), 1);
        assert!(
            report.projects[1].services.is_empty(),
            "one broken stack must not kill the report"
        );
        assert!(report.projects.iter().all(|p| p.last_deploy.is_none()));
        assert_eq!(report.host.disk_used_percent, 42);
        assert!(report.host.mem_total_bytes > 0, "real sysinfo numbers");
        assert!(report.host.cpu_percent >= 0.0);
    }
}
```

В `crates/infrastructure/src/lib.rs` добавить `pub mod stats;` (после `pub mod sqlite;`).

Run: `rtk cargo test -p pi-infrastructure stats` → FAIL.

- [ ] **Step 3: Реализовать**

В начало `crates/infrastructure/src/stats.rs`:

```rust
use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::{ContainerRuntime, DiskProbe, StatsProvider};
use pi_domain::entities::{HostStats, ProjectStats, StatsReport};
use pi_domain::error::DomainError;

/// StatsProvider over `docker stats` + sysinfo (§6 CompositeStats).
pub struct CompositeStats {
    runtime: Arc<dyn ContainerRuntime>,
    disk: Arc<dyn DiskProbe>,
}

impl CompositeStats {
    pub fn new(runtime: Arc<dyn ContainerRuntime>, disk: Arc<dyn DiskProbe>) -> Arc<CompositeStats> {
        Arc::new(CompositeStats { runtime, disk })
    }
}

/// CPU needs two samples with a minimal interval (sysinfo contract).
async fn host_stats(disk_used_percent: u8) -> HostStats {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_usage();
    tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    sys.refresh_cpu_usage();
    HostStats {
        cpu_percent: sys.global_cpu_usage() as f64,
        mem_used_bytes: sys.used_memory(),
        mem_total_bytes: sys.total_memory(),
        disk_used_percent,
        uptime_secs: System::uptime(),
    }
}

#[async_trait]
impl StatsProvider for CompositeStats {
    async fn report(&self, projects: Vec<String>) -> Result<StatsReport, DomainError> {
        let mut out = Vec::with_capacity(projects.len());
        for name in projects {
            // a stopped stack / docker hiccup must not kill the whole report
            let services = self.runtime.stats(&name).await.unwrap_or_default();
            out.push(ProjectStats {
                project: name,
                services,
                last_deploy: None,
            });
        }
        let disk = self.disk.used_percent().unwrap_or(0);
        Ok(StatsReport {
            host: host_stats(disk).await,
            projects: out,
        })
    }
}
```

- [ ] **Step 4: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(infra): CompositeStats provider over docker stats and sysinfo"
```

---

### Task 9: Application — `GetStats`

**Files:**
- Create: `crates/application/src/stats.rs`
- Modify: `crates/application/src/lib.rs`

- [ ] **Step 1: Падающие тесты**

Создать `crates/application/src/stats.rs` с тестами:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use pi_domain::contracts::{
        MockDeploymentHistory, MockProjectRepository, MockStatsProvider,
    };
    use pi_domain::entities::{
        Deployment, DeploymentStatus, HostStats, Project, ProjectConfig, ProjectStats,
        StatsReport,
    };
    use pi_domain::error::DomainError;
    use std::sync::Arc;

    fn registered(name: &str) -> Project {
        Project {
            config: ProjectConfig {
                name: name.into(),
                repo: "r".into(),
                branch: "main".into(),
                compose_path: "docker-compose.yml".into(),
                service: "web".into(),
                container_port: 3000,
                hostname: None,
                healthcheck: Default::default(),
                timeouts: Default::default(),
            },
            host_port: 8000,
            created_at: 0,
        }
    }

    fn host() -> HostStats {
        HostStats {
            cpu_percent: 1.0,
            mem_used_bytes: 1,
            mem_total_bytes: 2,
            disk_used_percent: 10,
            uptime_secs: 100,
        }
    }

    fn deployment(id: &str) -> Deployment {
        Deployment {
            id: id.into(),
            project: "a".into(),
            git_ref: "main".into(),
            commit_sha: None,
            status: DeploymentStatus::Success,
            started_at: 1,
            finished_at: Some(2),
            log_tail: String::new(),
        }
    }

    #[tokio::test]
    async fn all_projects_report_with_last_deploy_filled() {
        let mut projects = MockProjectRepository::new();
        projects
            .expect_list()
            .returning(|| Ok(vec![registered("a"), registered("b")]));
        let mut provider = MockStatsProvider::new();
        provider
            .expect_report()
            .withf(|names| names == &["a".to_string(), "b".to_string()])
            .returning(|names| {
                Ok(StatsReport {
                    host: host(),
                    projects: names
                        .into_iter()
                        .map(|project| ProjectStats {
                            project,
                            services: vec![],
                            last_deploy: None,
                        })
                        .collect(),
                })
            });
        let mut history = MockDeploymentHistory::new();
        history
            .expect_latest()
            .withf(|p| p == "a")
            .returning(|_| Ok(Some(deployment("d1"))));
        history
            .expect_latest()
            .withf(|p| p == "b")
            .returning(|_| Ok(None));

        let report = GetStats::new(Arc::new(projects), Arc::new(history), Arc::new(provider))
            .execute(None)
            .await
            .unwrap();
        assert_eq!(report.projects[0].last_deploy.as_ref().unwrap().id, "d1");
        assert!(report.projects[1].last_deploy.is_none());
    }

    #[tokio::test]
    async fn single_project_filter_and_unknown_is_not_found() {
        let mut projects = MockProjectRepository::new();
        projects
            .expect_get()
            .withf(|n| n == "a")
            .returning(|_| Ok(Some(registered("a"))));
        projects
            .expect_get()
            .withf(|n| n == "ghost")
            .returning(|_| Ok(None));
        let mut provider = MockStatsProvider::new();
        provider
            .expect_report()
            .withf(|names| names == &["a".to_string()])
            .returning(|names| {
                Ok(StatsReport {
                    host: host(),
                    projects: names
                        .into_iter()
                        .map(|project| ProjectStats {
                            project,
                            services: vec![],
                            last_deploy: None,
                        })
                        .collect(),
                })
            });
        let mut history = MockDeploymentHistory::new();
        history.expect_latest().returning(|_| Ok(None));

        let get = GetStats::new(Arc::new(projects), Arc::new(history), Arc::new(provider));
        assert_eq!(get.execute(Some("a")).await.unwrap().projects.len(), 1);
        assert!(matches!(
            get.execute(Some("ghost")).await.unwrap_err(),
            DomainError::NotFound(_)
        ));
    }
}
```

В `crates/application/src/lib.rs` добавить `pub mod stats;` (после `pub mod scheduler;`).

Run: `rtk cargo test -p pi-application stats` → FAIL.

- [ ] **Step 2: Реализовать**

В начало `crates/application/src/stats.rs`:

```rust
use std::sync::Arc;

use pi_domain::contracts::{DeploymentHistory, ProjectRepository, StatsProvider};
use pi_domain::entities::StatsReport;
use pi_domain::error::DomainError;

/// `pi stats [project]` (§7 GetStats): provider metrics + last deploy
/// from history (v0.4 design §5).
pub struct GetStats {
    projects: Arc<dyn ProjectRepository>,
    history: Arc<dyn DeploymentHistory>,
    provider: Arc<dyn StatsProvider>,
}

impl GetStats {
    pub fn new(
        projects: Arc<dyn ProjectRepository>,
        history: Arc<dyn DeploymentHistory>,
        provider: Arc<dyn StatsProvider>,
    ) -> Arc<GetStats> {
        Arc::new(GetStats {
            projects,
            history,
            provider,
        })
    }

    pub async fn execute(&self, project: Option<&str>) -> Result<StatsReport, DomainError> {
        let names: Vec<String> = match project {
            Some(name) => {
                self.projects
                    .get(name)
                    .await?
                    .ok_or_else(|| DomainError::NotFound(format!("project '{name}'")))?;
                vec![name.to_string()]
            }
            None => self
                .projects
                .list()
                .await?
                .into_iter()
                .map(|p| p.config.name)
                .collect(),
        };
        let mut report = self.provider.report(names).await?;
        for stats in &mut report.projects {
            stats.last_deploy = self.history.latest(&stats.project).await?;
        }
        Ok(report)
    }
}
```

Run: `rtk cargo test -p pi-application stats` → PASS.

- [ ] **Step 3: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(app): GetStats use-case joining provider metrics with history"
```

---

### Task 10: Application — `ControlLifecycle`

**Files:**
- Create: `crates/application/src/lifecycle.rs`
- Modify: `crates/application/src/lib.rs`

- [ ] **Step 1: Падающие тесты**

Создать `crates/application/src/lifecycle.rs` с тестами:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CollectSink;
    use pi_domain::contracts::{MockContainerRuntime, MockProjectRepository};
    use pi_domain::entities::{LifecycleAction, Project, ProjectConfig};
    use pi_domain::error::DomainError;
    use std::sync::Arc;

    fn registered(name: &str) -> Project {
        Project {
            config: ProjectConfig {
                name: name.into(),
                repo: "r".into(),
                branch: "main".into(),
                compose_path: "docker-compose.yml".into(),
                service: "web".into(),
                container_port: 3000,
                hostname: None,
                healthcheck: Default::default(),
                timeouts: Default::default(),
            },
            host_port: 8000,
            created_at: 0,
        }
    }

    #[tokio::test]
    async fn unknown_project_is_not_found_and_runtime_untouched() {
        let mut projects = MockProjectRepository::new();
        projects.expect_get().returning(|_| Ok(None));
        let mut runtime = MockContainerRuntime::new();
        runtime.expect_lifecycle().times(0);

        let err = ControlLifecycle::new(Arc::new(projects), Arc::new(runtime))
            .execute("ghost", LifecycleAction::Start, CollectSink::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::NotFound(_)));
    }

    #[tokio::test]
    async fn action_is_forwarded_to_runtime() {
        let mut projects = MockProjectRepository::new();
        projects
            .expect_get()
            .returning(|_| Ok(Some(registered("rateme"))));
        let mut runtime = MockContainerRuntime::new();
        runtime
            .expect_lifecycle()
            .withf(|p, a, _| p == "rateme" && *a == LifecycleAction::Restart)
            .times(1)
            .returning(|_, _, _| Ok(()));

        ControlLifecycle::new(Arc::new(projects), Arc::new(runtime))
            .execute("rateme", LifecycleAction::Restart, CollectSink::new())
            .await
            .unwrap();
    }
}
```

В `crates/application/src/lib.rs` добавить `pub mod lifecycle;` (после `pub mod gc;`).

Run: `rtk cargo test -p pi-application lifecycle` → FAIL.

- [ ] **Step 2: Реализовать**

В начало `crates/application/src/lifecycle.rs`:

```rust
use std::sync::Arc;

use pi_domain::contracts::{ContainerRuntime, LogSink, ProjectRepository};
use pi_domain::entities::LifecycleAction;
use pi_domain::error::DomainError;

/// `pi start|stop|restart` (§7 ControlLifecycle): compose-level lifecycle,
/// no rebuild, no history record — this is not a deploy (v0.4 design §5).
pub struct ControlLifecycle {
    projects: Arc<dyn ProjectRepository>,
    runtime: Arc<dyn ContainerRuntime>,
}

impl ControlLifecycle {
    pub fn new(
        projects: Arc<dyn ProjectRepository>,
        runtime: Arc<dyn ContainerRuntime>,
    ) -> Arc<ControlLifecycle> {
        Arc::new(ControlLifecycle { projects, runtime })
    }

    pub async fn execute(
        &self,
        project: &str,
        action: LifecycleAction,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError> {
        self.projects
            .get(project)
            .await?
            .ok_or_else(|| DomainError::NotFound(format!("project '{project}'")))?;
        self.runtime.lifecycle(project, action, log).await
    }
}
```

Run: `rtk cargo test -p pi-application lifecycle` → PASS.

- [ ] **Step 3: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(app): ControlLifecycle use-case"
```

---

### Task 11: Application — `RemoveProject`

**Files:**
- Create: `crates/application/src/remove.rs`
- Modify: `crates/application/src/lib.rs`

- [ ] **Step 1: Падающие тесты**

Создать `crates/application/src/remove.rs` с тестами:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CollectSink;
    use pi_domain::contracts::{
        MockContainerRuntime, MockDeploymentHistory, MockIngress, MockOverrideStore,
        MockProjectRepository, MockSecretStore, MockSource,
    };
    use pi_domain::entities::{Deployment, DeploymentStatus, Project, ProjectConfig};
    use pi_domain::error::DomainError;
    use std::sync::Arc;

    fn registered(name: &str, hostname: Option<&str>) -> Project {
        Project {
            config: ProjectConfig {
                name: name.into(),
                repo: "r".into(),
                branch: "main".into(),
                compose_path: "docker-compose.yml".into(),
                service: "web".into(),
                container_port: 3000,
                hostname: hostname.map(str::to_string),
                healthcheck: Default::default(),
                timeouts: Default::default(),
            },
            host_port: 8000,
            created_at: 0,
        }
    }

    fn active_row() -> Deployment {
        Deployment {
            id: "d1".into(),
            project: "rateme".into(),
            git_ref: "main".into(),
            commit_sha: None,
            status: DeploymentStatus::Running,
            started_at: 1,
            finished_at: None,
            log_tail: String::new(),
        }
    }

    struct Mocks {
        projects: MockProjectRepository,
        history: MockDeploymentHistory,
        runtime: MockContainerRuntime,
        ingress: MockIngress,
        source: MockSource,
        secrets: MockSecretStore,
        overrides: MockOverrideStore,
    }

    impl Mocks {
        fn new() -> Mocks {
            Mocks {
                projects: MockProjectRepository::new(),
                history: MockDeploymentHistory::new(),
                runtime: MockContainerRuntime::new(),
                ingress: MockIngress::new(),
                source: MockSource::new(),
                secrets: MockSecretStore::new(),
                overrides: MockOverrideStore::new(),
            }
        }

        fn build(self) -> Arc<RemoveProject> {
            RemoveProject::new(
                Arc::new(self.projects),
                Arc::new(self.history),
                Arc::new(self.runtime),
                Arc::new(self.ingress),
                Arc::new(self.source),
                Arc::new(self.secrets),
                Arc::new(self.overrides),
            )
        }
    }

    #[tokio::test]
    async fn unknown_project_is_not_found() {
        let mut m = Mocks::new();
        m.projects.expect_get().returning(|_| Ok(None));
        m.runtime.expect_down().times(0);
        let err = m
            .build()
            .execute("ghost", false, CollectSink::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::NotFound(_)));
    }

    #[tokio::test]
    async fn active_deployment_is_conflict_and_nothing_is_removed() {
        let mut m = Mocks::new();
        m.projects
            .expect_get()
            .returning(|_| Ok(Some(registered("rateme", None))));
        m.history
            .expect_active()
            .returning(|_| Ok(vec![active_row()]));
        m.runtime.expect_down().times(0);
        m.projects.expect_remove().times(0);

        let err = m
            .build()
            .execute("rateme", false, CollectSink::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Conflict(_)), "got: {err}");
        assert!(err.to_string().contains("--cancel"), "hint in message");
    }

    #[tokio::test]
    async fn happy_path_removes_everything_and_reports_hostname() {
        let mut m = Mocks::new();
        m.projects
            .expect_get()
            .returning(|_| Ok(Some(registered("rateme", Some("rateme.example.com")))));
        m.history.expect_active().returning(|_| Ok(vec![]));
        m.runtime
            .expect_down()
            .withf(|p, volumes, _| p == "rateme" && *volumes)
            .times(1)
            .returning(|_, _, _| Ok(()));
        m.ingress
            .expect_remove()
            .withf(|h, _| h == "rateme.example.com")
            .times(1)
            .returning(|_, _| Ok(()));
        m.source
            .expect_cleanup()
            .times(1)
            .returning(|_| Ok(()));
        m.secrets.expect_remove().times(1).returning(|_| Ok(()));
        m.overrides.expect_remove().times(1).returning(|_| Ok(()));
        m.history
            .expect_remove_project()
            .times(1)
            .returning(|_| Ok(()));
        m.projects.expect_remove().times(1).returning(|_| Ok(()));

        let report = m
            .build()
            .execute("rateme", true, CollectSink::new())
            .await
            .unwrap();
        assert_eq!(report.hostname.as_deref(), Some("rateme.example.com"));
        assert!(report.volumes_removed);
    }

    #[tokio::test]
    async fn no_hostname_skips_ingress() {
        let mut m = Mocks::new();
        m.projects
            .expect_get()
            .returning(|_| Ok(Some(registered("rateme", None))));
        m.history.expect_active().returning(|_| Ok(vec![]));
        m.runtime.expect_down().returning(|_, _, _| Ok(()));
        m.ingress.expect_remove().times(0);
        m.source.expect_cleanup().returning(|_| Ok(()));
        m.secrets.expect_remove().returning(|_| Ok(()));
        m.overrides.expect_remove().returning(|_| Ok(()));
        m.history.expect_remove_project().returning(|_| Ok(()));
        m.projects.expect_remove().returning(|_| Ok(()));

        let report = m
            .build()
            .execute("rateme", false, CollectSink::new())
            .await
            .unwrap();
        assert!(report.hostname.is_none());
        assert!(!report.volumes_removed);
    }
}
```

В `crates/application/src/lib.rs` добавить `pub mod remove;` (после `pub mod mask;`).

Run: `rtk cargo test -p pi-application remove` → FAIL.

- [ ] **Step 2: Реализовать**

В начало `crates/application/src/remove.rs`:

```rust
use std::sync::Arc;

use pi_domain::contracts::{
    ContainerRuntime, DeploymentHistory, Ingress, LogSink, OverrideStore, ProjectRepository,
    SecretStore, Source,
};
use pi_domain::error::DomainError;

/// What `pi rm` should tell the user afterwards (v0.4 design §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveReport {
    /// Set when an ingress route existed: its DNS record survives and must be
    /// deleted in the Cloudflare dashboard (§11).
    pub hostname: Option<String>,
    pub volumes_removed: bool,
}

/// `pi rm <project>` (§7 RemoveProject). Every step is idempotent, so a retry
/// after a mid-way failure finishes the job.
pub struct RemoveProject {
    projects: Arc<dyn ProjectRepository>,
    history: Arc<dyn DeploymentHistory>,
    runtime: Arc<dyn ContainerRuntime>,
    ingress: Arc<dyn Ingress>,
    source: Arc<dyn Source>,
    secrets: Arc<dyn SecretStore>,
    overrides: Arc<dyn OverrideStore>,
}

impl RemoveProject {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        projects: Arc<dyn ProjectRepository>,
        history: Arc<dyn DeploymentHistory>,
        runtime: Arc<dyn ContainerRuntime>,
        ingress: Arc<dyn Ingress>,
        source: Arc<dyn Source>,
        secrets: Arc<dyn SecretStore>,
        overrides: Arc<dyn OverrideStore>,
    ) -> Arc<RemoveProject> {
        Arc::new(RemoveProject {
            projects,
            history,
            runtime,
            ingress,
            source,
            secrets,
            overrides,
        })
    }

    pub async fn execute(
        &self,
        name: &str,
        remove_volumes: bool,
        log: Arc<dyn LogSink>,
    ) -> Result<RemoveReport, DomainError> {
        let project = self
            .projects
            .get(name)
            .await?
            .ok_or_else(|| DomainError::NotFound(format!("project '{name}'")))?;
        if !self.history.active(name).await?.is_empty() {
            return Err(DomainError::Conflict(format!(
                "project '{name}' has an active deployment; cancel it first (`pi deploy --cancel`)"
            )));
        }

        self.runtime
            .down(name, remove_volumes, Arc::clone(&log))
            .await?;
        if let Some(hostname) = &project.config.hostname {
            self.ingress.remove(hostname, Arc::clone(&log)).await?;
        }
        self.source.cleanup(name).await?;
        self.secrets.remove(name).await?;
        self.overrides.remove(name).await?;
        self.history.remove_project(name).await?;
        self.projects.remove(name).await?;
        log.line(&format!("project '{name}' removed"));
        Ok(RemoveReport {
            hostname: project.config.hostname.clone(),
            volumes_removed: remove_volumes,
        })
    }
}
```

Run: `rtk cargo test -p pi-application remove` → PASS.

- [ ] **Step 3: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(app): RemoveProject use-case with idempotent teardown"
```

---

### Task 12: `SystemProbe` + `HostSystemProbe`

**Files:**
- Modify: `crates/domain/src/contracts.rs`
- Create: `crates/infrastructure/src/probe.rs`
- Modify: `crates/infrastructure/src/lib.rs`

- [ ] **Step 1: Контракт**

В `crates/domain/src/contracts.rs` добавить (импорт `DiagnosticCheck`):

```rust
/// Environment checks for `pi doctor` (§6, §14). Failures are FAIL checks
/// with hints, not errors — the report always renders.
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait SystemProbe: Send + Sync {
    async fn run_checks(&self) -> Vec<DiagnosticCheck>;
}
```

- [ ] **Step 2: Падающие тесты HostSystemProbe**

Создать `crates/infrastructure/src/probe.rs` с тестами:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct FakeRunner(HashMap<String, Result<String, String>>);

    fn key(program: &str, args: &[String]) -> String {
        format!("{program} {}", args.join(" "))
    }

    #[async_trait]
    impl ProbeRunner for FakeRunner {
        async fn run(&self, program: &str, args: &[String]) -> Result<String, String> {
            self.0
                .get(&key(program, args))
                .cloned()
                .unwrap_or_else(|| Err(format!("unexpected command: {}", key(program, args))))
        }
    }

    fn all_ok() -> HashMap<String, Result<String, String>> {
        let mut map = HashMap::new();
        map.insert(
            "docker info --format {{.ServerVersion}}".to_string(),
            Ok("27.0.3".to_string()),
        );
        map.insert(
            "docker compose version".to_string(),
            Ok("Docker Compose version v2.27.0".to_string()),
        );
        map.insert("id -nG".to_string(), Ok("pi-agent docker".to_string()));
        map.insert(
            "cloudflared --version".to_string(),
            Ok("cloudflared version 2026.1.0".to_string()),
        );
        map.insert(
            "systemctl --user is-active cloudflared".to_string(),
            Ok("active".to_string()),
        );
        map.insert("id -un".to_string(), Ok("pi-agent".to_string()));
        map.insert(
            "loginctl show-user pi-agent --property=Linger".to_string(),
            Ok("Linger=yes".to_string()),
        );
        map
    }

    #[tokio::test]
    async fn all_checks_pass_on_healthy_host_with_cloudflared() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cert.pem"), "cert").unwrap();
        let probe = HostSystemProbe::new(Arc::new(FakeRunner(all_ok())), dir.path(), true);

        let checks = probe.run_checks().await;
        assert_eq!(checks.len(), 7, "{checks:?}");
        assert!(checks.iter().all(|c| c.passed), "{checks:?}");
    }

    #[tokio::test]
    async fn failures_carry_hints() {
        let mut map = all_ok();
        map.insert(
            "docker info --format {{.ServerVersion}}".to_string(),
            Err("Cannot connect to the Docker daemon".to_string()),
        );
        map.insert("id -nG".to_string(), Ok("pi-agent".to_string())); // no docker group
        let dir = tempfile::tempdir().unwrap(); // no cert.pem
        let probe = HostSystemProbe::new(Arc::new(FakeRunner(map)), dir.path(), true);

        let checks = probe.run_checks().await;
        let daemon = checks.iter().find(|c| c.name == "docker daemon").unwrap();
        assert!(!daemon.passed);
        assert!(daemon.hint.is_some());
        let group = checks.iter().find(|c| c.name == "docker group").unwrap();
        assert!(!group.passed, "{group:?}");
        let cert = checks.iter().find(|c| c.name == "cloudflared cert").unwrap();
        assert!(!cert.passed);
    }

    #[tokio::test]
    async fn without_cloudflared_only_docker_checks_run() {
        let dir = tempfile::tempdir().unwrap();
        let probe = HostSystemProbe::new(Arc::new(FakeRunner(all_ok())), dir.path(), false);
        let checks = probe.run_checks().await;
        assert_eq!(checks.len(), 3, "{checks:?}");
        assert!(checks.iter().all(|c| !c.name.contains("cloudflared")));
    }
}
```

В `crates/infrastructure/src/lib.rs` добавить `pub mod probe;` (после `pub mod process;`).

Run: `rtk cargo test -p pi-infrastructure probe` → FAIL.

- [ ] **Step 3: Реализовать**

В начало `crates/infrastructure/src/probe.rs`:

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::SystemProbe;
use pi_domain::entities::DiagnosticCheck;
use tokio::process::Command;

use crate::process::run_capture;

/// Shell-out boundary of HostSystemProbe; injectable so the checks unit-test
/// on Windows with canned outputs.
#[async_trait]
pub trait ProbeRunner: Send + Sync {
    /// Ok(stdout) on zero exit, Err(message) otherwise.
    async fn run(&self, program: &str, args: &[String]) -> Result<String, String>;
}

pub struct SystemRunner;

impl SystemRunner {
    pub fn new() -> Arc<SystemRunner> {
        Arc::new(SystemRunner)
    }
}

#[async_trait]
impl ProbeRunner for SystemRunner {
    async fn run(&self, program: &str, args: &[String]) -> Result<String, String> {
        let mut cmd = Command::new(program);
        cmd.args(args);
        run_capture(cmd).await
    }
}

/// `pi doctor` host checks (§14). The disk check is NOT here — RunDiagnostics
/// adds it through the existing DiskProbe (v0.4 design §6).
pub struct HostSystemProbe {
    runner: Arc<dyn ProbeRunner>,
    data_dir: PathBuf,
    cloudflared: bool,
}

impl HostSystemProbe {
    pub fn new(
        runner: Arc<dyn ProbeRunner>,
        data_dir: &Path,
        cloudflared: bool,
    ) -> Arc<HostSystemProbe> {
        Arc::new(HostSystemProbe {
            runner,
            data_dir: data_dir.to_path_buf(),
            cloudflared,
        })
    }

    async fn command_check(
        &self,
        name: &str,
        program: &str,
        args: &[&str],
        hint: &str,
    ) -> DiagnosticCheck {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        match self.runner.run(program, &args).await {
            Ok(out) => DiagnosticCheck {
                name: name.to_string(),
                passed: true,
                detail: out.lines().next().unwrap_or("").to_string(),
                hint: None,
            },
            Err(e) => DiagnosticCheck {
                name: name.to_string(),
                passed: false,
                detail: e,
                hint: Some(hint.to_string()),
            },
        }
    }

    async fn docker_group_check(&self) -> DiagnosticCheck {
        let name = "docker group".to_string();
        let hint = "usermod -aG docker pi-agent, then restart the agent service";
        match self.runner.run("id", &["-nG".to_string()]).await {
            Ok(groups) if groups.split_whitespace().any(|g| g == "docker") => DiagnosticCheck {
                name,
                passed: true,
                detail: "agent user is in the docker group".to_string(),
                hint: None,
            },
            Ok(groups) => DiagnosticCheck {
                name,
                passed: false,
                detail: format!("groups: {groups}"),
                hint: Some(hint.to_string()),
            },
            Err(e) => DiagnosticCheck {
                name,
                passed: false,
                detail: e,
                hint: Some(hint.to_string()),
            },
        }
    }

    async fn linger_check(&self) -> DiagnosticCheck {
        let name = "systemd linger".to_string();
        let hint = "loginctl enable-linger <agent user> (needed to restart cloudflared without sudo)";
        let user = match self.runner.run("id", &["-un".to_string()]).await {
            Ok(user) => user.trim().to_string(),
            Err(e) => {
                return DiagnosticCheck {
                    name,
                    passed: false,
                    detail: e,
                    hint: Some(hint.to_string()),
                }
            }
        };
        let args = vec![
            "show-user".to_string(),
            user,
            "--property=Linger".to_string(),
        ];
        match self.runner.run("loginctl", &args).await {
            Ok(out) if out.contains("Linger=yes") => DiagnosticCheck {
                name,
                passed: true,
                detail: "linger enabled".to_string(),
                hint: None,
            },
            Ok(out) => DiagnosticCheck {
                name,
                passed: false,
                detail: out.trim().to_string(),
                hint: Some(hint.to_string()),
            },
            Err(e) => DiagnosticCheck {
                name,
                passed: false,
                detail: e,
                hint: Some(hint.to_string()),
            },
        }
    }

    fn cert_check(&self) -> DiagnosticCheck {
        let path = self.data_dir.join("cert.pem");
        DiagnosticCheck {
            name: "cloudflared cert".to_string(),
            passed: path.exists(),
            detail: path.display().to_string(),
            hint: (!path.exists()).then(|| {
                "run `cloudflared tunnel login` as the agent user (docs/install-agent-v0.1.md)"
                    .to_string()
            }),
        }
    }
}

#[async_trait]
impl SystemProbe for HostSystemProbe {
    async fn run_checks(&self) -> Vec<DiagnosticCheck> {
        let mut checks = vec![
            self.command_check(
                "docker daemon",
                "docker",
                &["info", "--format", "{{.ServerVersion}}"],
                "install docker and start the daemon; check `systemctl status docker`",
            )
            .await,
            self.command_check(
                "docker compose",
                "docker",
                &["compose", "version"],
                "install the docker compose v2 plugin",
            )
            .await,
            self.docker_group_check().await,
        ];
        if self.cloudflared {
            checks.push(
                self.command_check(
                    "cloudflared binary",
                    "cloudflared",
                    &["--version"],
                    "install cloudflared (docs/install-agent-v0.1.md)",
                )
                .await,
            );
            checks.push(
                self.command_check(
                    "cloudflared unit",
                    "systemctl",
                    &["--user", "is-active", "cloudflared"],
                    "systemctl --user start cloudflared; check linger",
                )
                .await,
            );
            checks.push(self.linger_check().await);
            checks.push(self.cert_check());
        }
        checks
    }
}
```

Run: `rtk cargo test -p pi-infrastructure probe` → PASS.

- [ ] **Step 4: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(infra): HostSystemProbe doctor checks with injectable runner"
```

---

### Task 13: Application — `RunDiagnostics` + `AgentStatus`

**Files:**
- Create: `crates/application/src/diagnostics.rs`
- Modify: `crates/application/src/lib.rs`

- [ ] **Step 1: Падающие тесты**

Создать `crates/application/src/diagnostics.rs` с тестами:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use pi_domain::contracts::{
        MockClock, MockDeploymentHistory, MockDiskProbe, MockProjectRepository, MockSystemProbe,
    };
    use pi_domain::entities::{Deployment, DeploymentStatus, DiagnosticCheck, Project, ProjectConfig};
    use std::sync::Arc;

    fn pass(name: &str) -> DiagnosticCheck {
        DiagnosticCheck {
            name: name.into(),
            passed: true,
            detail: String::new(),
            hint: None,
        }
    }

    fn registered(name: &str) -> Project {
        Project {
            config: ProjectConfig {
                name: name.into(),
                repo: "r".into(),
                branch: "main".into(),
                compose_path: "docker-compose.yml".into(),
                service: "web".into(),
                container_port: 3000,
                hostname: None,
                healthcheck: Default::default(),
                timeouts: Default::default(),
            },
            host_port: 8000,
            created_at: 0,
        }
    }

    fn active_row(project: &str) -> Deployment {
        Deployment {
            id: "d1".into(),
            project: project.into(),
            git_ref: "main".into(),
            commit_sha: None,
            status: DeploymentStatus::Running,
            started_at: 1,
            finished_at: None,
            log_tail: String::new(),
        }
    }

    #[tokio::test]
    async fn diagnostics_append_disk_check_with_threshold() {
        let mut probe = MockSystemProbe::new();
        probe
            .expect_run_checks()
            .returning(|| vec![pass("docker daemon")]);
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(90));

        let report = RunDiagnostics::new(Arc::new(probe), Arc::new(disk), 85)
            .execute()
            .await;
        assert_eq!(report.checks.len(), 2);
        let disk_check = report.checks.last().unwrap();
        assert!(!disk_check.passed, "90% >= 85% threshold");
        assert!(disk_check.detail.contains("90"), "{disk_check:?}");
        assert!(disk_check.hint.as_deref().unwrap_or("").contains("pi gc"));
        assert!(!report.all_passed());
    }

    #[tokio::test]
    async fn disk_below_threshold_passes() {
        let mut probe = MockSystemProbe::new();
        probe.expect_run_checks().returning(Vec::new);
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(40));

        let report = RunDiagnostics::new(Arc::new(probe), Arc::new(disk), 85)
            .execute()
            .await;
        assert!(report.all_passed());
    }

    #[tokio::test]
    async fn agent_status_aggregates_overview() {
        let mut projects = MockProjectRepository::new();
        projects
            .expect_list()
            .returning(|| Ok(vec![registered("a"), registered("b")]));
        let mut history = MockDeploymentHistory::new();
        history
            .expect_active()
            .withf(|p| p == "a")
            .returning(|p| Ok(vec![active_row(p)]));
        history
            .expect_active()
            .withf(|p| p == "b")
            .returning(|_| Ok(vec![]));
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(33));
        let mut clock = MockClock::new();
        clock.expect_now_unix().returning(|| 150);

        let overview = AgentStatus::new(
            Arc::new(projects),
            Arc::new(history),
            Arc::new(disk),
            Arc::new(clock),
            "0.4.0".to_string(),
            100,
        )
        .execute()
        .await
        .unwrap();
        assert_eq!(overview.version, "0.4.0");
        assert_eq!(overview.uptime_secs, 50);
        assert_eq!(overview.disk_used_percent, 33);
        assert_eq!(overview.projects, 2);
        assert_eq!(overview.active_deployments, 1);
    }
}
```

В `crates/application/src/lib.rs` добавить `pub mod diagnostics;` (после `pub mod deploy;`).

Run: `rtk cargo test -p pi-application diagnostics` → FAIL.

- [ ] **Step 2: Реализовать**

В начало `crates/application/src/diagnostics.rs`:

```rust
use std::sync::Arc;

use pi_domain::contracts::{Clock, DeploymentHistory, DiskProbe, ProjectRepository, SystemProbe};
use pi_domain::entities::{AgentOverview, DiagnosticCheck, DiagnosticReport};
use pi_domain::error::DomainError;

/// `pi doctor` (§7 RunDiagnostics): SystemProbe checks + the disk check via
/// the GC DiskProbe and its threshold (v0.4 design §5).
pub struct RunDiagnostics {
    probe: Arc<dyn SystemProbe>,
    disk: Arc<dyn DiskProbe>,
    disk_threshold_percent: u8,
}

impl RunDiagnostics {
    pub fn new(
        probe: Arc<dyn SystemProbe>,
        disk: Arc<dyn DiskProbe>,
        disk_threshold_percent: u8,
    ) -> Arc<RunDiagnostics> {
        Arc::new(RunDiagnostics {
            probe,
            disk,
            disk_threshold_percent,
        })
    }

    pub async fn execute(&self) -> DiagnosticReport {
        let mut checks = self.probe.run_checks().await;
        checks.push(self.disk_check());
        DiagnosticReport { checks }
    }

    fn disk_check(&self) -> DiagnosticCheck {
        let name = "disk space".to_string();
        let threshold = self.disk_threshold_percent;
        match self.disk.used_percent() {
            Ok(used) => DiagnosticCheck {
                name,
                passed: used < threshold,
                detail: format!("{used}% used (threshold {threshold}%)"),
                hint: (used >= threshold)
                    .then(|| "run `pi gc`; consider a larger SD card".to_string()),
            },
            Err(e) => DiagnosticCheck {
                name,
                passed: false,
                detail: e.to_string(),
                hint: Some("check the agent data_dir filesystem".to_string()),
            },
        }
    }
}

/// `pi status` / GET /v1/status (v0.4 design §5).
pub struct AgentStatus {
    projects: Arc<dyn ProjectRepository>,
    history: Arc<dyn DeploymentHistory>,
    disk: Arc<dyn DiskProbe>,
    clock: Arc<dyn Clock>,
    version: String,
    started_at: i64,
}

impl AgentStatus {
    pub fn new(
        projects: Arc<dyn ProjectRepository>,
        history: Arc<dyn DeploymentHistory>,
        disk: Arc<dyn DiskProbe>,
        clock: Arc<dyn Clock>,
        version: String,
        started_at: i64,
    ) -> Arc<AgentStatus> {
        Arc::new(AgentStatus {
            projects,
            history,
            disk,
            clock,
            version,
            started_at,
        })
    }

    pub async fn execute(&self) -> Result<AgentOverview, DomainError> {
        let projects = self.projects.list().await?;
        let mut active = 0usize;
        for project in &projects {
            active += self.history.active(&project.config.name).await?.len();
        }
        Ok(AgentOverview {
            version: self.version.clone(),
            uptime_secs: (self.clock.now_unix() - self.started_at).max(0) as u64,
            disk_used_percent: self.disk.used_percent().unwrap_or(0),
            projects: projects.len(),
            active_deployments: active,
        })
    }
}
```

Run: `rtk cargo test -p pi-application diagnostics` → PASS.

- [ ] **Step 3: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(app): RunDiagnostics and AgentStatus use-cases"
```

---

### Task 14: Bin — rolling-логи агента (`[logs]`, tracing-appender, logfile.rs)

**Files:**
- Modify: `Cargo.toml` (workspace: +`tracing-appender`, +`time`)
- Modify: `crates/bin/Cargo.toml`
- Modify: `crates/bin/src/duration.rs` (суффикс `h`)
- Modify: `crates/bin/src/agent/config.rs` (`[logs]`)
- Create: `crates/bin/src/agent/logfile.rs`
- Modify: `crates/bin/src/agent/mod.rs`
- Modify: `crates/bin/src/agent/run.rs`
- Modify: `crates/bin/src/main.rs` (реструктуризация tracing-инициализации)

- [ ] **Step 1: Зависимости**

Workspace `Cargo.toml` `[workspace.dependencies]`:

```toml
tracing-appender = "0.2"
time = { version = "0.3", features = ["formatting", "parsing"] }
```

`crates/bin/Cargo.toml` `[dependencies]`: добавить `tracing-appender = { workspace = true }` и `time = { workspace = true }`.

- [ ] **Step 2: Суффикс `h` в parse_duration_secs (падающий тест)**

В `crates/bin/src/duration.rs` обновить тест:

```rust
    #[test]
    fn parse_duration_secs_supports_h_m_s_and_bare_numbers() {
        assert_eq!(parse_duration_secs("60s").unwrap(), 60);
        assert_eq!(parse_duration_secs("2m").unwrap(), 120);
        assert_eq!(parse_duration_secs("2h").unwrap(), 7200);
        assert_eq!(parse_duration_secs("90").unwrap(), 90);
        assert!(parse_duration_secs("soon").is_err());
    }
```

(старый тест `parse_duration_secs_supports_s_m_and_bare_numbers` заменить этим.)

Run: `rtk cargo test -p pi duration` → FAIL. Реализация — в начало цепочки `if`:

```rust
    let (digits, mult) = if let Some(d) = s.strip_suffix('h') {
        (d, 3600)
    } else if let Some(d) = s.strip_suffix('m') {
        (d, 60)
    } else if let Some(d) = s.strip_suffix('s') {
        (d, 1)
    } else {
        (s, 1)
    };
```

Обновить текст ошибки: `(expected like "60s", "2m" or "1h")`. Run → PASS.

- [ ] **Step 3: `[logs]` в AgentConfig (падающий тест)**

В `mod tests` файла `crates/bin/src/agent/config.rs`:

```rust
    #[test]
    fn logs_section_defaults_and_overrides() {
        let config = AgentConfig::parse("").unwrap();
        assert_eq!(config.logs.dir, std::path::PathBuf::from("/var/log/pi"));
        assert_eq!(config.logs.retention_days, 14);

        let config =
            AgentConfig::parse("[logs]\ndir = \".dev-logs\"\nretention_days = 3").unwrap();
        assert_eq!(config.logs.dir, std::path::PathBuf::from(".dev-logs"));
        assert_eq!(config.logs.retention_days, 3);
    }
```

Run: `rtk cargo test -p pi agent::config` → FAIL. Реализация — в struct `AgentConfig` добавить:

```rust
    #[serde(default)]
    pub logs: LogsSection,
```

И секцию:

```rust
/// [logs] in agent.toml (§14): rolling file logs of the agent itself.
#[derive(Debug, Deserialize)]
pub struct LogsSection {
    #[serde(default = "default_log_dir")]
    pub dir: PathBuf,
    #[serde(default = "default_log_retention_days")]
    pub retention_days: u32,
}

impl Default for LogsSection {
    fn default() -> LogsSection {
        LogsSection {
            dir: default_log_dir(),
            retention_days: default_log_retention_days(),
        }
    }
}

fn default_log_dir() -> PathBuf {
    PathBuf::from("/var/log/pi")
}
fn default_log_retention_days() -> u32 {
    14
}
```

Run → PASS.

- [ ] **Step 4: logfile.rs (падающие тесты)**

Создать `crates/bin/src/agent/logfile.rs` с тестами; в `crates/bin/src/agent/mod.rs` добавить `pub mod logfile;`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, date: &str, content: &str) {
        std::fs::write(dir.join(format!("{LOG_FILE_PREFIX}.{date}")), content).unwrap();
    }

    #[test]
    fn file_date_extracts_suffix() {
        assert_eq!(file_date("pi-agent.log.2026-06-12"), Some("2026-06-12"));
        assert_eq!(file_date("pi-agent.log"), None);
        assert_eq!(file_date("other.txt"), None);
    }

    #[test]
    fn log_files_sorted_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "2026-06-12", "");
        write(dir.path(), "2026-06-10", "");
        std::fs::write(dir.path().join("unrelated.txt"), "").unwrap();
        let files: Vec<String> = log_files(dir.path())
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files, vec!["pi-agent.log.2026-06-10", "pi-agent.log.2026-06-12"]);
    }

    #[test]
    fn tail_lines_spans_files_newest_last() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "2026-06-10", "old-1\nold-2\n");
        write(dir.path(), "2026-06-11", "new-1\nnew-2\n");
        assert_eq!(tail_lines(dir.path(), 3), vec!["old-2", "new-1", "new-2"]);
        assert_eq!(tail_lines(dir.path(), 100).len(), 4);
        assert!(tail_lines(dir.path(), 0).is_empty());
    }

    #[test]
    fn lines_since_filters_by_rfc3339_prefix() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "2026-06-11",
            "2026-06-11T10:00:00.0Z early\n2026-06-11T12:00:00.0Z late\n  continuation line\n",
        );
        let lines = lines_since(dir.path(), "2026-06-11T11:00:00Z");
        assert_eq!(
            lines,
            vec![
                "2026-06-11T12:00:00.0Z late".to_string(),
                "  continuation line".to_string()
            ],
            "continuation lines inherit the verdict of their timestamped line"
        );
    }

    #[test]
    fn prune_old_logs_removes_files_before_cutoff() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "2026-06-01", "");
        write(dir.path(), "2026-06-10", "");
        let removed = prune_old_logs(dir.path(), "2026-06-05");
        assert_eq!(removed, 1);
        assert_eq!(log_files(dir.path()).len(), 1);
    }

    #[test]
    fn unix_to_rfc3339_formats_epoch() {
        assert_eq!(
            unix_to_rfc3339(0).as_deref(),
            Some("1970-01-01T00:00:00Z")
        );
        assert!(unix_to_rfc3339(i64::MAX).is_none());
    }

    #[test]
    fn read_new_lines_tracks_growth_partial_lines_and_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("pi-agent.log.2026-06-10");
        std::fs::write(&old, "a\n").unwrap();

        let pos = follow_pos(dir.path());
        assert_eq!(pos, (old.clone(), 2));

        // growth with a partial trailing line: only complete lines are emitted
        std::fs::write(&old, "a\nb\npart").unwrap();
        let (pos, lines) = read_new_lines(dir.path(), pos);
        assert_eq!(lines, vec!["b".to_string()]);
        assert_eq!(pos.1, 4, "offset stops after the last full line");

        // rotation: remainder of the old file, then the new file from 0
        std::fs::write(&old, "a\nb\npartial-done\n").unwrap();
        let new = dir.path().join("pi-agent.log.2026-06-11");
        std::fs::write(&new, "fresh\n").unwrap();
        let (pos, lines) = read_new_lines(dir.path(), pos);
        assert_eq!(
            lines,
            vec!["partial-done".to_string(), "fresh".to_string()]
        );
        assert_eq!(pos.0, new);
    }
}
```

Run: `rtk cargo test -p pi logfile` → FAIL.

- [ ] **Step 5: Реализовать logfile.rs**

В начало `crates/bin/src/agent/logfile.rs`:

```rust
use std::path::{Path, PathBuf};

/// Rolling file prefix; tracing-appender daily naming is `<prefix>.<YYYY-MM-DD>`.
pub(crate) const LOG_FILE_PREFIX: &str = "pi-agent.log";

/// Extracts the date suffix from a rolling file name.
pub(crate) fn file_date(name: &str) -> Option<&str> {
    name.strip_prefix("pi-agent.log.").filter(|d| d.len() == 10)
}

/// Rolling files of the dir, oldest first (ISO dates sort lexicographically).
pub(crate) fn log_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .and_then(file_date)
                .is_some()
        })
        .collect();
    files.sort();
    files
}

/// Last `n` lines across the rolling files, oldest first.
pub(crate) fn tail_lines(dir: &Path, n: usize) -> Vec<String> {
    let mut rev: Vec<String> = Vec::new();
    for file in log_files(dir).into_iter().rev() {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for line in text.lines().rev() {
            if rev.len() == n {
                break;
            }
            rev.push(line.to_string());
        }
        if rev.len() == n {
            break;
        }
    }
    rev.reverse();
    rev
}

/// Lines at/after the cutoff. tracing lines start with an RFC3339 timestamp;
/// RFC3339 UTC compares lexicographically. Lines without a timestamp
/// (multi-line payloads) inherit the verdict of the previous line.
pub(crate) fn lines_since(dir: &Path, cutoff_rfc3339: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut including = false;
    for file in log_files(dir) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for line in text.lines() {
            if let Some(ts) = line.split_whitespace().next() {
                if looks_like_timestamp(ts) {
                    including = ts >= cutoff_rfc3339;
                }
            }
            if including {
                out.push(line.to_string());
            }
        }
    }
    out
}

pub(crate) fn looks_like_timestamp(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 19 && b[4] == b'-' && b[7] == b'-' && b[10] == b'T'
}

/// Deletes rolling files dated strictly before the cutoff (YYYY-MM-DD).
pub(crate) fn prune_old_logs(dir: &Path, cutoff_date: &str) -> usize {
    let mut removed = 0;
    for file in log_files(dir) {
        let Some(date) = file
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(file_date)
        else {
            continue;
        };
        if date < cutoff_date && std::fs::remove_file(&file).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Cutoff date (YYYY-MM-DD) for retention: today - retention_days.
pub(crate) fn retention_cutoff(retention_days: u32) -> String {
    (time::OffsetDateTime::now_utc() - time::Duration::days(i64::from(retention_days)))
        .date()
        .to_string()
}

pub(crate) fn unix_to_rfc3339(secs: i64) -> Option<String> {
    time::OffsetDateTime::from_unix_timestamp(secs)
        .ok()?
        .format(&time::format_description::well_known::Rfc3339)
        .ok()
}

/// Follow cursor: which file we read and how many bytes of it are consumed.
pub(crate) type FollowPos = (PathBuf, u64);

/// Initial cursor: end of the newest file (follow emits only new lines).
pub(crate) fn follow_pos(dir: &Path) -> FollowPos {
    match log_files(dir).pop() {
        Some(path) => {
            let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            (path, len)
        }
        None => (dir.join(format!("{LOG_FILE_PREFIX}.none")), 0),
    }
}

/// Complete lines appended past `offset`; the new offset stops after the last
/// full line so a partially written line is emitted on the next poll.
fn read_from(path: &Path, offset: u64) -> (Vec<String>, u64) {
    let Ok(bytes) = std::fs::read(path) else {
        return (vec![], offset);
    };
    let start = if (offset as usize) <= bytes.len() {
        offset as usize
    } else {
        0 // file truncated/recreated - start over
    };
    let slice = &bytes[start..];
    let Some(last_newline) = slice.iter().rposition(|b| *b == b'\n') else {
        return (vec![], start as u64);
    };
    let complete = &slice[..=last_newline];
    let lines = String::from_utf8_lossy(complete)
        .lines()
        .map(str::to_string)
        .collect();
    (lines, (start + last_newline + 1) as u64)
}

/// One follow poll: drains the tracked file; on daily rotation flushes the
/// remainder of the old file and switches to the new one.
pub(crate) fn read_new_lines(dir: &Path, pos: FollowPos) -> (FollowPos, Vec<String>) {
    let (path, offset) = pos;
    let Some(newest) = log_files(dir).pop() else {
        return ((path, offset), vec![]);
    };
    if newest == path {
        let (lines, new_offset) = read_from(&path, offset);
        return ((path, new_offset), lines);
    }
    let (mut lines, _) = read_from(&path, offset);
    let (fresh, new_offset) = read_from(&newest, 0);
    lines.extend(fresh);
    ((newest, new_offset), lines)
}
```

Run: `rtk cargo test -p pi logfile` → PASS.

- [ ] **Step 6: Tracing с file-слоем + prune при старте**

`crates/bin/src/main.rs` — вынести инициализацию в функцию и диспетчеризовать agent run до неё:

```rust
fn init_cli_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        // the agent builds its own subscriber (stderr + rolling file, §14)
        Cmd::Agent {
            cmd: AgentCmd::Run { config },
        } => agent::run::run(config).await,
        cmd => {
            init_cli_tracing();
            run_cli(cmd).await
        }
    }
}

async fn run_cli(cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Deploy { git_ref, cancel, connect } => {
            if cancel {
                cli::commands::deploy_cancel(connect).await
            } else {
                cli::commands::deploy(git_ref, connect).await
            }
        }
        Cmd::Ls { connect } => cli::commands::ls(connect).await,
        Cmd::Gc { connect } => cli::commands::gc(connect).await,
        Cmd::Env { cmd: EnvCmd::Send { apply, connect } } => cli::commands::env_send(apply, connect).await,
        Cmd::Env { cmd: EnvCmd::Ls { connect } } => cli::commands::env_ls(connect).await,
        Cmd::Agent { cmd: AgentCmd::Run { .. } } => {
            anyhow::bail!("internal: `agent run` is dispatched in main")
        }
    }
}
```

`crates/bin/src/agent/run.rs` — инициализация tracing с rolling-файлом:

```rust
use crate::agent::config::{AgentConfig, LogsSection};
use crate::agent::logfile;

/// stderr (journald) + rolling file (§14). Returns the effective log dir and
/// the appender guard; None dir when the directory cannot be created — the
/// agent must keep working with stderr only.
fn init_tracing(
    logs: &LogsSection,
) -> (Option<PathBuf>, Option<tracing_appender::non_blocking::WorkerGuard>) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())
    };
    match std::fs::create_dir_all(&logs.dir) {
        Ok(()) => {
            let appender =
                tracing_appender::rolling::daily(&logs.dir, logfile::LOG_FILE_PREFIX);
            let (writer, guard) = tracing_appender::non_blocking(appender);
            tracing_subscriber::registry()
                .with(filter())
                .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(writer),
                )
                .init();
            (Some(logs.dir.clone()), Some(guard))
        }
        Err(e) => {
            tracing_subscriber::fmt()
                .with_env_filter(filter())
                .with_writer(std::io::stderr)
                .init();
            tracing::warn!(
                "cannot create log dir {}: {e}; file logging disabled",
                logs.dir.display()
            );
            (None, None)
        }
    }
}

pub async fn run(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let config = AgentConfig::load(config_path.as_deref())?;
    let (log_dir, _appender_guard) = init_tracing(&config.logs);
    if let Some(dir) = &log_dir {
        let removed =
            logfile::prune_old_logs(dir, &logfile::retention_cutoff(config.logs.retention_days));
        if removed > 0 {
            tracing::info!("pruned {removed} old log file(s) (§14 retention)");
        }
    }
    let state = build_state(&config)?;
    // ... (остальное тело без изменений: свип, router, listen)
}
```

(`build_state` получит `log_dir` в Task 15 — в этом таске сигнатура не меняется.)

- [ ] **Step 7: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS. Дополнительно вручную: `rtk cargo run -p pi -- agent run --config dev/agent.toml` с `[logs] dir = ".dev-logs"` — каталог и файл `pi-agent.log.<дата>` появляются.

```bash
rtk git add -A && rtk git commit -m "feat(bin): rolling agent log files with retention and [logs] config"
```

---

### Task 15: Bin — DTO, wiring и эндпоинты stats/status/doctor/lifecycle/rm

**Files:**
- Modify: `crates/bin/src/proto.rs`
- Modify: `crates/bin/src/agent/state.rs`
- Modify: `crates/bin/src/agent/run.rs` (передать `log_dir` в `build_state`)
- Modify: `crates/bin/src/agent/http.rs`

- [ ] **Step 1: DTO в proto.rs**

В `crates/bin/src/proto.rs` добавить (импорты: `AgentOverview`, `DiagnosticReport`, `StatsReport` в `use pi_domain::entities::{...}`):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatsDto {
    pub service: String,
    pub cpu_percent: f64,
    pub mem_used_bytes: u64,
    pub mem_limit_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectStatsDto {
    pub project: String,
    pub services: Vec<ServiceStatsDto>,
    pub last_deploy: Option<DeploymentDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostStatsDto {
    pub cpu_percent: f64,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
    pub disk_used_percent: u8,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsResponse {
    pub host: HostStatsDto,
    pub projects: Vec<ProjectStatsDto>,
}

impl From<StatsReport> for StatsResponse {
    fn from(r: StatsReport) -> StatsResponse {
        StatsResponse {
            host: HostStatsDto {
                cpu_percent: r.host.cpu_percent,
                mem_used_bytes: r.host.mem_used_bytes,
                mem_total_bytes: r.host.mem_total_bytes,
                disk_used_percent: r.host.disk_used_percent,
                uptime_secs: r.host.uptime_secs,
            },
            projects: r
                .projects
                .into_iter()
                .map(|p| ProjectStatsDto {
                    project: p.project,
                    services: p
                        .services
                        .into_iter()
                        .map(|s| ServiceStatsDto {
                            service: s.service,
                            cpu_percent: s.cpu_percent,
                            mem_used_bytes: s.mem_used_bytes,
                            mem_limit_bytes: s.mem_limit_bytes,
                        })
                        .collect(),
                    last_deploy: p.last_deploy.map(Into::into),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticCheckDto {
    pub name: String,
    pub passed: bool,
    pub detail: String,
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorResponse {
    pub checks: Vec<DiagnosticCheckDto>,
    pub all_passed: bool,
}

impl From<DiagnosticReport> for DoctorResponse {
    fn from(r: DiagnosticReport) -> DoctorResponse {
        DoctorResponse {
            all_passed: r.all_passed(),
            checks: r
                .checks
                .into_iter()
                .map(|c| DiagnosticCheckDto {
                    name: c.name,
                    passed: c.passed,
                    detail: c.detail,
                    hint: c.hint,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatusResponse {
    pub version: String,
    pub uptime_secs: u64,
    pub disk_used_percent: u8,
    pub projects: usize,
    pub active_deployments: usize,
}

impl From<AgentOverview> for AgentStatusResponse {
    fn from(o: AgentOverview) -> AgentStatusResponse {
        AgentStatusResponse {
            version: o.version,
            uptime_secs: o.uptime_secs,
            disk_used_percent: o.disk_used_percent,
            projects: o.projects,
            active_deployments: o.active_deployments,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleRequest {
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveResponse {
    /// Hostname whose DNS record survives `pi rm` (§11) — the CLI prints the
    /// manual deletion instruction from it.
    pub hostname: Option<String>,
    pub volumes_removed: bool,
}
```

В `mod tests` файла proto.rs добавить:

```rust
    #[test]
    fn stats_response_converts_from_report() {
        use pi_domain::entities::{HostStats, ProjectStats, ServiceStats, StatsReport};
        let report = StatsReport {
            host: HostStats {
                cpu_percent: 1.5,
                mem_used_bytes: 10,
                mem_total_bytes: 20,
                disk_used_percent: 30,
                uptime_secs: 40,
            },
            projects: vec![ProjectStats {
                project: "a".into(),
                services: vec![ServiceStats {
                    service: "web".into(),
                    cpu_percent: 0.5,
                    mem_used_bytes: 1,
                    mem_limit_bytes: 2,
                }],
                last_deploy: None,
            }],
        };
        let dto: StatsResponse = report.into();
        assert_eq!(dto.host.disk_used_percent, 30);
        assert_eq!(dto.projects[0].services[0].service, "web");
        assert!(dto.projects[0].last_deploy.is_none());
    }
```

Run: `rtk cargo test -p pi proto` → PASS (после реализации DTO).

- [ ] **Step 2: Wiring в state.rs**

`crates/bin/src/agent/state.rs` — импорты:

```rust
use std::path::PathBuf;

use pi_application::diagnostics::{AgentStatus, RunDiagnostics};
use pi_application::lifecycle::ControlLifecycle;
use pi_application::logs::StreamLogs;
use pi_application::remove::RemoveProject;
use pi_application::stats::GetStats;
use pi_domain::contracts::Clock;
use pi_infrastructure::probe::{HostSystemProbe, SystemRunner};
use pi_infrastructure::stats::CompositeStats;
```

В `AppState` добавить поля:

```rust
    pub stats: Arc<GetStats>,
    pub lifecycle: Arc<ControlLifecycle>,
    pub remove: Arc<RemoveProject>,
    pub diagnostics: Arc<RunDiagnostics>,
    pub overview: Arc<AgentStatus>,
    pub stream_logs: Arc<StreamLogs>,
    /// None when the rolling-file layer could not be initialized — the
    /// /v1/agent/logs endpoint answers 404 with an explanation.
    pub log_dir: Option<PathBuf>,
```

Сигнатура: `pub fn build_state(config: &AgentConfig, log_dir: Option<PathBuf>) -> anyhow::Result<AppState>`. Внутри — `ingress` больше не мувается в `DeployProject` (передавать `Arc::clone(&ingress)`), `disk` клонировать для всех потребителей. После создания `env_keys` добавить:

```rust
    let provider = CompositeStats::new(runtime.clone(), disk.clone());
    let stats = GetStats::new(projects.clone(), Arc::clone(&history), provider);
    let lifecycle = ControlLifecycle::new(projects.clone(), runtime.clone());
    let remove = RemoveProject::new(
        projects.clone(),
        Arc::clone(&history),
        runtime.clone(),
        Arc::clone(&ingress),
        source.clone(),
        secrets.clone(),
        overrides.clone(),
    );
    let probe = HostSystemProbe::new(
        SystemRunner::new(),
        &config.data_dir,
        config.cloudflared.is_some(),
    );
    let diagnostics = RunDiagnostics::new(probe, disk.clone(), config.gc.disk_threshold_percent);
    let clock = SystemClock::new();
    let overview = AgentStatus::new(
        projects.clone(),
        Arc::clone(&history),
        disk.clone(),
        clock.clone(),
        env!("CARGO_PKG_VERSION").to_string(),
        clock.now_unix(),
    );
    let stream_logs = StreamLogs::new(projects.clone(), secrets.clone(), runtime.clone());
```

(внимание на порядок: `remove`/`send_env` потребляют `projects`/`secrets`/`overrides`/`source`/`runtime` — везде использовать `.clone()`, мув оставить последнему потребителю.) В литерал `AppState` добавить новые поля + `log_dir`.

В `crates/bin/src/agent/run.rs` заменить вызов: `let state = build_state(&config, log_dir.clone())?;`.

- [ ] **Step 3: Падающие тесты эндпоинтов**

В `crates/bin/src/agent/http.rs`:

В `ok_source()` добавить:

```rust
        source.expect_cleanup().returning(|_| Ok(()));
```

В `ok_runtime()` добавить:

```rust
        runtime.expect_stats().returning(|_| {
            Ok(vec![pi_domain::entities::ServiceStats {
                service: "web".into(),
                cpu_percent: 1.5,
                mem_used_bytes: 1024,
                mem_limit_bytes: 2048,
            }])
        });
        runtime.expect_lifecycle().returning(|_, _, _| Ok(()));
        runtime.expect_down().returning(|_, _, _| Ok(()));
```

`state_with` дополнить созданием новых use-cases (по образцу build_state, с моками):

```rust
        use pi_application::diagnostics::{AgentStatus, RunDiagnostics};
        use pi_application::lifecycle::ControlLifecycle;
        use pi_application::logs::StreamLogs;
        use pi_application::remove::RemoveProject;
        use pi_application::stats::GetStats;
        use pi_infrastructure::stats::CompositeStats;

        let disk_for_stats = {
            let mut d = pi_domain::contracts::MockDiskProbe::new();
            d.expect_used_percent().returning(|| Ok(10));
            Arc::new(d)
        };
        let provider = CompositeStats::new(Arc::clone(&runtime), disk_for_stats.clone());
        let stats = GetStats::new(projects.clone(), Arc::clone(&history), provider);
        let lifecycle = ControlLifecycle::new(projects.clone(), Arc::clone(&runtime));
        let ingress: Arc<dyn pi_domain::contracts::Ingress> = DisabledIngress::new();
        let remove = RemoveProject::new(
            projects.clone(),
            Arc::clone(&history),
            Arc::clone(&runtime),
            ingress,
            source.clone(),
            secrets.clone(),
            overrides.clone(),
        );
        let probe = {
            let mut p = pi_domain::contracts::MockSystemProbe::new();
            p.expect_run_checks().returning(Vec::new);
            Arc::new(p)
        };
        let diagnostics = RunDiagnostics::new(probe, disk_for_stats.clone(), 85);
        let overview = AgentStatus::new(
            projects.clone(),
            Arc::clone(&history),
            disk_for_stats,
            SystemClock::new(),
            env!("CARGO_PKG_VERSION").to_string(),
            0,
        );
        let stream_logs = StreamLogs::new(projects.clone(), secrets.clone(), Arc::clone(&runtime));
```

(`DisabledIngress` уже импортирован в `state_with`; `DeployProject::new` получает `DisabledIngress::new()` отдельным вызовом, как сейчас. Новые поля — в литерал `AppState`, `log_dir: None`.)

Новые тесты:

```rust
    #[tokio::test]
    async fn status_endpoint_reports_version_and_counts() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let (status, json) = request(app, get_req("/v1/status")).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(json["projects"], 0);
        assert_eq!(json["active_deployments"], 0);
        assert_eq!(json["disk_used_percent"], 10);
    }

    #[tokio::test]
    async fn doctor_endpoint_renders_disk_check() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let (status, json) = request(app, get_req("/v1/doctor")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["all_passed"], true, "{json}");
        assert_eq!(json["checks"][0]["name"], "disk space");
    }

    async fn deploy_and_wait(app: &Router, name: &str) {
        let (status, json) =
            request(app.clone(), post_json("/v1/deployments", &deploy_body(name))).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{json}");
        let id = json["deployment_id"].as_str().unwrap().to_string();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            assert!(tokio::time::Instant::now() < deadline, "deploy hung");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let (_, json) =
                request(app.clone(), get_req(&format!("/v1/deployments/{id}"))).await;
            if json["status"] == "success" {
                break;
            }
        }
    }

    #[tokio::test]
    async fn stats_endpoint_joins_runtime_and_history() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        deploy_and_wait(&app, "rateme").await;

        let (status, json) = request(app.clone(), get_req("/v1/stats")).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["projects"][0]["project"], "rateme");
        assert_eq!(json["projects"][0]["services"][0]["service"], "web");
        assert_eq!(json["projects"][0]["last_deploy"]["status"], "success");
        assert!(json["host"]["mem_total_bytes"].as_u64().unwrap() > 0);

        let (status, json) =
            request(app, get_req("/v1/projects/rateme/stats")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["projects"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn stats_for_unknown_project_is_404() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let (status, _) = request(app, get_req("/v1/projects/ghost/stats")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn lifecycle_endpoint_validates_action_and_project() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        deploy_and_wait(&app, "rateme").await;

        let body = serde_json::json!({ "action": "restart" });
        let (status, json) = request(
            app.clone(),
            post_json("/v1/projects/rateme/lifecycle", &body),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{json}");

        let bad = serde_json::json!({ "action": "explode" });
        let (status, _) = request(
            app.clone(),
            post_json("/v1/projects/rateme/lifecycle", &bad),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, _) = request(app, post_json("/v1/projects/ghost/lifecycle", &body)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn remove_endpoint_deletes_project_and_404s_on_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        deploy_and_wait(&app, "rateme").await;

        let (status, json) = request(app.clone(), delete_req("/v1/projects/rateme")).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["volumes_removed"], false);

        let (status, json) = request(app.clone(), get_req("/v1/projects")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json.as_array().unwrap().len(), 0, "project gone");

        let (status, _) = request(app, delete_req("/v1/projects/rateme")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn remove_with_active_deployment_is_409() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with(dir.path(), Arc::new(ok_source()), Arc::new(ok_runtime()));
        // register the project, then plant an active history row
        let app = router(state.clone());
        deploy_and_wait(&app, "rateme").await;
        state
            .history
            .record_queued(&pi_domain::entities::Deployment {
                id: "ghost-q".into(),
                project: "rateme".into(),
                git_ref: "main".into(),
                commit_sha: None,
                status: DeploymentStatus::Queued,
                started_at: 1,
                finished_at: None,
                log_tail: String::new(),
            })
            .await
            .unwrap();

        let (status, json) = request(app, delete_req("/v1/projects/rateme")).await;
        assert_eq!(status, StatusCode::CONFLICT, "{json}");
        assert!(json["error"].as_str().unwrap().contains("--cancel"));
    }
```

Run: `rtk cargo test -p pi http` → FAIL (роутов/хендлеров нет).

- [ ] **Step 4: Реализовать хендлеры и роуты**

В `crates/bin/src/agent/http.rs`:

Импорты: `use axum::extract::Query;`, `use axum::routing::{delete, get, post, put};`, `use serde::Deserialize;`, в `crate::proto::{...}` добавить `AgentStatusResponse, DoctorResponse, LifecycleRequest, RemoveResponse, StatsResponse`, из `pi_domain::entities` — `LifecycleAction`.

В `router()` добавить роуты:

```rust
        .route("/v1/stats", get(get_stats))
        .route("/v1/status", get(agent_status))
        .route("/v1/doctor", get(doctor))
        .route("/v1/projects/{name}", delete(remove_project_handler))
        .route("/v1/projects/{name}/lifecycle", post(lifecycle_handler))
        .route("/v1/projects/{name}/stats", get(project_stats))
```

Хендлеры:

```rust
async fn get_stats(State(state): State<AppState>) -> Result<Json<StatsResponse>, ApiError> {
    Ok(Json(state.stats.execute(None).await.map_err(ApiError)?.into()))
}

async fn project_stats(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<StatsResponse>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    Ok(Json(
        state.stats.execute(Some(&name)).await.map_err(ApiError)?.into(),
    ))
}

async fn agent_status(
    State(state): State<AppState>,
) -> Result<Json<AgentStatusResponse>, ApiError> {
    Ok(Json(state.overview.execute().await.map_err(ApiError)?.into()))
}

async fn doctor(State(state): State<AppState>) -> Json<DoctorResponse> {
    Json(state.diagnostics.execute().await.into())
}

/// Lifecycle and remove run synchronously inside the request; their output
/// goes to the agent journal (TracingSink), the CLI gets a JSON summary —
/// same contract as POST /v1/gc.
async fn lifecycle_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<LifecycleRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    let action: LifecycleAction = req.action.parse().map_err(|_| {
        ApiError(DomainError::Invalid(format!(
            "unknown action '{}' (expected start|stop|restart)",
            req.action
        )))
    })?;
    state
        .lifecycle
        .execute(&name, action, Arc::new(TracingSink))
        .await
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({ "status": "ok" })))
}

#[derive(Deserialize)]
struct RemoveQuery {
    #[serde(default)]
    volumes: bool,
}

async fn remove_project_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<RemoveQuery>,
) -> Result<Json<RemoveResponse>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    let report = state
        .remove
        .execute(&name, q.volumes, Arc::new(TracingSink))
        .await
        .map_err(ApiError)?;
    Ok(Json(RemoveResponse {
        hostname: report.hostname,
        volumes_removed: report.volumes_removed,
    }))
}
```

- [ ] **Step 5: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(bin): stats, status, doctor, lifecycle and remove endpoints"
```

---

### Task 16: Bin — SSE-логи проекта и агента

**Files:**
- Modify: `crates/bin/src/agent/http.rs`

- [ ] **Step 1: Падающие тесты**

В `mod tests` файла `crates/bin/src/agent/http.rs` добавить хелпер и тесты:

```rust
    async fn request_raw(
        app: Router,
        req: axum::http::Request<axum::body::Body>,
    ) -> (StatusCode, String) {
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn project_logs_stream_lines_then_end() {
        let dir = tempfile::tempdir().unwrap();
        let mut runtime = ok_runtime();
        runtime
            .expect_logs()
            .withf(|p, tail, follow, _| p == "rateme" && *tail == 7 && !*follow)
            .returning(|_, _, _, sink| {
                sink.line("web | hello");
                sink.line("db  | ready");
                Ok(())
            });
        let app = router(state_with(dir.path(), Arc::new(ok_source()), Arc::new(runtime)));
        deploy_and_wait(&app, "rateme").await;

        let (status, body) = request_raw(
            app,
            get_req("/v1/projects/rateme/logs?tail=7&follow=false"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("web | hello"), "{body}");
        assert!(body.contains("db  | ready"), "{body}");
        assert!(body.contains("event: finished"), "{body}");
        assert!(body.contains("data: end"), "{body}");
    }

    #[tokio::test]
    async fn project_logs_unknown_project_is_404() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let (status, _) = request(app, get_req("/v1/projects/ghost/logs")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn agent_logs_tail_from_rolling_files() {
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(
            log_dir.join("pi-agent.log.2026-06-11"),
            "line-1\nline-2\nline-3\n",
        )
        .unwrap();
        let mut state = state_with(dir.path(), Arc::new(ok_source()), Arc::new(ok_runtime()));
        state.log_dir = Some(log_dir);
        let app = router(state);

        let (status, body) = request_raw(app, get_req("/v1/agent/logs?tail=2")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!body.contains("line-1"), "{body}");
        assert!(body.contains("line-2") && body.contains("line-3"), "{body}");
        assert!(body.contains("data: end"), "{body}");
    }

    #[tokio::test]
    async fn agent_logs_404_when_file_logging_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        )); // log_dir: None
        let (status, json) = request(app, get_req("/v1/agent/logs")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(json["error"].as_str().unwrap().contains("file logging"));
    }
```

Run: `rtk cargo test -p pi http` → FAIL (роутов нет).

- [ ] **Step 2: Реализовать SSE-хендлеры**

В `crates/bin/src/agent/http.rs` добавить:

```rust
/// LogSink writing into an mpsc channel; the SSE stream drains it.
struct ChannelSink(tokio::sync::mpsc::UnboundedSender<String>);

impl pi_domain::contracts::LogSink for ChannelSink {
    fn line(&self, line: &str) {
        let _ = self.0.send(line.to_string());
    }
    fn finished(&self, _status: DeploymentStatus) {}
}

/// Aborts the producer task when the SSE response is dropped (client gone);
/// kill_on_drop in the process adapter then terminates `compose logs -f`.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Deserialize)]
struct LogsQuery {
    #[serde(default = "default_tail")]
    tail: usize,
    #[serde(default)]
    follow: bool,
}

fn default_tail() -> usize {
    pi_application::logs::DEFAULT_LOG_TAIL
}

async fn project_logs(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<LogsQuery>,
) -> Result<Response, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    // 404 must be an HTTP status, not an SSE event
    state.stream_logs.ensure_project(&name).await.map_err(ApiError)?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let stream_logs = Arc::clone(&state.stream_logs);
    let handle = tokio::spawn(async move {
        let sink: Arc<dyn pi_domain::contracts::LogSink> = Arc::new(ChannelSink(tx));
        if let Err(e) = stream_logs
            .execute(&name, q.tail, q.follow, Arc::clone(&sink))
            .await
        {
            sink.line(&format!("error: {e}"));
        }
    });
    let stream = async_stream::stream! {
        let _guard = AbortOnDrop(handle);
        while let Some(line) = rx.recv().await {
            yield sse_log(line);
        }
        yield sse_finished("end");
    };
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

#[derive(Deserialize)]
struct AgentLogsQuery {
    tail: Option<usize>,
    since: Option<i64>,
    #[serde(default)]
    follow: bool,
}

async fn agent_logs(
    State(state): State<AppState>,
    Query(q): Query<AgentLogsQuery>,
) -> Result<Response, ApiError> {
    use crate::agent::logfile;

    let Some(dir) = state.log_dir.clone() else {
        return Err(ApiError(DomainError::NotFound(
            "file logging is disabled on the agent (log dir was not created)".into(),
        )));
    };
    let backlog = match q.since {
        Some(since) => {
            let cutoff = logfile::unix_to_rfc3339(since).ok_or_else(|| {
                ApiError(DomainError::Invalid(format!("bad since timestamp {since}")))
            })?;
            logfile::lines_since(&dir, &cutoff)
        }
        None => logfile::tail_lines(&dir, q.tail.unwrap_or(default_tail())),
    };
    let follow = q.follow;
    let stream = async_stream::stream! {
        for line in backlog {
            yield sse_log(line);
        }
        if follow {
            let mut pos = logfile::follow_pos(&dir);
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let (new_pos, lines) = logfile::read_new_lines(&dir, pos);
                pos = new_pos;
                for line in lines {
                    yield sse_log(line);
                }
            }
        } else {
            yield sse_finished("end");
        }
    };
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}
```

Роуты в `router()`:

```rust
        .route("/v1/projects/{name}/logs", get(project_logs))
        .route("/v1/agent/logs", get(agent_logs))
```

- [ ] **Step 3: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(bin): SSE streaming for project and agent logs"
```

---

### Task 17: CLI — `SshExec` и методы `ApiClient`

**Files:**
- Create: `crates/bin/src/cli/ssh.rs`
- Modify: `crates/bin/src/cli/mod.rs`
- Modify: `crates/bin/src/cli/tunnel.rs` (`expand_home` → `pub(crate)`)
- Modify: `crates/bin/src/cli/api.rs`

- [ ] **Step 1: Падающий тест SshExec**

Создать `crates/bin/src/cli/ssh.rs` с тестом; в `crates/bin/src/cli/mod.rs` добавить `pub mod ssh;`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_command_shape_with_key() {
        let profile = ServerProfile {
            host: "203.0.113.7".into(),
            user: "pi".into(),
            key: Some("./k".into()),
        };
        let ssh = SshExec { profile: &profile };
        let cmd = ssh.command(&["journalctl", "-u", "pi-agent"]);
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![
                "-i",
                "./k",
                "-o",
                "BatchMode=yes",
                "pi@203.0.113.7",
                "--",
                "journalctl",
                "-u",
                "pi-agent"
            ]
        );
    }

    #[test]
    fn ssh_command_without_key_has_no_identity_flag() {
        let profile = ServerProfile {
            host: "h".into(),
            user: "u".into(),
            key: None,
        };
        let cmd = SshExec { profile: &profile }.command(&["true"]);
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["-o", "BatchMode=yes", "u@h", "--", "true"]);
    }
}
```

В `crates/bin/src/cli/tunnel.rs` сменить видимость: `pub(crate) fn expand_home(path: &str) -> String`.

Run: `rtk cargo test -p pi ssh` → FAIL.

- [ ] **Step 2: Реализовать SshExec**

В начало `crates/bin/src/cli/ssh.rs`:

```rust
use crate::cli::config::ServerProfile;
use crate::cli::tunnel::expand_home;

/// Plain `ssh user@host -- cmd` — the diagnostics fallback when the agent API
/// is unreachable (v0.4 design §2.1). Reuses the tunnel's connection options.
pub struct SshExec<'a> {
    pub profile: &'a ServerProfile,
}

impl SshExec<'_> {
    pub fn command(&self, remote: &[&str]) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("ssh");
        if let Some(key) = &self.profile.key {
            cmd.arg("-i").arg(expand_home(key));
        }
        cmd.args(["-o", "BatchMode=yes"]);
        cmd.arg(format!("{}@{}", self.profile.user, self.profile.host));
        cmd.arg("--");
        cmd.args(remote);
        cmd
    }

    /// Interactive: remote output goes straight to the user's terminal
    /// (journalctl/systemctl fallbacks of `pi agent logs|status`).
    pub async fn run(&self, remote: &[&str]) -> anyhow::Result<()> {
        let mut cmd = self.command(remote);
        cmd.stdin(std::process::Stdio::null());
        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("cannot spawn ssh: {e}"))?;
        let status = child.wait().await?;
        if !status.success() {
            anyhow::bail!("ssh {} exited with {status}", remote.join(" "));
        }
        Ok(())
    }

    /// Quiet connectivity probe for `pi doctor`: Ok on zero exit,
    /// Err(stderr) otherwise.
    pub async fn check(&self) -> Result<(), String> {
        let mut cmd = self.command(&["true"]);
        cmd.stdin(std::process::Stdio::null());
        match cmd.output().await {
            Ok(out) if out.status.success() => Ok(()),
            Ok(out) => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
            Err(e) => Err(format!("cannot spawn ssh: {e}")),
        }
    }
}
```

Run: `rtk cargo test -p pi ssh` → PASS.

- [ ] **Step 3: Методы ApiClient + generic SSE**

В `crates/bin/src/cli/api.rs`:

Импорт DTO: в `use crate::proto::{...}` добавить `AgentStatusResponse, DoctorResponse, LifecycleRequest, RemoveResponse, StatsResponse`.

Обобщить SSE-стрим: переименовать тело `follow_logs` в `stream_sse` и сделать `follow_logs` обёрткой:

```rust
    /// Streams an SSE endpoint; returns the `finished` event data, or None
    /// when the stream ends without one (agent restart / connection drop).
    pub async fn stream_sse(
        &self,
        path_and_query: &str,
        mut on_line: impl FnMut(&str),
    ) -> anyhow::Result<Option<String>> {
        let resp = self
            .http
            .get(format!("{}{path_and_query}", self.base))
            .send()
            .await?;
        let resp = extract_error(resp).await?;
        let mut stream = resp.bytes_stream();
        let mut parser = SseParser::default();
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            buf.extend_from_slice(&chunk?);
            let valid_up_to = match std::str::from_utf8(&buf) {
                Ok(_) => buf.len(),
                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                Err(_) => buf.len(),
            };
            if valid_up_to == 0 {
                continue;
            }
            let text = String::from_utf8_lossy(&buf[..valid_up_to]).into_owned();
            buf.drain(..valid_up_to);
            for ev in parser.push(&text) {
                match ev.event.as_str() {
                    "log" => on_line(&ev.data),
                    "finished" => return Ok(Some(ev.data)),
                    _ => {}
                }
            }
        }
        Ok(None)
    }

    pub async fn follow_logs(
        &self,
        id: &str,
        on_line: impl FnMut(&str),
    ) -> anyhow::Result<String> {
        self.stream_sse(&format!("/v1/deployments/{id}/logs"), on_line)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("log stream ended without a final status (agent restarted?)")
            })
    }
```

Новые методы:

```rust
    pub async fn stats(&self, project: Option<&str>) -> anyhow::Result<StatsResponse> {
        let url = match project {
            Some(name) => format!("{}/v1/projects/{name}/stats", self.base),
            None => format!("{}/v1/stats", self.base),
        };
        let resp = self.http.get(url).send().await?;
        Ok(extract_error(resp).await?.json().await?)
    }

    pub async fn agent_status(&self) -> anyhow::Result<AgentStatusResponse> {
        let resp = self
            .http
            .get(format!("{}/v1/status", self.base))
            .send()
            .await?;
        Ok(extract_error(resp).await?.json().await?)
    }

    pub async fn doctor(&self) -> anyhow::Result<DoctorResponse> {
        let resp = self
            .http
            .get(format!("{}/v1/doctor", self.base))
            .send()
            .await?;
        Ok(extract_error(resp).await?.json().await?)
    }

    pub async fn lifecycle(&self, project: &str, action: &str) -> anyhow::Result<()> {
        let req = LifecycleRequest {
            action: action.to_string(),
        };
        let resp = self
            .http
            .post(format!("{}/v1/projects/{project}/lifecycle", self.base))
            .json(&req)
            .send()
            .await?;
        extract_error(resp).await?;
        Ok(())
    }

    pub async fn remove_project(
        &self,
        project: &str,
        volumes: bool,
    ) -> anyhow::Result<RemoveResponse> {
        let resp = self
            .http
            .delete(format!(
                "{}/v1/projects/{project}?volumes={volumes}",
                self.base
            ))
            .send()
            .await?;
        Ok(extract_error(resp).await?.json().await?)
    }
```

- [ ] **Step 4: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(cli): ssh exec fallback and api client methods for operations"
```

---

### Task 18: CLI — команды logs / stats / start|stop|restart / rm / status

**Files:**
- Modify: `crates/bin/src/cli/commands.rs`
- Modify: `crates/bin/src/main.rs`

- [ ] **Step 1: Падающие тесты рендера и clap**

В `mod tests` файла `crates/bin/src/cli/commands.rs`:

```rust
    #[test]
    fn human_bytes_and_duration_format() {
        assert_eq!(human_bytes(512), "512B");
        assert_eq!(human_bytes(1536), "1.5KiB");
        assert_eq!(human_bytes(13_107_200), "12.5MiB");
        assert_eq!(human_duration(90), "1m");
        assert_eq!(human_duration(3 * 3600 + 120), "3h2m");
        assert_eq!(human_duration(2 * 86400 + 5 * 3600), "2d5h");
    }

    #[test]
    fn render_stats_shows_host_projects_and_last_deploy() {
        use crate::proto::{
            DeploymentDto, HostStatsDto, ProjectStatsDto, ServiceStatsDto, StatsResponse,
        };
        let resp = StatsResponse {
            host: HostStatsDto {
                cpu_percent: 12.3,
                mem_used_bytes: 512 * 1024 * 1024,
                mem_total_bytes: 1024 * 1024 * 1024,
                disk_used_percent: 42,
                uptime_secs: 3700,
            },
            projects: vec![ProjectStatsDto {
                project: "rateme".into(),
                services: vec![ServiceStatsDto {
                    service: "web".into(),
                    cpu_percent: 1.5,
                    mem_used_bytes: 100 * 1024 * 1024,
                    mem_limit_bytes: 1024 * 1024 * 1024,
                }],
                last_deploy: Some(DeploymentDto {
                    id: "d1".into(),
                    project: "rateme".into(),
                    git_ref: "main".into(),
                    commit_sha: None,
                    status: "success".into(),
                    started_at: 1,
                    finished_at: Some(2),
                    log_tail: String::new(),
                }),
            }],
        };
        let out = render_stats(&resp);
        assert!(out.contains("disk 42%"), "{out}");
        assert!(out.contains("rateme"), "{out}");
        assert!(out.contains("web"), "{out}");
        assert!(out.contains("success main"), "{out}");
    }

    #[test]
    fn render_stats_marks_empty_projects() {
        use crate::proto::{HostStatsDto, ProjectStatsDto, StatsResponse};
        let resp = StatsResponse {
            host: HostStatsDto {
                cpu_percent: 0.0,
                mem_used_bytes: 0,
                mem_total_bytes: 1,
                disk_used_percent: 0,
                uptime_secs: 0,
            },
            projects: vec![ProjectStatsDto {
                project: "idle".into(),
                services: vec![],
                last_deploy: None,
            }],
        };
        let out = render_stats(&resp);
        assert!(out.contains("no running containers"), "{out}");
        assert!(out.contains("never deployed"), "{out}");
    }
```

В `mod tests` файла `crates/bin/src/main.rs`:

```rust
    #[test]
    fn logs_defaults_tail_100() {
        let cli = Cli::try_parse_from(["pi", "logs", "rateme"]).unwrap();
        match cli.cmd {
            Cmd::Logs { project, follow, tail, .. } => {
                assert_eq!(project, "rateme");
                assert!(!follow);
                assert_eq!(tail, 100);
            }
            _ => panic!("expected logs"),
        }
    }

    #[test]
    fn rm_flags_parse() {
        let cli = Cli::try_parse_from(["pi", "rm", "rateme", "--volumes", "--yes"]).unwrap();
        match cli.cmd {
            Cmd::Rm { project, volumes, yes, .. } => {
                assert_eq!(project, "rateme");
                assert!(volumes && yes);
            }
            _ => panic!("expected rm"),
        }
    }

    #[test]
    fn lifecycle_subcommands_parse() {
        assert!(Cli::try_parse_from(["pi", "start", "rateme"]).is_ok());
        assert!(Cli::try_parse_from(["pi", "stop", "rateme"]).is_ok());
        assert!(Cli::try_parse_from(["pi", "restart", "rateme"]).is_ok());
        assert!(Cli::try_parse_from(["pi", "start"]).is_err(), "project required");
    }
```

Run: `rtk cargo test -p pi` → FAIL.

- [ ] **Step 2: Реализовать команды**

В `crates/bin/src/cli/commands.rs` добавить (импорт `use crate::proto::StatsResponse;` для рендера):

```rust
pub(crate) fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = b as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{b}B")
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}

pub(crate) fn human_duration(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, (secs % 86400) / 3600, (secs % 3600) / 60);
    if d > 0 {
        format!("{d}d{h}h")
    } else if h > 0 {
        format!("{h}h{m}m")
    } else {
        format!("{m}m")
    }
}

pub(crate) fn render_stats(r: &StatsResponse) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "host: cpu {:.1}%  mem {}/{}  disk {}%  up {}\n",
        r.host.cpu_percent,
        human_bytes(r.host.mem_used_bytes),
        human_bytes(r.host.mem_total_bytes),
        r.host.disk_used_percent,
        human_duration(r.host.uptime_secs),
    ));
    for p in &r.projects {
        let last = p
            .last_deploy
            .as_ref()
            .map(|d| format!("{} {}", d.status, d.git_ref))
            .unwrap_or_else(|| "never deployed".into());
        out.push_str(&format!("\n{}  (last deploy: {last})\n", p.project));
        if p.services.is_empty() {
            out.push_str("  (no running containers)\n");
        }
        for s in &p.services {
            out.push_str(&format!(
                "  {:<16} cpu {:>5.1}%  mem {}/{}\n",
                s.service,
                s.cpu_percent,
                human_bytes(s.mem_used_bytes),
                human_bytes(s.mem_limit_bytes)
            ));
        }
    }
    out
}

pub async fn logs(
    project: String,
    follow: bool,
    tail: usize,
    connect: ConnectOpts,
) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let end = api
        .stream_sse(
            &format!("/v1/projects/{project}/logs?tail={tail}&follow={follow}"),
            |line| println!("{line}"),
        )
        .await?;
    if end.is_none() && !follow {
        anyhow::bail!("log stream ended unexpectedly (agent restarted?)");
    }
    Ok(())
}

pub async fn stats(
    project: Option<String>,
    json: bool,
    connect: ConnectOpts,
) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let resp = api.stats(project.as_deref()).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
    } else {
        print!("{}", render_stats(&resp));
    }
    Ok(())
}

pub async fn lifecycle(
    project: String,
    action: &str,
    connect: ConnectOpts,
) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    api.lifecycle(&project, action).await?;
    eprintln!("{action} '{project}': done");
    Ok(())
}

pub async fn rm(
    project: String,
    volumes: bool,
    yes: bool,
    connect: ConnectOpts,
) -> anyhow::Result<()> {
    if !yes {
        eprintln!(
            "this removes containers{}, the ingress route, workdir, secrets, deploy key and history of '{project}'",
            if volumes { ", VOLUMES (project data!)" } else { "" }
        );
        eprint!("type the project name to confirm: ");
        use std::io::Write;
        std::io::stderr().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if input.trim() != project {
            anyhow::bail!("confirmation failed: expected '{project}'");
        }
    }

    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let resp = api.remove_project(&project, volumes).await?;
    eprintln!(
        "project '{project}' removed{}",
        if resp.volumes_removed { " (volumes included)" } else { " (volumes kept)" }
    );
    if let Some(hostname) = resp.hostname {
        eprintln!("note: the DNS record for {hostname} still exists (§11);");
        eprintln!("delete it manually: Cloudflare dashboard -> your zone -> DNS -> remove the {hostname} CNAME");
    }
    Ok(())
}

pub async fn status(json: bool, connect: ConnectOpts) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let resp = api.agent_status().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }
    println!("agent v{} (cli v{})", resp.version, env!("CARGO_PKG_VERSION"));
    println!("uptime: {}", human_duration(resp.uptime_secs));
    println!("disk: {}% used", resp.disk_used_percent);
    println!(
        "projects: {}, active deployments: {}",
        resp.projects, resp.active_deployments
    );
    Ok(())
}
```

- [ ] **Step 3: Сабкоманды в main.rs**

В enum `Cmd` (`crates/bin/src/main.rs`) добавить:

```rust
    /// Stream container logs of a project
    Logs {
        project: String,
        /// Keep following new lines
        #[arg(short, long)]
        follow: bool,
        /// How many recent lines to start with
        #[arg(long, default_value_t = 100)]
        tail: usize,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Live CPU/memory/disk metrics (host + projects)
    Stats {
        project: Option<String>,
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Start project containers (no rebuild)
    Start {
        project: String,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Stop project containers
    Stop {
        project: String,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Restart project containers
    Restart {
        project: String,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Remove a project: containers, ingress route, workdir, secrets, history
    Rm {
        project: String,
        /// Also remove named volumes (project data!)
        #[arg(long)]
        volumes: bool,
        /// Skip the interactive confirmation
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Agent and host overview
    Status {
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
```

В `run_cli` добавить ветки:

```rust
        Cmd::Logs { project, follow, tail, connect } => {
            cli::commands::logs(project, follow, tail, connect).await
        }
        Cmd::Stats { project, json, connect } => cli::commands::stats(project, json, connect).await,
        Cmd::Start { project, connect } => cli::commands::lifecycle(project, "start", connect).await,
        Cmd::Stop { project, connect } => cli::commands::lifecycle(project, "stop", connect).await,
        Cmd::Restart { project, connect } => {
            cli::commands::lifecycle(project, "restart", connect).await
        }
        Cmd::Rm { project, volumes, yes, connect } => {
            cli::commands::rm(project, volumes, yes, connect).await
        }
        Cmd::Status { json, connect } => cli::commands::status(json, connect).await,
```

- [ ] **Step 4: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS. Smoke вручную (агент в dev-режиме): `pi stats`, `pi logs <project>`, `pi status` через `PI_AGENT_URL`.

```bash
rtk git add -A && rtk git commit -m "feat(bin): pi logs, stats, lifecycle, rm and status commands"
```

---

### Task 19: CLI — `pi doctor` и `pi agent status|logs` с ssh-фолбэком

**Files:**
- Modify: `crates/bin/src/cli/commands.rs`
- Modify: `crates/bin/src/main.rs`

- [ ] **Step 1: Падающие тесты хелперов**

В `mod tests` файла `crates/bin/src/cli/commands.rs`:

```rust
    #[test]
    fn render_doctor_marks_failures_and_hints() {
        use crate::proto::DiagnosticCheckDto;
        let checks = vec![
            DiagnosticCheckDto {
                name: "docker daemon".into(),
                passed: true,
                detail: "27.0".into(),
                hint: None,
            },
            DiagnosticCheckDto {
                name: "disk space".into(),
                passed: false,
                detail: "91% used".into(),
                hint: Some("run `pi gc`".into()),
            },
        ];
        let (out, ok) = render_doctor(&checks);
        assert!(!ok);
        assert!(out.contains("PASS  docker daemon"), "{out}");
        assert!(out.contains("FAIL  disk space"), "{out}");
        assert!(out.contains("hint: run `pi gc`"), "{out}");

        let (_, ok) = render_doctor(&checks[..1]);
        assert!(ok);
    }

    #[test]
    fn agent_logs_query_prefers_since_over_tail() {
        let q = build_agent_logs_query(false, &None, 50, 1000).unwrap();
        assert_eq!(q, "/v1/agent/logs?tail=50&follow=false");
        let q = build_agent_logs_query(true, &Some("2h".into()), 50, 10_000).unwrap();
        assert_eq!(q, "/v1/agent/logs?since=2800&follow=true");
        assert!(build_agent_logs_query(false, &Some("soon".into()), 50, 0).is_err());
    }

    #[test]
    fn journalctl_args_shape() {
        assert_eq!(
            journalctl_args(false, None, 100),
            vec!["journalctl", "-u", "pi-agent", "--no-pager", "-n", "100"]
        );
        assert_eq!(
            journalctl_args(true, Some(1234), 50),
            vec![
                "journalctl",
                "-u",
                "pi-agent",
                "--no-pager",
                "-n",
                "50",
                "--since=@1234",
                "-f"
            ]
        );
    }
```

Run: `rtk cargo test -p pi commands` → FAIL.

- [ ] **Step 2: Реализовать**

В `crates/bin/src/cli/commands.rs` (импорты: `use crate::cli::ssh::SshExec;`, `use crate::proto::DiagnosticCheckDto;`, `use crate::duration::parse_duration_secs;`):

```rust
pub(crate) fn render_doctor(checks: &[DiagnosticCheckDto]) -> (String, bool) {
    let mut out = String::new();
    let mut ok = true;
    for c in checks {
        let mark = if c.passed {
            "PASS"
        } else {
            ok = false;
            "FAIL"
        };
        out.push_str(&format!("{mark}  {} - {}\n", c.name, c.detail));
        if let (false, Some(hint)) = (c.passed, &c.hint) {
            out.push_str(&format!("      hint: {hint}\n"));
        }
    }
    (out, ok)
}

fn check(name: &str, passed: bool, detail: String, hint: Option<&str>) -> DiagnosticCheckDto {
    DiagnosticCheckDto {
        name: name.to_string(),
        passed,
        detail,
        hint: if passed { None } else { hint.map(str::to_string) },
    }
}

/// Client-side checks (ssh, tunnel, agent api, version) + agent-side
/// GET /v1/doctor (§14). Exit code 1 on any FAIL.
pub async fn doctor(connect: ConnectOpts) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let mut checks: Vec<DiagnosticCheckDto> = Vec::new();

    let ssh = SshExec { profile: &profile };
    checks.push(match ssh.check().await {
        Ok(()) => check(
            "ssh connection",
            true,
            format!("{}@{}", profile.user, profile.host),
            None,
        ),
        Err(e) => check(
            "ssh connection",
            false,
            e,
            Some("check host/user/key in ~/.config/pi/config.toml; try plain `ssh` manually"),
        ),
    });

    match SshTunnel::open(&profile).await {
        Err(e) => checks.push(check(
            "agent tunnel",
            false,
            e.to_string(),
            Some("is pi-agent.service running on the Pi? try `pi agent status`"),
        )),
        Ok(tunnel) => {
            let api = ApiClient::new(tunnel.base_url.clone());
            match api.version().await {
                Err(e) => checks.push(check(
                    "agent api",
                    false,
                    e.to_string(),
                    Some("agent is unreachable through the tunnel; `pi agent logs` for details"),
                )),
                Ok(v) => {
                    checks.push(check(
                        "agent api",
                        true,
                        format!("agent v{} (api {})", v.version, v.api),
                        None,
                    ));
                    let cli_version = env!("CARGO_PKG_VERSION");
                    checks.push(check(
                        "version match",
                        v.version == cli_version,
                        format!("cli v{cli_version}, agent v{}", v.version),
                        Some("update the agent binary on the Pi"),
                    ));
                    match api.doctor().await {
                        Ok(resp) => checks.extend(resp.checks),
                        Err(e) => checks.push(check(
                            "agent doctor",
                            false,
                            e.to_string(),
                            Some("agent is older than v0.4? update it on the Pi"),
                        )),
                    }
                }
            }
        }
    }

    let (rendered, ok) = render_doctor(&checks);
    print!("{rendered}");
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}

pub async fn agent_status(connect: ConnectOpts) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let api_attempt = async {
        let tunnel = SshTunnel::open(&profile).await?;
        let resp = ApiClient::new(tunnel.base_url.clone()).agent_status().await?;
        anyhow::Ok(resp)
    };
    match api_attempt.await {
        Ok(resp) => {
            println!("agent v{} (cli v{})", resp.version, env!("CARGO_PKG_VERSION"));
            println!("uptime: {}", human_duration(resp.uptime_secs));
            println!("disk: {}% used", resp.disk_used_percent);
            println!(
                "projects: {}, active deployments: {}",
                resp.projects, resp.active_deployments
            );
            Ok(())
        }
        Err(err) => {
            eprintln!("agent API unreachable ({err})");
            eprintln!("falling back to: ssh {}@{} systemctl status pi-agent", profile.user, profile.host);
            SshExec { profile: &profile }
                .run(&["systemctl", "status", "pi-agent", "--no-pager"])
                .await
        }
    }
}

pub(crate) fn build_agent_logs_query(
    follow: bool,
    since: &Option<String>,
    tail: usize,
    now_unix: i64,
) -> anyhow::Result<String> {
    match since {
        Some(spec) => {
            let secs = parse_duration_secs(spec).map_err(|e| anyhow::anyhow!(e))?;
            let cutoff = now_unix - secs as i64;
            Ok(format!("/v1/agent/logs?since={cutoff}&follow={follow}"))
        }
        None => Ok(format!("/v1/agent/logs?tail={tail}&follow={follow}")),
    }
}

pub(crate) fn journalctl_args(follow: bool, since_unix: Option<i64>, tail: usize) -> Vec<String> {
    let mut args: Vec<String> = ["journalctl", "-u", "pi-agent", "--no-pager", "-n"]
        .map(String::from)
        .to_vec();
    args.push(tail.to_string());
    if let Some(cutoff) = since_unix {
        args.push(format!("--since=@{cutoff}"));
    }
    if follow {
        args.push("-f".to_string());
    }
    args
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub async fn agent_logs(
    follow: bool,
    since: Option<String>,
    tail: usize,
    connect: ConnectOpts,
) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let now = now_unix();
    let query = build_agent_logs_query(follow, &since, tail, now)?;

    let api_attempt = async {
        let tunnel = SshTunnel::open(&profile).await?;
        let api = ApiClient::new(tunnel.base_url.clone());
        api.stream_sse(&query, |line| println!("{line}")).await?;
        anyhow::Ok(())
    };
    match api_attempt.await {
        Ok(()) => Ok(()),
        Err(err) => {
            eprintln!("agent API unreachable ({err})");
            let since_unix = since
                .as_deref()
                .and_then(|s| parse_duration_secs(s).ok())
                .map(|secs| now - secs as i64);
            let args = journalctl_args(follow, since_unix, tail);
            let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
            eprintln!("falling back to: ssh {}@{} {}", profile.user, profile.host, args.join(" "));
            SshExec { profile: &profile }.run(&args_ref).await
        }
    }
}
```

Проверка ожидания в тесте `agent_logs_query_prefers_since_over_tail`: `now_unix=10_000`, `2h=7200` → `since=2800`. ✓

- [ ] **Step 3: Сабкоманды агента в main.rs**

В enum `Cmd` добавить вариант `Doctor`:

```rust
    /// Environment self-diagnosis (PASS/FAIL + hints)
    Doctor {
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
```

В `AgentCmd` добавить:

```rust
    /// Agent overview; falls back to `systemctl status` over ssh
    Status {
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Agent's own logs; falls back to `journalctl` over ssh
    Logs {
        /// Keep following new lines
        #[arg(short, long)]
        follow: bool,
        /// Show lines newer than a duration ago, e.g. "30m", "2h"
        #[arg(long)]
        since: Option<String>,
        /// How many recent lines to start with (ignored with --since)
        #[arg(long, default_value_t = 100)]
        tail: usize,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
```

В `run_cli`:

```rust
        Cmd::Doctor { connect } => cli::commands::doctor(connect).await,
        Cmd::Agent { cmd: AgentCmd::Status { connect } } => {
            cli::commands::agent_status(connect).await
        }
        Cmd::Agent {
            cmd: AgentCmd::Logs { follow, since, tail, connect },
        } => cli::commands::agent_logs(follow, since, tail, connect).await,
```

И clap-тест в `mod tests` main.rs:

```rust
    #[test]
    fn agent_logs_flags_parse() {
        let cli =
            Cli::try_parse_from(["pi", "agent", "logs", "-f", "--since", "2h"]).unwrap();
        match cli.cmd {
            Cmd::Agent { cmd: AgentCmd::Logs { follow, since, tail, .. } } => {
                assert!(follow);
                assert_eq!(since.as_deref(), Some("2h"));
                assert_eq!(tail, 100);
            }
            _ => panic!("expected agent logs"),
        }
    }
```

- [ ] **Step 4: Прогнать тесты и закоммитить**

Run: `rtk cargo test`
Expected: PASS.

```bash
rtk git add -A && rtk git commit -m "feat(bin): pi doctor and agent status/logs with ssh fallback"
```

---

### Task 20: Финал — доки, dev-конфиг, версия 0.4.0

**Files:**
- Modify: `Cargo.toml` (workspace `version = "0.4.0"`)
- Modify: `dev/agent.toml`
- Modify: `docs/install-agent-v0.1.md`
- Modify: `README.md`

- [ ] **Step 1: dev/agent.toml**

Добавить в конец:

```toml
# [logs]                          # rolling agent log files (§14)
# dir = ".dev-logs"               # default on the Pi: /var/log/pi
# retention_days = 14
```

- [ ] **Step 2: docs/install-agent-v0.1.md**

В секцию про `agent.toml` добавить блок:

```markdown
### v0.4: логи и диагностика

```toml
[logs]
dir = "/var/log/pi"        # rolling-файлы pi-agent.log.YYYY-MM-DD
retention_days = 14        # файлы старше удаляются при старте агента
```

Новые команды CLI: `pi logs <project> [-f]`, `pi stats [project]`,
`pi start|stop|restart <project>`, `pi rm <project> [--volumes]`, `pi status`,
`pi doctor`, `pi agent status`, `pi agent logs [-f] [--since 2h]`.
Если агент недоступен, `pi agent status|logs` автоматически фолбэчатся на
`systemctl status pi-agent` / `journalctl -u pi-agent` через ssh.
Каталог `/var/log/pi` должен принадлежать пользователю `pi-agent`
(`install -d -o pi-agent -g pi-agent /var/log/pi`).
```

- [ ] **Step 3: README.md**

Обновить строку статуса:

```markdown
Status: v0.4 (Операционка) — deploy/env/ingress/CI (v0.1–v0.3) + `pi logs`,
`pi stats`, `pi start|stop|restart`, `pi rm`, `pi status`, `pi doctor`,
`pi agent status|logs`, rolling-логи агента. Установка одной командой и
`pi agent setup` — v0.5 (§23 spec).
```

- [ ] **Step 4: Версия 0.4.0**

В `Cargo.toml` (workspace): `version = "0.4.0"`.

- [ ] **Step 5: Полный прогон и коммит**

Run:
```bash
rtk cargo fmt
rtk cargo clippy --workspace --all-targets
rtk cargo test
```
Expected: clippy без warning'ов, тесты PASS.

```bash
rtk git add -A && rtk git commit -m "docs: v0.4 agent options and commands; release v0.4.0"
```

---

## Self-Review (выполнен при написании плана)

1. **Покрытие спеки v0.4:** `pi logs` → Tasks 2/7/16/18; `pi stats` → Tasks 1/2/3/8/9/15/18; lifecycle → Tasks 1/2/10/15/18; `pi rm` (+волюмы, подтверждение, Conflict, DNS-инструкция) → Tasks 2/3/4/5/6/11/15/18; `pi status` → Tasks 1/13/15/18; `pi doctor` (клиент+агент, диск через DiskProbe) → Tasks 1/12/13/15/19; `pi agent status|logs` + ssh-фолбэк → Tasks 14/16/17/19; rolling-логи + ретеншен → Task 14; версия 0.4.0 и доки → Task 20.
2. **Типы согласованы:** `ServiceStats`/`StatsReport`/`DiagnosticCheck`/`LifecycleAction`/`AgentOverview` определены в Task 1 и используются в Tasks 2/8/9/12/13/15; `StreamLogs::ensure_project` (Task 7) используется в Task 16; `stream_sse` (Task 17) — в Tasks 18/19; `DEFAULT_LOG_TAIL` (Task 7) — в Task 16.
3. **Отклонение от дизайна** (зафиксировано в «Решениях»): `ContainerRuntime::stats` принимает один проект вместо списка — агрегацию делает `CompositeStats`; `OverrideStore::remove` добавлен (артефакт `overrides/<name>.yml` иначе осиротел бы).




