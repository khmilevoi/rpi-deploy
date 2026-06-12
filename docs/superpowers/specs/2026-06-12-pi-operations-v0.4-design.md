# pi v0.4 — Операционка (дизайн)

Дата: 2026-06-12. Базовая спека: `2026-06-09-pi-deploy-tool-design.md` (§23 v0.4).

**Критерий готовности:** проблема диагностируется штатными командами,
без ssh-археологии — в том числе когда агент мёртв.

## 1. Скоуп

Входит (§23 v0.4 + одно дополнение):

- `pi doctor` — самодиагностика PASS/FAIL с подсказками (§14).
- `pi agent logs [-f] [--since]`, `pi agent status` + rolling-файлы логов
  агента (§14).
- `pi stats [project]` — live-метрики хоста и проектов (`CompositeStats`:
  `ContainerRuntime` + `sysinfo`).
- `pi start|stop|restart <project>` (`ControlLifecycle`).
- `pi rm <project> [--volumes] [--yes]` (+DNS-инструкция, §11).
- `pi status` — обзор агента и хоста.
- **Дополнение к роадмапу:** `pi logs <project> [-f] [--tail N]` — логи
  контейнеров проекта (`StreamLogs`). Команда заявлена в §16, но не была
  закреплена за версией; для «диагностики без ssh-археологии» логи
  контейнеров нужны не меньше, чем логи агента.

НЕ входит:

- `stats_snapshots` и история метрик — YAGNI: потребителя истории нет до
  веб-дашборда (пост-1.0). `pi stats` показывает только текущие значения,
  фоновых записей в БД нет. Упоминание «таблица появится в v0.4» из плана
  v0.3 этим решением отменено.
- `pi agent setup/update/uninstall`, `pi setup`, `pi init`, install.sh — v0.5.
- Интерактивный «отменить?» при Ctrl+C во время follow — v0.5.

## 2. Ключевые решения

1. **API-first + ssh-фолбэк.** Все новые фичи работают через HTTP API
   агента. Но `pi agent logs`, `pi agent status` и `pi doctor` при
   недоступном API не падают, а фолбэчатся на ssh-exec
   (`journalctl -u pi-agent`, `systemctl status pi-agent`) — диагностика
   работает даже при мёртвом агенте. CLI уже шеллит системный `ssh` для
   туннеля; добавляется небольшой `SshExec` рядом.
2. **`pi rm` без сноса данных по умолчанию.** `compose down` (named-волюмы
   живут); `--volumes` добавляет `-v`. Сносятся: контейнеры, ingress-правило,
   workdir, секреты, deploy-key, порт-аллокация, записи БД (проект +
   история). DNS-запись остаётся — CLI печатает инструкцию (§11). Без
   `--yes` команда просит ввести имя проекта (точное совпадение). При
   активном деплое (queued/running) — отказ `Conflict` с подсказкой
   «сначала `pi deploy --cancel`».
3. **Без истории метрик** (см. скоуп).
4. **Lifecycle — нативные compose-подкоманды**: `compose start|stop|restart`,
   без rebuild и без записи в историю деплоев (это не деплой).
5. **Порог диска для doctor** переиспользует `gc.disk_threshold_percent`
   из `agent.toml` — отдельной настройки нет.

## 3. Поверхность: CLI и HTTP API

Новые команды (все принимают `ConnectOpts`, как в v0.3):

| Команда | Поведение |
|---|---|
| `pi logs <project> [-f] [--tail N]` | стрим логов контейнеров проекта с агента; значения секретов маскируются (§8.1) |
| `pi stats [project]` | без аргумента — хост (CPU/RAM/диск/uptime) + все проекты; с проектом — его сервисы (CPU/mem) + last deploy |
| `pi start\|stop\|restart <project>` | `compose start/stop/restart` |
| `pi rm <project> [--volumes] [--yes]` | снос проекта (решение 2) + DNS-инструкция |
| `pi status` | версии CLI/агента, аптайм агента, диск %, число проектов, активные деплои |
| `pi doctor` | PASS/FAIL-чеклист: клиентские чеки (ssh-коннект, ответ агента, совпадение версий) + агентские (`GET /v1/doctor`); exit-код 1 при любом FAIL |
| `pi agent status` | через API (версия/аптайм/сводка); фолбэк — ssh `systemctl status pi-agent` |
| `pi agent logs [-f] [--since]` | rolling-файл агента через API; фолбэк — ssh `journalctl -u pi-agent` |

`pi stats` и `pi status` поддерживают `--json`.

Новые эндпоинты агента:

- `GET /v1/projects/{name}/logs?tail=N&follow=true` — SSE (как deploy-логи).
- `GET /v1/stats`, `GET /v1/projects/{name}/stats`.
- `POST /v1/projects/{name}/lifecycle` — `{"action": "start"|"stop"|"restart"}`.
- `DELETE /v1/projects/{name}?volumes=true|false`.
- `GET /v1/status` — `AgentOverview`.
- `GET /v1/doctor` — `DiagnosticReport` (агентские чеки).
- `GET /v1/agent/logs?tail=N&since=<unix>&follow=true` — SSE из rolling-файла.

Rolling-логи агента: `tracing-appender`, ротация по дням, каталог и ретеншен
из `agent.toml` `[logs]` (дефолт ретеншена — 14 дней, чистка старых файлов
при старте). Слой stderr→journald остаётся.

## 4. Domain

Сущности (`entities.rs`):

- `ServiceStats` — `service`, `cpu_percent`, `mem_used_bytes`, `mem_limit_bytes`.
- `ProjectStats` — `project`, `services: Vec<ServiceStats>`,
  `last_deploy: Option<Deployment>`.
- `HostStats` — `cpu_percent`, `mem_used_bytes`, `mem_total_bytes`,
  `disk_used_percent`, `uptime_secs`.
- `StatsReport` — `host: HostStats`, `projects: Vec<ProjectStats>`.
- `DiagnosticCheck` — `name`, `passed: bool`, `detail`, `hint: Option<String>`;
  `DiagnosticReport` — `checks` + `all_passed()`.
- `LifecycleAction` — `Start | Stop | Restart` (+ `as_str`/`FromStr`).
- `AgentOverview` — `version`, `uptime_secs`, `disk_used_percent`,
  `projects: usize`, `active_deployments: usize`.

Контракты (`contracts.rs`):

- `ContainerRuntime` +: `logs(project, tail, follow, sink)`,
  `stats(projects) -> Vec<(String, Vec<ServiceStats>)>`,
  `lifecycle(stack, action, sink)`, `down(stack, remove_volumes, sink)`.
  Остаётся единственной точкой знания про docker.
- `Ingress` +: `remove(hostname, sink)` — снять правило, рестарт только при
  diff; DNS не трогаем.
- `SecretStore` +: `remove(project)`.
- `Source` +: `cleanup(project)` — workdir и deploy-key проекта.
- `ProjectRepository` +: `remove(name)` — запись проекта + порт-аллокация.
- `DeploymentHistory` +: `remove_project(project)`.
- Новый `StatsProvider` — `report(projects) -> StatsReport` (§6).
- Новый `SystemProbe` — `run_checks() -> Vec<DiagnosticCheck>` (§6).

## 5. Application (use-cases)

- `StreamLogs` — резолв проекта (`NotFound`, если нет) →
  `ContainerRuntime::logs` через `MaskingSink` с секретами проекта.
- `GetStats` — проекты из репозитория (или один) → `StatsProvider::report`;
  last deploy — из `DeploymentHistory`.
- `ControlLifecycle` — резолв проекта → `ContainerRuntime::lifecycle`.
- `RemoveProject` — порядок: `DeploymentHistory::active` не пуст → `Conflict`;
  затем `down(remove_volumes)` → `Ingress::remove` → `Source::cleanup` →
  `SecretStore::remove` → `DeploymentHistory::remove_project` →
  `ProjectRepository::remove`. Возвращает отчёт (что снесено + hostname для
  DNS-инструкции). Ошибка любого шага — стоп с понятным сообщением; все шаги
  идемпотентны, повторный `pi rm` доделывает остальное.
- `RunDiagnostics` — `SystemProbe::run_checks` + чек диска через `DiskProbe`
  → `DiagnosticReport`.
- `AgentStatus` — `AgentOverview` из времени старта агента, `DiskProbe`,
  `ProjectRepository`, `DeploymentHistory`.

Новые варианты `DomainError` не нужны (`Conflict`/`NotFound` есть с v0.3).

## 6. Infrastructure

- `docker.rs` +: `compose logs --tail N [-f]` (через `run_streamed`),
  `docker stats --no-stream --format json` (построчный JSON),
  `compose start|stop|restart`, `compose down [-v]`.
- `stats.rs` (новый) — `CompositeStats`: контейнерные метрики из
  `ContainerRuntime::stats` + хостовые из `sysinfo`.
- `probe.rs` (новый) — `HostSystemProbe`. Чеки: `docker info`,
  `docker compose version`, `cloudflared` в PATH,
  `systemctl --user is-active cloudflared`, членство `pi-agent` в группе
  docker (`id -nG`), linger (`loginctl show-user`), наличие `cert.pem`,
  диск ниже порога. Команды выполняются через инжектируемый runner —
  юнит-тесты на Windows с фейковым выводом; реальная система —
  `#[ignore]`-интеграция.
- `cloudflared.rs` — `remove`: убрать правило из `config.yml`, рестарт
  только при diff (логика diff уже есть).
- `secrets.rs`/`git.rs`/`repo.rs`/`history.rs` — методы удаления; все
  идемпотентны (нет файла/строки — `Ok`).

## 7. Bin

Агент:

- `http.rs` — роуты из §3; SSE проектных и агентских логов по образцу
  deploy-логов (канальный sink).
- Rolling-лог: `tracing-appender` (daily) + чистка файлов старше
  `retention_days` при старте; `GET /v1/agent/logs` читает файлы по
  `since`/`tail`; `follow` — хвост через периодический poll.
- `config.rs` — секция `[logs]` (`dir`, `retention_days`, дефолт 14).
- `state.rs` — wiring новых use-cases; фиксируется время старта (uptime).

CLI:

- `commands.rs` — команды из §3; `pi rm` без `--yes` запрашивает точное имя
  проекта; doctor печатает `PASS/FAIL имя — detail (hint)`.
- `ssh.rs` (новый) — `SshExec`: `ssh <host> <cmd>` с теми же опциями, что
  туннель; фолбэк для `pi agent logs|status`, чек «агент недоступен» для
  `pi doctor`.
- `proto.rs` — DTO: `StatsReportDto`, `DiagnosticReportDto`,
  `AgentOverviewDto`, `LifecycleRequest`, `RemoveResponse`.

## 8. Тестирование

- Use-cases — юнит-тесты на мок-контрактах (без docker), как в v0.2/v0.3.
- `HostSystemProbe` — юнит на фейковом runner'е; реальная система —
  `#[ignore]`.
- HTTP-роуты — тесты в стиле v0.3 (`state_with`).
- Интеграция с реальным docker/cloudflared — `#[ignore]`.

Версия по завершении: **0.4.0**.
