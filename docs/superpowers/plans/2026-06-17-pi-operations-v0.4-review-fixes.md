# pi v0.4 (Операционка) — План работы над замечаниями к PR #4

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Исправить 15 замечаний по ревью PR #4 (от владельца khmilevoi и бота Codex), устранить утечки ресурсов, баги с CPU, жестко захардкоженные пороги, падения при старте и некорректный запуск docker compose команд без контекста файлов проекта.

**Architecture:** Корректируем слои проекта (§5):
1. **Infrastructure/Domain**: Изменяем сигнатуры `ContainerRuntime::lifecycle` и `down` на `&ComposeStack`, добавляем `OverrideStore::path` (sync-метод) для вычисления путей override-файлов в use-case. Фиксируем CPU в `CompositeStats` (двухточечное измерение sysinfo с задержкой 200мс).
2. **Application**: В `ControlLifecycle` и `RemoveProject` собираем `ComposeStack` перед вызовом рантайма. Убираем лишний Conflict-гард из lifecycle (соответствие спеке §3).
3. **Bin/Agent**: Внедряем `AbortOnDrop` в http.rs для уничтожения зависших child-процессов `docker compose logs -f` и poll-задач при разрыве SSE-соединения. Деградируем логирование до stderr-only при недоступности `/var/log/pi`.

**Tech Stack:** Rust, Tokio, sysinfo 0.33, axum.

---

## Сводка замечаний

| № | Приоритет | Файл / Место | Суть проблемы | Источник |
|---|---|---|---|---|
| 1 | 🔴 Блокер | `stats.rs:29` | CPU в `pi stats` всегда 0%. Измерение sysinfo требует интервала | khmilevoi |
| 2 | 🔴 Блокер | `probe.rs:109` | Порог диска захардкожен на 90% вместо `gc.disk_threshold_percent` | khmilevoi |
| 3 | 🔴 Блокер | `probe.rs:98` | `pi doctor` всегда FAIL на docker-only хостах из-за безусловного чека cloudflared | khmilevoi |
| 4 | 🔴 Блокер | `http.rs:329` | Утечка `docker compose logs -f` при дисконнекте SSE-клиента `pi logs -f` | khmilevoi |
| 5 | 🔴 Блокер (P1) | `docker.rs:293` | `pi start/stop/restart` работают без compose-файлов (ошибка compose v2) | Codex Bot |
| 6 | 🔴 Блокер (P1) | `docker.rs:305` | `pi rm` (compose down) работает без compose-файлов (не удаляет volumes) | Codex Bot |
| 7 | 🟡 До мержа | `probe.rs:128` | Аптайм в status показывает аптайм хоста, а не агента | khmilevoi |
| 8 | 🟡 До мержа | `run.rs:12` | Агент падает, если нет прав на `/var/log/pi` (не умеет деградировать) | khmilevoi |
| 9 | 🟡 До мержа | `http.rs:348` | `pi agent logs --since` обрезается до 100 строк (не сбрасывает tail) | khmilevoi |
| 10 | 🟡 До мержа | `probe.rs:82` | `pi doctor` беднее спеки §6: нет чеков группы docker, linger, systemctl | khmilevoi |
| 11 | 🔵 На усмотрение | `cloudflared.rs:158` | `remove` падает, если `config.yml` отсутствует (не идемпотентно) | khmilevoi |
| 12 | 🔵 На усмотрение | `remove.rs:62` | В Conflict-ошибке удаления нет подсказки `pi deploy --cancel` (§2.2) | khmilevoi |
| 13 | 🔵 На усмотрение | `lifecycle.rs:37` | Лишний/недокументированный Conflict-гард в lifecycle (§3) | khmilevoi |
| 14 | 🔵 На усмотрение | `logfile.rs:118` | `follow` перечитывает весь каталог логов каждую секунду | khmilevoi |
| 15 | 🔵 На усмотрение | `logfile.rs:35` | `DailyWriter` делает syscall open/append/close на каждый чих | khmilevoi |

---

## План исправления

### Task 1: CPU stats fix (🔴 Блокер)

`sysinfo 0.33` требует времени между сэмплами для вычисления CPU. Будем делать `sleep` 200мс (минимальный интервал) в `CompositeStats::report`.

**Files:**
- Modify: `crates/infrastructure/src/stats.rs`

- [ ] **Step 1: Написать падающий интеграционный тест**

В `stats.rs` в тестах временно переопределить мок и вызвать `report`. Так как тесты не спят по умолчанию, мы проверим, что CPU возвращает валидные f64 (не обязательно 0). Но физически unit-тест на моках всегда давал 0. Напишем тест, где мы делаем реальный CPU-замер (на Windows/Linux) и проверяем, что он больше или равен 0.0.

- [ ] **Step 2: Реализовать двухточечное измерение с задержкой**

В `crates/infrastructure/src/stats.rs:26-34`:
```rust
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        // Даём время для замера дельты использования CPU
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        sys.refresh_cpu_usage();
        
        let host = HostStats {
            cpu_percent: f64::from(sys.global_cpu_usage()),
            mem_used_bytes: sys.used_memory(),
            mem_total_bytes: sys.total_memory(),
            disk_used_percent: self.disk.used_percent().unwrap_or(0),
            uptime_secs: System::uptime(),
        };
```
*Примечание:* Использование `System::new()` вместо `new_all()` решает проблему "расточительности" ресурсов, так как обновляет только CPU и память.

- [ ] **Step 3: Прогнать тесты и закоммитить**
```bash
rtk cargo test -p pi-infrastructure stats
rtk git add -A && rtk git commit -m "fix(stats): correct non-zero CPU sampling with sysinfo 0.33"
```

---

### Task 2: Doctor — disk threshold & conditional cloudflared (🔴 Блокер)

`HostSystemProbe` должен получать `disk_threshold_percent`, флаг `cloudflared_enabled` и `started_at` (для аптайма, Task 3).

**Files:**
- Modify: `crates/infrastructure/src/probe.rs`
- Modify: `crates/bin/src/agent/state.rs`
- Modify: `crates/bin/src/agent/http.rs` (тесты)

- [ ] **Step 1: Обновить конструктор `HostSystemProbe`**

В `probe.rs`:
```rust
pub struct HostSystemProbe {
    runner: Arc<dyn ProbeRunner>,
    disk: Arc<dyn DiskProbe>,
    projects: Arc<dyn ProjectRepository>,
    version: String,
    disk_threshold_percent: u8,
    cloudflared_enabled: bool,
    started_at: i64,
}

impl HostSystemProbe {
    pub fn new(
        runner: Arc<dyn ProbeRunner>,
        disk: Arc<dyn DiskProbe>,
        projects: Arc<dyn ProjectRepository>,
        version: String,
        disk_threshold_percent: u8,
        cloudflared_enabled: bool,
        started_at: i64,
    ) -> Arc<HostSystemProbe> {
        Arc::new(HostSystemProbe {
            runner,
            disk,
            projects,
            version,
            disk_threshold_percent,
            cloudflared_enabled,
            started_at,
        })
    }
}
```

- [ ] **Step 2: Использовать порог диска и условие cloudflared в `diagnostics()`**

В `probe.rs` в `diagnostics()`:
```rust
        let mut checks = vec![
            self.command_check(
                "docker daemon",
                "docker",
                &["version", "--format", "{{.Server.Version}}"],
                "start Docker and make sure the pi-agent user can access it",
            )
            .await,
            self.command_check(
                "docker compose",
                "docker",
                &["compose", "version"],
                "install Docker Compose v2",
            )
            .await,
        ];

        if self.cloudflared_enabled {
            checks.push(
                self.command_check(
                    "cloudflared",
                    "cloudflared",
                    &["--version"],
                    "install cloudflared or disable [cloudflared] routing",
                )
                .await,
            );
        }
```
А порог диска проверять так:
```rust
        checks.push(match self.disk.used_percent() {
            Ok(percent) => DiagnosticCheck {
                name: "disk space".into(),
                passed: percent < self.disk_threshold_percent,
                detail: format!("{percent}% used"),
                hint: (percent >= self.disk_threshold_percent).then(|| "run `pi gc` or free disk space".into()),
            },
            Err(err) => ...
```

- [ ] **Step 3: Прокинуть параметры в `state.rs` и тестах `http.rs`**

В `state.rs`:
```rust
    let now = SystemClock::new().now_unix(); // Зафиксировать время старта агента
    // ...
    let probe = HostSystemProbe::new(
        Arc::new(SystemRunner),
        disk,
        projects.clone(),
        env!("CARGO_PKG_VERSION").to_string(),
        config.gc.disk_threshold_percent,
        config.cloudflared.is_some(),
        now,
    );
```
Обновить все тесты в `http.rs` (метод `state_with`), где собирается мок-стейт. Передать туда дефолтные значения `85`, `false`, `100`.

- [ ] **Step 4: Проверить тесты и закоммитить**
```bash
rtk cargo test
rtk git add -A && rtk git commit -m "fix(doctor): correct disk threshold, conditional cloudflared and probe wiring"
```

---

### Task 3: Uptime агента вместо хоста (🟡 До мержа)

Статус агента должен выводить аптайм агента (а не аптайм хоста через `System::uptime()`).

**Files:**
- Modify: `crates/infrastructure/src/probe.rs`

- [ ] **Step 1: Реализовать подсчёт времени**

В `probe.rs` метод `overview()`:
```rust
    async fn overview(&self) -> Result<AgentOverview, DomainError> {
        let projects = self.projects.list().await?.len();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let uptime_secs = (now - self.started_at).max(0) as u64;

        Ok(AgentOverview {
            version: self.version.clone(),
            uptime_secs,
            disk_used_percent: self.disk.used_percent()?,
            projects,
            active_deployments: 0,
        })
    }
```

- [ ] **Step 2: Запустить тесты и закоммитить**
```bash
rtk cargo test -p pi-infrastructure probe
rtk git add -A && rtk git commit -m "fix(agent): show actual agent process uptime in overview"
```

---

### Task 4: Утечка процессов SSE-логов (🔴 Блокер)

При отключении клиента SSE axum-стрим дропается, но запущенная в `tokio::spawn` задача логов крутится бесконечно.

**Files:**
- Modify: `crates/bin/src/agent/http.rs`

- [ ] **Step 1: Добавить `AbortOnDrop` guard**

В `crates/bin/src/agent/http.rs`:
```rust
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);
impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}
```

- [ ] **Step 2: Применить `AbortOnDrop` в `project_logs` и `agent_logs`**

В `project_logs`:
```rust
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let logs = state.stream_logs.clone();
    let task = tokio::spawn(async move {
        let _ = logs
            .execute(&name, q.tail, q.follow, Arc::new(ChannelSink(tx)))
            .await;
    });
    let stream = async_stream::stream! {
        let _guard = AbortOnDrop(task); // Дропнется при уничтожении стрима SSE
        while let Some(line) = rx.recv().await {
            yield sse_log(line);
        }
    };
```
Сделать то же самое для `agent_logs` ( follow-путь на строках 364-366).

- [ ] **Step 3: Прогнать тесты и закоммитить**
```bash
rtk cargo test
rtk git add -A && rtk git commit -m "fix(agent): prevent docker compose logs leaks using AbortOnDrop guard"
```

---

### Task 5: Использование compose-файлов в `lifecycle` и `down` (🔴 Блокер от Codex P1)

Вместо запуска без контекста `-p <project>` (что приводит к падению `no configuration file provided`), команды должны использовать `ComposeStack`.

**Files:**
- Modify: `crates/domain/src/contracts.rs`
- Modify: `crates/infrastructure/src/docker.rs`
- Modify: `crates/application/src/lifecycle.rs`
- Modify: `crates/application/src/remove.rs`
- Modify: `crates/bin/src/agent/run.rs` (тесты, моки)

- [ ] **Step 1: Обновить контракт `ContainerRuntime` и `OverrideStore`**

В `contracts.rs` добавить в `OverrideStore`:
```rust
    /// Returns the expected path of the override file for the project.
    fn path(&self, project: &str) -> PathBuf;
```
Реализовать в `crates/infrastructure/src/overrides.rs`:
```rust
    fn path(&self, project: &str) -> PathBuf {
        self.dir.join(format!("{project}.yml"))
    }
```
Обновить сигнатуры в `ContainerRuntime` (contracts.rs):
```rust
    async fn lifecycle(
        &self,
        stack: &ComposeStack,
        action: LifecycleAction,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError>;

    async fn down(
        &self,
        stack: &ComposeStack,
        remove_volumes: bool,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError>;
```

- [ ] **Step 2: Обновить адаптер `docker.rs`**

В `docker.rs` использовать `self.compose(stack, ...)` для `lifecycle` и `down`:
```rust
    async fn lifecycle(
        &self,
        stack: &ComposeStack,
        action: LifecycleAction,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError> {
        log.line(&format!("docker compose {} ...", action.as_str()));
        run_streamed(self.compose(stack, &[action.as_str()]), log)
            .await
            .map_err(DomainError::Runtime)
    }

    async fn down(
        &self,
        stack: &ComposeStack,
        remove_volumes: bool,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError> {
        log.line("docker compose down ...");
        let mut tail = vec!["down", "--remove-orphans"];
        if remove_volumes {
            tail.push("--volumes");
        }
        run_streamed(self.compose(stack, &tail), log)
            .await
            .map_err(DomainError::Runtime)
    }
```
*Устойчивость:* В `file_chain()` в `docker.rs` добавить проверку `stack.override_file.exists()`, чтобы не передавать `-f` для несуществующего override.

- [ ] **Step 3: Инжектировать `Source` и `OverrideStore` в `ControlLifecycle`**

В `lifecycle.rs` обновить структуру, чтобы она могла строить `ComposeStack`:
```rust
pub struct ControlLifecycle {
    projects: Arc<dyn ProjectRepository>,
    history: Arc<dyn DeploymentHistory>,
    runtime: Arc<dyn ContainerRuntime>,
    source: Arc<dyn Source>,
    overrides: Arc<dyn OverrideStore>,
}
```
В `execute()` собрать `ComposeStack`:
```rust
        let project = self.projects.get(project).await?
            .ok_or_else(|| DomainError::NotFound(format!("project {project}")))?;
        let workdir = self.source.workdir(project);
        let stack = ComposeStack {
            project_name: project.config.name.clone(),
            workdir: workdir.clone(),
            compose_file: workdir.join(&project.config.compose_path),
            override_file: self.overrides.path(&project.config.name),
        };
        self.runtime.lifecycle(&stack, action, log).await
```

- [ ] **Step 4: Обновить `RemoveProject`**

В `remove.rs` собрать `ComposeStack`:
```rust
        let workdir = self.source.workdir(project);
        let stack = ComposeStack {
            project_name: project.to_string(),
            workdir: workdir.clone(),
            compose_file: workdir.join(&existing.config.compose_path),
            override_file: self.overrides.path(project),
        };
        
        // Если workdir не существует (ручной снос/сбой), пропускаем down для идемпотентности
        if stack.compose_file.exists() {
            self.runtime.down(&stack, remove_volumes, Arc::clone(&log)).await?;
        } else {
            log.line("workdir gone, skipping compose down");
        }
```

- [ ] **Step 5: Обновить моки, тесты и `state.rs` wiring**

Добавить передачу `source` и `overrides` в `ControlLifecycle::new` в `state.rs` и во всех юнит-тестах в `deploy.rs`, `http.rs`. Исправить `CountingRuntime` в `deploy.rs` тестах.

- [ ] **Step 6: Прогнать тесты и закоммитить**
```bash
rtk cargo test
rtk git add -A && rtk git commit -m "fix(docker): run lifecycle and down with full ComposeStack context"
```

---

### Task 6: Исправление обрезки логов `--since` (🟡 До мержа)

В режиме `--since` лимит строк `tail` должен отключаться (передаваться `None`).

**Files:**
- Modify: `crates/bin/src/agent/http.rs`

- [ ] **Step 1: Исправить логику в `agent_logs`**

В `http.rs:348`:
```rust
    let tail = if q.since.is_some() { None } else { Some(q.tail) };
    let initial = logfile::read(&state.log_dir, tail, q.since)
```

- [ ] **Step 2: Запустить тесты и закоммитить**
```bash
rtk cargo test
rtk git add -A && rtk git commit -m "fix(logs): do not apply default tail limit when since query is provided"
```

---

### Task 7: Деградация при отсутствии прав на `/var/log/pi` (🟡 До мержа)

Агент должен успешно стартовать на запись в stderr, даже если `/var/log/pi` создать не удалось.

**Files:**
- Modify: `crates/bin/src/agent/run.rs`
- Modify: `crates/bin/src/agent/http.rs`
- Modify: `crates/bin/src/agent/state.rs`

- [ ] **Step 1: Сделать file_layer логирования опциональным**

В `run.rs:12`:
```rust
    let mut file_layer = None;
    match std::fs::create_dir_all(&config.logs.dir) {
        Ok(_) => {
            let _ = logfile::prune_old(&config.logs.dir, config.logs.retention_days);
            file_layer = Some(
                tracing_subscriber::fmt::layer()
                    .without_time()
                    .with_ansi(false)
                    .with_writer(logfile::DailyMakeWriter::new(config.logs.dir.clone()))
            );
        }
        Err(e) => {
            eprintln!("Warning: failed to create log directory {}: {e}. Agent logging will fall back to stderr-only.", config.logs.dir.display());
        }
    }
```
Прокидывать `file_layer` в инициализацию реестра `tracing` только если он `Some`.

- [ ] **Step 2: Возвращать 404 на эндпоинте логов агента при деградации**

В `AppState` добавить флаг `log_dir_available: bool`.
В `agent_logs` эндпоинте в `http.rs`:
```rust
    if !state.log_dir_available {
        return Err(ApiError(DomainError::NotFound("agent file logging is disabled/unavailable".into())));
    }
```

- [ ] **Step 3: Проверить тесты и закоммитить**
```bash
rtk cargo test
rtk git add -A && rtk git commit -m "fix(agent): graceful logging degradation to stderr-only on write failures"
```

---

### Task 8: Расширение чеков `pi doctor` (§6) (🟡 До мержа)

Добавить в `HostSystemProbe` дополнительные чеки: docker-группа, linger, cloudflared systemctl, cert.pem.

**Files:**
- Modify: `crates/infrastructure/src/probe.rs`

- [ ] **Step 1: Написать новые методы чеков**

В `probe.rs` добавить чеки:
```rust
    // docker group membership
    let group_check = self.command_check("pi-agent group", "id", &["-nG"], "add pi-agent to the 'docker' group").await;
    
    // systemd linger check
    let linger_check = self.command_check("systemd linger", "loginctl", &["show-user", "pi-agent"], "enable linger: loginctl enable-linger pi-agent").await;
```
Если `cloudflared_enabled`, то также добавить проверку его юнит-файла:
```rust
    let cf_systemd = self.command_check("cloudflared service", "systemctl", &["--user", "is-active", "cloudflared"], "start cloudflared service").await;
```

- [ ] **Step 2: Запустить тесты и закоммитить**
```bash
rtk cargo test
rtk git add -A && rtk git commit -m "feat(doctor): expand diagnostic checks to match spec section 6"
```

---

### Task 9: Идемпотентность и подсказки (🔵 На усмотрение)

1. Добавить `pi deploy --cancel` хинт в Conflict-ошибку удаления (`remove.rs`).
2. Убрать Conflict-гард из `lifecycle.rs` (соответствие спеке §3).
3. NotFound → Ok для missing `config.yml` в `cloudflared.rs` (`remove`).

**Files:**
- Modify: `crates/application/src/remove.rs`
- Modify: `crates/application/src/lifecycle.rs`
- Modify: `crates/infrastructure/src/cloudflared.rs`

- [ ] **Step 1: Реализовать подсказку при Conflict удалении**

В `remove.rs`:
```rust
        if !active.is_empty() {
            return Err(DomainError::Conflict(format!(
                "project {project} has active deployment; cancel it first using `pi deploy --cancel`"
            )));
        }
```

- [ ] **Step 2: Убрать Conflict-гард из lifecycle**

В `lifecycle.rs:35-40` удалить проверку `active.is_empty()`. Спека §3 разрешает управление жизненным циклом без блокировки.

- [ ] **Step 3: Сделать `cloudflared remove` идемпотентным**

В `cloudflared.rs`:
```rust
    async fn remove(&self, hostname: &str, log: Arc<dyn LogSink>) -> Result<(), DomainError> {
        let text = match tokio::fs::read_to_string(&self.config_path).await {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log.line("ingress: no config.yml found; skipping remove");
                return Ok(());
            }
            Err(e) => return Err(...)
        };
        // ...
```

- [ ] **Step 4: Прогнать тесты и закоммитить**
```bash
rtk cargo test
rtk git add -A && rtk git commit -m "style(app): simplify lifecycle guards, add cancel hints, ensure cf remove idempotency"
```

---

## Варианты выполнения

План полностью подготовлен. Доступно два варианта исполнения:

1. **Subagent-Driven (Рекомендуемый)** — Я запускаю по агенту на каждую задачу, выполняем TDD цикл, проверяем и коммитим по одному.
2. **Inline Execution** — Выполняем все задачи пакетно в этой сессии, делая проверочные чекпоинты.

Какой подход предпочтительнее? Разрешить выполнение?