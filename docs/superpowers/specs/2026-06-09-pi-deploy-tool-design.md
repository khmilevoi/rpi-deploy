# pi — деплой-тула для Raspberry Pi (design spec)

- **Дата:** 2026-06-09
- **Статус:** утверждён к планированию
- **Автор:** khmilevoi@gmail.com

## 1. Контекст и цель

Raspberry Pi используется как домашний сервер. Нужна CLI-тула на Rust, которая
позволяет из проекта (репозитория) описать сборку/запуск в конфиге и развернуть
проект на Pi через Docker, отправить секреты по защищённому каналу, управлять
проектами и смотреть статистику. Тула должна интегрироваться в CI/CD (деплой
после тестов) и архитектурно поддерживать будущий приём GitHub-вебхуков.

**Целевой сценарий:** несколько проектов на одном Pi, каждый — за поддоменом
(напр. `rateme.isskelo.com`) через Cloudflare Tunnel. Деплой: `pi deploy`
вызывается вручную или из GitHub Actions после прохождения тестов.

**UX-ожидания:** агент ставится из репозитория одной командой и сам себя
настраивает (папки, ключи, фоновый процесс). У самой тулы есть наблюдаемые логи
и самодиагностика, чтобы чинить проблемы. На клиенте — интерактивный CLI для
настройки подключения и проекта.

## 2. Ключевые решения (зафиксированы в брейншторме)

| Тема | Решение |
|---|---|
| Архитектура связи | Демон-агент `pi-agent` на Pi (systemd) |
| Транспорт CLI↔агент | HTTP API (axum) на unix-сокете `/run/pi/agent.sock` (`0660`); SSH-туннель форвардит на сокет |
| Аутентификация | SSH-ключи (переиспользуем существующие); агент не торчит в интернет |
| Источник кода (v1) | git/GitHub, сборка на Pi; абстракция `Source` под будущие способы |
| Единица запуска | docker-compose (одиночный сервис — частный случай) |
| Триггер деплоя (v1) | `pi deploy` по SSH (CI/CD + вручную); webhook-receiver — заложенное расширение |
| Формат конфига | `pi.toml` (serde) |
| Маршрутизация | Cloudflare Tunnel + поддомены; агент авто-правит ingress + DNS; абстракция `Ingress` |
| Стиль | Чистая архитектура, DI/IoC, инкапсуляция через контракты-трейты |
| DI-механизм | Ручной composition root + `Arc<dyn Trait>` (constructor injection) |
| State | SQLite в data-dir агента |
| Шифрование env на диске | `EncryptedFileStore` — секреты шифруются ключом агента (`0600`) |
| Стратегия отката (v1) | In-place `up -d --build`; health-check — гейт (success/failed); авто-откат → v2 |
| Аллокация портов | Стабильный host-порт на проект из диапазона 8000–8999 |
| Установка агента | one-line install-скрипт + `pi agent setup` (systemd system-сервис под юзером `pi-agent`) |
| Cloudflared | locally-managed, отдельный systemd-юнит под `pi-agent`; reload без sudo (linger + `systemctl --user`) |
| Привязка сервера | `pi deploy` → default-профиль клиента; override `--server`/`PI_SERVER`; CI — флаги `--host/--user/--key` |
| Логи тулы | journald (live) + rolling-файл (история) + `pi doctor` |
| Интерактив клиента | `inquire`: `pi setup` (сервер) + `pi init` (проект); неинтерактивный режим флагами |
| Docker-адаптер | `docker compose` CLI (shell-out) + `docker stats --format json` |
| Git-адаптер | `git` CLI (shell-out) |
| SQLite-адаптер | `rusqlite` (sync, обёрнут в `spawn_blocking`), миграции `rusqlite_migration` |
| SSH-транспорт | системный `ssh -L` (shell-out) |
| Краш-рекавери | in-memory локи; при старте свип `running` → `interrupted` |
| Очередь деплоев | глубина 1 на проект, latest wins (вытесненный → `superseded`) |
| Таймауты деплоя | поэтапные (fetch/build/up) + отмена (`pi deploy --cancel`) |
| GC диска | `image prune` после успеха + `builder prune` по порогу диска; ручной `pi gc` |
| HTTP API | версионированный префикс `/v1` + version-handshake |
| Секреты в логах | маскировка значений EnvBundle (≥6 симв.) → `***KEY***` |
| Параллельные билды | глобальный build-семафор (=1) |
| Update агента | drain: 503 новым деплоям, дождаться активных, затем своп бинаря |

## 3. Не-цели v1 (YAGNI)

- Приём GitHub-вебхуков (только архитектурный задел через `Trigger`).
- Git-polling агентом.
- Сборка вне Pi / push готового образа в registry (задел через `Source`).
- Reverse-proxy типа Traefik/Caddy (задел через `Ingress`).
- mTLS / WireGuard транспорт (задел через `Transport`).
- Zero-downtime blue-green swap и авто-откат к last-known-good (v1 — in-place
  своп с health-check-гейтом, без авто-отката).
- Веб-дашборд (axum-сервер заложен, UI — позже).
- Мульти-Pi / кластер (клиентский конфиг хранит профили серверов как задел).

## 4. Сущности (domain)

- `Project` — имя, источник, ingress-настройки, ссылка на env-bundle.
- `Deployment` — один акт деплоя: ref, commit-sha, время, результат, лог-хвост.
- `DeployRef` — ветка или конкретный commit-sha.
- `EnvBundle` — набор key→value секретов проекта (значения непубличны).
- `ServiceStatus` — состояние стека (running/stopped/failed/health).
- `PortBinding` — host-порт, закреплённый за проектом.
- `Hostname` — FQDN для ingress (напр. `rateme.isskelo.com`).
- `DiagnosticReport` — результат самодиагностики (набор проверок PASS/FAIL+подсказка).

## 5. Архитектура: слои и Dependency Rule

Зависимости направлены только внутрь, к домену.

```
pi/
├─ crates/
│  ├─ domain/          # сущности + контракты (трейты). Нет внешних зависимостей
│  ├─ application/     # use-cases; зависят ТОЛЬКО от domain-трейтов
│  ├─ infrastructure/  # адаптеры, реализуют domain-контракты
│  └─ bin/             # бинарь `pi`: presentation (clap + axum) + 2 composition root
└─ Cargo.toml          # workspace
```

- `domain` не зависит ни от кого.
- `application` → `domain`.
- `infrastructure` → `domain` (реализует трейты), внешние инструменты/крейты
  (`docker compose` и `git` CLI через shell-out, rusqlite, age, tracing, и т.д.).
- `bin` → всё (composition root, точки входа).

Единый бинарь `pi` работает в двух режимах: CLI локально и `pi agent` (демон).
Композиция зависимостей различается по режиму, ядро (use-cases) — одно.

## 6. Контракты (трейты) и v1-адаптеры

| Контракт | Назначение | v1-адаптер |
|---|---|---|
| `Source` | получить код по `DeployRef` | `GitSource` |
| `ContainerRuntime` | build / up / down / ps / logs / **stats** (абстракция над бэкендом контейнеров) | `DockerComposeRuntime` |
| `Ingress` | upsert/remove маршрут hostname→port + DNS-запись | `CloudflaredIngress` |
| `SecretStore` | сохранить/выдать `EnvBundle` | `EncryptedFileStore` |
| `ProjectRepository` | реестр проектов, порт-аллокации | `SqliteProjectRepo` |
| `DeploymentHistory` | история деплоев | `SqliteHistory` |
| `StatsProvider` | агрегирует контейнерные метрики (из `ContainerRuntime`) + хостовые | `CompositeStats` (`ContainerRuntime` + `sysinfo`) |
| `SystemProbe` | проверки окружения для `doctor` | `HostSystemProbe` |
| `Trigger` | источник события «deploy X at ref Y» | `HttpApiTrigger` (webhook — позже) |
| `Transport` | канал CLI→агент | `SshTunnelHttp` |
| `Clock`, `IdGen` | детерминизм в тестах | системные impl |

Все use-cases зависят только от этих трейтов → тестируются с моками без Docker.

`ContainerRuntime` — единственная точка знания про контейнерный бэкенд (build,
запуск, логи, **stats**). Это граница замены: docker → podman → systemd-nspawn →
даже fake/filesystem-бэкенд в тестах, не трогая `application`/`domain`.
`StatsProvider` не шеллит docker напрямую — он композирует контейнерные метрики
из `ContainerRuntime` и хостовые из `sysinfo`.

## 7. Use-cases (application)

Каждый — структура с инжектированными `Arc<dyn …>`:

- `DeployProject` — главный (см. §8).
- `SendEnv` — принять `EnvBundle`, сохранить через `SecretStore`.
- `ListProjects` — реестр + текущий статус.
- `GetStats` — метрики по проекту/всем.
- `ControlLifecycle` — start/stop/restart стека.
- `RemoveProject` — снести контейнеры, ingress-маршрут, workdir, записи.
- `StreamLogs` — стрим логов контейнеров.
- `RunDiagnostics` — собрать `DiagnosticReport` через `SystemProbe` (для `pi doctor`).

## 8. Поток `pi deploy`

1. **CLI**: читает `pi.toml`, выбирает сервер (§16/17), открывает SSH-туннель,
   `POST /v1/deployments {project, ref}` → агент сразу отвечает `{deployment_id}`
   (деплой — async-задача). CLI follow-ит логи через `GET /v1/deployments/{id}/logs`
   (SSE/chunked) — не держит один долгий HTTP-запрос на весь build.
2. **Agent** (`DeployProject`):
   1. **per-project lock** (нет параллельных деплоев одного проекта) +
      **глобальная критическая секция** на общие ресурсы (аллокация порта,
      правка ingress-конфига). Если деплой проекта уже идёт — новый запрос
      встаёт в **pending-слот глубины 1**; следующий запрос замещает ожидающий
      (тот помечается `superseded`). Деплоится всегда самый свежий ref —
      CI с двумя пушами подряд работает без ретраев;
   2. `Source.fetch(ref)` → `git fetch` + `reset --hard <sha>` в workdir проекта
      (идемпотентно, переживает force-push/rebase). git ходит с per-project
      deploy-key через `GIT_SSH_COMMAND="ssh -i <project-key>"` (§15.1);
   3. `ProjectRepository` выдаёт/резервирует стабильный host-порт (8000–8999)
      под глобальным локом; unique-constraint на `host_port` в БД — бэкстоп;
   4. `SecretStore` отдаёт env → расшифровка в `.env` рабочей папки (остаётся,
      `0600`); генерация `docker-compose.override.yml` (маппинг
      `127.0.0.1:<host-порт>` → публичный сервис, см. §12.1);
   5. `ContainerRuntime`: `docker compose -f compose -f override build` →
      `up -d --build` (named volumes сохраняются, без `down -v`) →
      **health-check** публичного сервиса — гибрид: docker `healthcheck` (ждём
      `healthy`, если объявлен) → иначе HTTP GET на host-порт (`path`/статус из
      `[healthcheck]`) → иначе TCP-connect; таймаут/ретраи из конфига;
   6. `Ingress.upsert(hostname → host-порт)`: правит cloudflared ingress, заводит
      DNS-запись (`cloudflared tunnel route dns`) и перезапускает cloudflared-юнит
      (без sudo, §11);
   7. `DeploymentHistory.record(ref, sha, time, result)`; после успеха —
      `docker image prune -f` (GC, §8.1);
   8. лог-хвост и статус стримятся в CLI через SSE-эндпоинт (значения
      секретов маскируются, §8.1).
3. **Откат (v1)**: деплой — in-place своп (`up -d --build`), при свопе возможен
   кратковременный downtime (zero-downtime отложен). Health-check — **гейт**: при
   провале деплой помечается `failed`, логи + причина возвращаются в CLI, стек
   остаётся как задеплоен (без авто-сноса). Авто-откат к last-known-good sha —
   §21/v2.

### 8.1 Устойчивость деплоя

- **Краш-рекавери.** Per-project локи, pending-слоты и build-семафор живут
  **только в памяти** (умирают вместе с процессом — нечего чистить). При старте
  агент свипает БД: все деплои в статусе `queued`/`running` помечаются
  `interrupted`. Авто-резюма нет — пользователь перезапускает деплой сам.
- **Поэтапные таймауты.** Свой таймаут на каждый этап — дефолты в `agent.toml`
  (`fetch 2м`, `build 30м`, `up 5м`), override на проект в `[timeouts]`
  `pi.toml`. Просроченный этап убивает дочерний процесс, деплой →
  `failed` с указанием этапа (`timeout: build`).
- **Отмена.** `pi deploy --cancel` (и вопрос «отменить?» при Ctrl+C во время
  follow) → `DELETE /v1/deployments/{id}` — агент убивает текущий этап,
  деплой → `canceled`.
- **Build-семафор.** Фаза build проходит через глобальный семафор (размер 1,
  настраивается в `agent.toml`): параллельные деплои разных проектов не
  устраивают OOM на Pi; остальные этапы идут параллельно.
- **GC диска.** После успешного деплоя — `docker image prune -f` (только
  dangling; build cache живёт, билды остаются быстрыми). Если диск заполнен
  выше порога (`agent.toml`, дефолт 85%) — дополнительно `docker builder
  prune` с лимитом возраста. Ручная команда `pi gc` — то же по запросу.
- **Маскировка секретов.** Агент знает значения `EnvBundle` проекта — во всех
  потоках (deploy-стрим, `pi logs`, `log_tail` в БД) значения длиной ≥6
  символов заменяются на `***KEY***`. Порог отсекает ложные срабатывания на
  `true`/`3000`.

## 9. Транспорт и безопасность

- Агент `pi-agent` слушает HTTP на **unix-сокете** `/run/pi/agent.sock`
  (владелец `pi-agent`, группа `pi-agent`, `0660`) — наружу не торчит, доступ
  контролируется правами ОС: другой юзер на Pi или контейнер с
  `network_mode: host` до API не дотянется. TCP `127.0.0.1:<port>` — опция в
  `agent.toml` (не дефолт).
- CLI поднимает SSH-туннель с форвардом на сокет
  (`ssh -L <local-port>:/run/pi/agent.sock`) и шлёт HTTP внутрь него.
- Аутентификация — SSH-ключами; в CI/CD раннер кладёт deploy-ключ. SSH-туннель
  ходит под **обычным логин-юзером** Pi (не под сервис-юзером `pi-agent`, который
  `nologin`); для доступа к сокету логин-юзер добавляется в группу `pi-agent`
  при `pi agent setup`.
- `Transport`-трейт абстрагирует канал → позже можно mTLS/WireGuard.
- Webhook-роут (будущее) поднимается на том же axum-сервере и публикуется
  отдельно через Cloudflare, не затрагивая control-плоскость.

### 9.1 HTTP API агента (v1)

Все роуты под префиксом `/v1`; ломающие изменения — только сменой префикса.

| Метод и путь | Назначение |
|---|---|
| `GET /v1/version` | версия агента + API (handshake) |
| `POST /v1/deployments` | запустить деплой `{project, ref}` → `{deployment_id}` |
| `GET /v1/deployments/{id}` | статус деплоя |
| `GET /v1/deployments/{id}/logs` | SSE-стрим логов деплоя |
| `DELETE /v1/deployments/{id}` | отменить деплой (`canceled`) |
| `GET /v1/projects` | список проектов + статусы (`pi ls`) |
| `DELETE /v1/projects/{name}` | снести проект (`pi rm`) |
| `GET /v1/projects/{name}/stats` | метрики проекта; `GET /v1/stats` — все |
| `PUT /v1/projects/{name}/env` | принять EnvBundle (`pi env send`) |
| `GET /v1/projects/{name}/env` | имена ключей, без значений (`pi env ls`) |
| `POST /v1/projects/{name}/lifecycle` | start / stop / restart |
| `GET /v1/projects/{name}/logs` | SSE-стрим логов контейнеров |
| `GET /v1/status` | обзор агента и хоста |
| `GET /v1/doctor` | `DiagnosticReport` |

**Version-handshake:** при коннекте CLI дёргает `GET /v1/version`. Расхождение
версий бинарей CLI/агента — warning + подсказка `pi agent update`; отсутствие
`/v1` (404) — чёткая ошибка о несовместимости вместо загадочных ошибок
десериализации.

## 10. Секреты (`pi env send`)

- Транзит — внутри SSH (уже шифрованный).
- На Pi: `EncryptedFileStore` шифрует bundle ключом, сгенерированным агентом при
  первом старте (хранится `0600` в data-dir). Зашифрованные бандлы лежат отдельно
  в `/var/lib/pi/secrets/<project>.env.age`. В открытом виде там их нет.
- При деплое env расшифровывается в `.env` в рабочей папке compose-проекта
  (workdir клона, `0600`) — compose авто-подхватывает его для `${VAR}`-подстановки
  и `env_file`. Файл **остаётся** в workdir: lifecycle-команды (`start/stop/restart`)
  и любой повторный `docker compose` ре-инжектят/обновляют `.env` из `SecretStore`
  при каждой операции, т.к. `${VAR}` резолвится при каждом запуске compose. См. §12.1.
- `pi env ls` показывает только имена ключей, не значения.
- **Применение к работающему стеку:** работающие контейнеры не видят новый env
  без пересоздания. `pi env send` по умолчанию только сохраняет bundle и
  печатает: «saved; применится при следующем deploy/restart». Флаг `--apply`
  сразу делает `up -d` с новым `.env` — пересоздаются только затронутые сервисы.
- Значения EnvBundle маскируются во всех логах и стримах (§8.1).

## 11. Ingress (Cloudflare Tunnel)

- **Режим:** cloudflared — **locally-managed**, конфиг (`config.yml`) в data-dir
  агента, владелец `pi-agent`. Это отдельный systemd-юнит `User=pi-agent`,
  `Restart=on-failure` — он живёт независимо от агента, поэтому апдейт/краш/рестарт
  `pi-agent` **не рвёт** публичный доступ ко всем сайтам.
- `CloudflaredIngress` при деплое: (1) правит ingress-правило
  (`hostname → http://127.0.0.1:<host-порт>`); (2) заводит DNS-запись через
  `cloudflared tunnel route dns <tunnel> <hostname>`; (3) применяет конфиг
  перезапуском cloudflared-юнита **без sudo** — через systemd-linger +
  `systemctl --user restart` (точечное правило sudoers — фолбэк).
- **Бутстрап** (один раз при `pi agent setup`): `cloudflared tunnel login` →
  `cert.pem` в data-dir (нужен для `route dns`); создаётся/выбирается tunnel.
- **Рестарт — только при реальном изменении.** `CloudflaredIngress` сравнивает
  желаемый ingress с текущим `config.yml` и перезапускает cloudflared только при
  diff. Host-порт проекта стабилен → обычный редеплой ingress не меняет и
  cloudflared не трогается. Короткий разрыв всех туннелей остаётся только при
  добавлении/удалении проекта или смене hostname — **known limitation v1**.
- **`pi rm` и DNS.** Ingress-правило удаляется, но `cloudflared tunnel route dns`
  не умеет удалять DNS-записи — запись остаётся (ведёт в туннель, отвечающий 404).
  CLI печатает точную инструкцию удалить её в Cloudflare-дашборде; авто-удаление
  через Cloudflare API token — точка расширения (§21).
- Hostname и публичный сервис берутся из `pi.toml` (`[ingress].hostname/service/port`);
  host-порт выделяет `pi` и пробрасывает на контейнерный порт через override (§12.1).
- Контракт `Ingress` позволяет заменить на Traefik/Caddy/manual-ports позже.

## 12. Схема `pi.toml` (конфиг проекта, лежит в репе)

```toml
schema = 1                        # версия формата pi.toml (под будущие миграции)

[project]
name = "rateme"

[source]
repo = "git@github.com:isskelo/rateme.git"
branch = "main"

[build]
compose = "docker-compose.yml"   # compose проекта — source of truth (app + БД + …)

[ingress]
hostname = "rateme.isskelo.com"  # FQDN → CloudflaredIngress (ingress + DNS)
service = "web"                   # какой сервис из compose публичный
port = 3000                       # контейнерный порт этого сервиса
# host-порт pi выделяет сам (8000–8999) и инжектит через override

[healthcheck]                     # гейт деплоя (§8.2.5); все поля опциональны
path = "/"                        # путь HTTP-пробы, если нет docker healthcheck
# expect = "2xx"                  # ожидаемый статус (дефолт 2xx/3xx)
# timeout = "60s"                 # суммарный бюджет; interval/retries — дефолты

# [timeouts]                      # override поэтапных таймаутов агента (§8.1)
# fetch = "2m"
# build = "30m"
# up = "5m"

[env]
file = ".env"                     # что шлёт `pi env send`
```

### 12.1 Работа с compose проекта

- **Source of truth — compose проекта.** В нём могут быть app + базы + redis +
  любые сервисы. Агент делает `docker compose build` + `up -d` именно по нему;
  `image:`-сервисы пуллятся, `build:`-сервисы собираются на Pi. Все в одной
  compose-сети, видят друг друга по именам.
- **Публичный сервис — один.** `[ingress].service` + `port` указывают, какой
  сервис и контейнерный порт публичны. Остальные сервисы (БД и т.п.) наружу не
  выставляются.
- **Инжект порта — через отдельный override.** `pi` генерит override в data-dir
  (`/var/lib/pi/overrides/<project>.yml` — вне workdir: не конфликтует с git и
  файлами проекта), который маппит выделенный host-порт (8000–8999) →
  контейнерный порт публичного сервиса. Биндинг — на `127.0.0.1:<host-порт>`
  (не `0.0.0.0`), чтобы порт не торчал в LAN; cloudflared ходит на этот
  loopback. Проектный compose не правится.
- **Цепочка `-f` чтит override проекта.** Явные `-f` отключают авто-подхват
  `docker-compose.override.yml` — поэтому, если он есть в репе, агент включает
  его в цепочку: `docker compose -f compose.yml [-f docker-compose.override.yml]
  -f /var/lib/pi/overrides/<project>.yml`. Файл pi — последний, его маппинг
  порта побеждает.
- **Сохранность данных (жёсткое правило).** Деплой = `up -d --build`: пересоздаёт
  изменённые сервисы, **named volumes сохраняются**. `pi` **никогда** не делает
  `down -v`. `pi rm` удаляет volumes только с явным подтверждением. Данные БД
  переживают редеплои, ребилды и откаты.
- **Инжект секретов.** Расшифрованный `.env` кладётся в рабочую папку
  compose-проекта (см. §10) → compose авто-подхватывает его для `${VAR}`-подстановки;
  сервисы потребляют переменные через `env_file`/`environment`.

## 13. Установка и self-setup агента

Ожидание: «поставил из репозитория → само настроилось → подняло фоновый процесс».

- **Установка бинаря:** one-line `curl -fsSL .../install.sh | sh` — определяет
  архитектуру (aarch64/armv7), качает релизный бинарь в `/usr/local/bin/pi`,
  **сверяет sha256** с опубликованным в релизе.
  Альтернатива для разработчиков: `cargo install` / сборка из исходников.
- **`pi agent setup`** (один раз, `sudo`), **идемпотентно**:
  1. создаёт системного юзера `pi-agent` (nologin, без home-shell);
  2. создаёт каталоги (см. §17): data-dir, log-dir, config-dir;
  3. генерит ключ шифрования секретов (`0600`, владелец `pi-agent`);
  4. добавляет юзера `pi-agent` в группу `docker`; добавляет **логин-юзера**
     (через которого ходит SSH) в группу `pi-agent` — доступ к unix-сокету API
     (§9); включает systemd-linger для `pi-agent` (чтобы агент мог
     `systemctl --user restart cloudflared` без sudo);
  5. проверяет наличие `docker`, `docker compose`, `cloudflared` — если чего-то
     нет, печатает чёткое предупреждение и инструкцию (не падает молча);
  6. бутстрапит cloudflared: `cloudflared tunnel login` → `cert.pem` в data-dir,
     создаёт/выбирает tunnel, пишет его `config.yml` и user-юнит cloudflared
     (`User=pi-agent`, `Restart=on-failure`);
  7. пишет unit `pi-agent.service`, `systemctl enable --now pi-agent`.
- **Deploy-key Pi→GitHub:** генерится **на проект** (лениво при первом деплое
  репо), public-ключ показывается для добавления в Deploy keys **этого** репо
  (детали — §15.1).
- **Самоинициализация:** при старте сервиса агент досоздаёт отсутствующие
  каталоги/БД (на случай ручного запуска или частичной установки).
- **Повторный `setup`** чинит/обновляет конфигурацию, ничего не ломая.
- Доп. команды: `pi agent status`, `pi agent update`, `pi agent uninstall`
  (снять юнит/юзера/каталоги — с подтверждением).
- **`pi agent update` — с drain:** агент переводится в draining (новые деплои
  получают 503 + подсказку повторить позже), активные деплои дожидаются
  завершения (таймаут drain — в `agent.toml`); затем бинарь перекачивается
  (sha256-проверка) и сервис рестартует. Если drain не уложился в таймаут —
  деплой прерывается, свип при старте пометит его `interrupted` (§8.1).

## 14. Логи и диагностика самой тулы

Цель: при ошибке быстро понять причину и починить.

- Логирование — `tracing` + `tracing-subscriber`.
  - **Агент:** два слоя — stderr → journald (ротация системой) **и** rolling-файл
    (`tracing-appender`, `/var/log/pi/`, ротация по дням, ретеншен N дней).
  - **Клиент:** human-читаемые ошибки в stderr; при `-v/--verbose` — debug в
    rolling-файл `~/.local/state/pi/logs/`.
- Формат: pretty по умолчанию, `--json` для машинного парсинга; уровни через
  `RUST_LOG` и `-v`.
- **`pi agent logs [-f] [--since <when>]`** — обёртка над journald + файлом.
- **`pi doctor`** (`RunDiagnostics` + `SystemProbe`) — самодиагностика, каждый чек
  с PASS/FAIL и подсказкой как чинить: docker daemon доступен; `docker compose`
  есть; `cloudflared` установлен и туннель жив; права/группы (`pi-agent` в
  `docker`, linger включён, `cert.pem` на месте); свободное место на диске; связь
  клиент→агент (с клиента — через SSH-туннель).
- Паники/критические ошибки агента логируются с контекстом перед выходом;
  systemd рестартит сервис (`Restart=on-failure`).

## 15. Интерактивная настройка клиента (`inquire`)

Два независимых интерактивных потока + неинтерактивный режим для CI.

- **`pi setup`** (один раз на машину): мастер спрашивает host/IP Pi, SSH-user,
  путь к ключу (детектит файлы в `~/.ssh/`), имя/алиас сервера. **SSH-ключи
  клиент→Pi бутстрапятся автоматически** (см. §15.1). В конце — тест связи:
  SSH-коннект + ping агента + удалённый `pi doctor`. Пишет
  `~/.config/pi/config.toml`. Поддержка нескольких серверов-профилей (задел;
  есть дефолтный профиль).

### 15.1 Авто-бутстрап SSH-ключей

Две независимые связки.

**Клиент → Pi** (для туннеля), полный авто-бутстрап в `pi setup`. Ключ кладётся в
`authorized_keys` обычного **логин-юзера** Pi (не `pi-agent`):
1. детект клиентского ключа в `~/.ssh/` (ed25519/rsa);
2. если ключа нет — `ssh-keygen -t ed25519` (с подтверждением);
3. предложить залить pubkey на Pi через `ssh-copy-id`-стиль: один раз спросить
   пароль Pi, дописать в `authorized_keys`;
4. проверить, что вход по ключу работает.
- **Fallback:** если на Pi пароль-аутентификация выключена и ключа ещё нет —
  показать pubkey и инструкцию занести его вне программы (Raspberry Pi Imager
  при прошивке / физический доступ). `pi doctor` диагностирует это состояние.

**Pi → GitHub** (deploy-key для `GitSource`) — **по ключу на проект** (GitHub не
даёт переиспользовать один deploy-key между репозиториями):
- при первом деплое приватного репо агент лениво генерит keypair в
  `/var/lib/pi/keys/<project>/` (`0600`, владелец `pi-agent`), показывает **public**
  ключ и инструкцию добавить его в GitHub → Deploy keys **этого** репо (read-only);
- git использует нужный ключ через `GIT_SSH_COMMAND="ssh -i <project-key>"` —
  без правки глобального `~/.ssh/config`;
- авто-регистрация ключа через `gh`/GitHub API — точка расширения (§21).
- **`pi init`** (на проект): мастер с **авто-детектом** пред-заполненных дефолтов —
  `name` (имя папки), `repo` (git remote origin), `branch` (текущая), `compose`
  (найденный compose-файл), `hostname`, внутренний `port`, env-файл. Валидация
  ввода. Генерит `pi.toml`. В конце предлагает выполнить сразу `pi env send` и
  `pi deploy`.
- **Неинтерактивный режим:** любой промпт задаётся флагом
  (`pi init --name … --repo … --hostname …`, `pi setup --host … --user …`),
  `--yes` принимает детект-дефолты — для CI/скриптов.

## 16. Команды CLI

| Команда | Действие |
|---|---|
| `pi setup` | интерактивная настройка подключения к серверу (клиентский конфиг) |
| `pi init` | интерактивная генерация `pi.toml` в текущем проекте |
| `pi deploy [--ref <branch/sha>] [--server <alias>]` | деплой текущего проекта (сервер: default-профиль / `--server` / `PI_SERVER`; CI — `--host/--user/--key`) |
| `pi deploy --cancel` | отменить идущий деплой текущего проекта |
| `pi env send [--file .env] [--apply]` | защищённо отправить env; `--apply` — сразу применить (`up -d`) |
| `pi env ls` | список ключей env (без значений) |
| `pi ls` / `pi ps` | список проектов + статус |
| `pi logs <project> [-f]` | логи контейнеров проекта |
| `pi stats [project]` | метрики (CPU/mem/диск, uptime, last deploy) |
| `pi start\|stop\|restart <project>` | управление жизненным циклом |
| `pi rm <project>` | снести проект (контейнеры + ingress + workdir) |
| `pi status` | обзор агента и хоста |
| `pi gc` | почистить docker-образы и build cache (§8.1) |
| `pi doctor` | самодиагностика окружения (PASS/FAIL + подсказки) |
| `pi agent setup` | развернуть/донастроить агент на Pi (systemd, идемпотентно) |
| `pi agent status\|update\|uninstall` | управление агентом |
| `pi agent logs [-f] [--since]` | логи самого агента (journald + файл) |
| `pi help` | clap генерит список всех команд автоматически |

## 17. Каталоги, конфиги, права

**На Pi (агент, владелец `pi-agent`):**

- `/var/lib/pi/` — data-dir: `state.db` (SQLite), `secret.key` (`0600`),
  `secrets/<project>.env.age` (зашифрованные env-bundle),
  `keys/<project>/` (per-project deploy-ключи Pi→GitHub, `0600`),
  `cloudflared/` (`config.yml`, `cert.pem`, tunnel-credentials),
  `overrides/<project>.yml` (сгенерированные compose-override, §12.1),
  `workdirs/<project>/` (клоны репо + расшифрованный `.env`, `0600`).
- `/run/pi/agent.sock` — unix-сокет API (`0660`, группа `pi-agent`; создаётся
  при старте агента, §9).
- `/var/log/pi/` — rolling-логи агента.
- `/etc/pi/agent.toml` — конфиг агента (путь сокета / опц. TCP-порт на localhost,
  диапазон host-портов, поэтапные таймауты, порог диска для GC, размер
  build-семафора, ретеншен логов и БД, путь к cloudflared-конфигу).
- `/etc/systemd/system/pi-agent.service` — unit агента.
- cloudflared — user-юнит `pi-agent` (`~/.config/systemd/user/` под linger) или
  отдельный system-юнит `User=pi-agent` — перезапускается без sudo.

**Бэкап (что сохранять при отказе SD-карты):** data-dir `/var/lib/pi/` **без**
`workdirs/` (клоны восстанавливаются из git) + `/etc/pi/agent.toml`. Этого
достаточно для полного восстановления: состояние, секреты, deploy-ключи,
туннель. Команды `pi agent backup`/`restore` — точка расширения (§21).

**На клиенте:**

- `~/.config/pi/config.toml` — серверы-профили:

```toml
default = "home"

[servers.home]
host = "192.168.1.50"      # или публичный адрес
user = "pi"                # ЛОГИН-юзер Pi для SSH (НЕ сервис-юзер pi-agent)
key = "~/.ssh/id_ed25519"
# агент слушает unix-сокет /run/pi/agent.sock; туннель форвардит на него (§9)
```

- `~/.local/state/pi/logs/` — debug-логи клиента (при `-v`).

## 18. State (SQLite, в data-dir агента)

Таблицы (черновик):

- `projects` — id, name, repo, branch, compose_path, hostname, internal_port,
  host_port, created_at.
- `deployments` — id, project_id, ref, commit_sha, started_at, finished_at,
  status (`queued|running|success|failed|canceled|interrupted|superseded`),
  log_tail (с маскировкой секретов, §8.1).
- `port_allocations` — host_port, project_id (уникальность порта).
- `stats_snapshots` — project_id, ts, cpu, mem, … (опц., для истории метрик).

Режим и обслуживание:

- БД открывается в WAL-режиме (`PRAGMA journal_mode=WAL`).
- Ретеншен: последние 50 деплоев на проект (настраиваемо в `agent.toml`),
  `stats_snapshots` — N дней; чистка после каждой вставки.
- При старте агента — свип: `queued`/`running` → `interrupted` (§8.1).

## 19. Обработка ошибок

- `thiserror` — типизированные error-enum'ы на слой.
- Инфра-ошибки мапятся в доменные на границе слоёв.
- `anyhow` допустим только в composition root / точках входа.
- Ошибки — значения: всё через `Result<T, E>` (errors-as-values идиоматичен для
  Rust), без паник в use-cases/адаптерах.

## 20. Тестирование

- `domain` / `application` — юнит-тесты на мок-контрактах (без Docker), детерминизм
  через `Clock`/`IdGen`.
- `infrastructure` — интеграционные тесты на реальных docker/git/cloudflared.
- deploy-flow — e2e опционально (отдельный Pi или docker-in-docker).
- `pi doctor` / `setup` — тестируются через мок `SystemProbe`.

## 21. Точки расширения (заложить, не реализовывать в v1)

- `Source`: добавить registry/готовый образ.
- `Ingress`: Traefik/Caddy/manual-ports.
- `Trigger`: GitHub webhook-receiver (роут на axum), git-polling.
- `Transport`: mTLS/WireGuard.
- Авто-откат к last-known-good sha + zero-downtime blue-green swap (health-check
  гейт уже есть в v1).
- Авто-регистрация deploy-key в GitHub через `gh`/GitHub API.
- Авто-удаление DNS-записей при `pi rm` через Cloudflare API token (§11).
- `pi agent backup`/`restore` — архив data-dir без workdirs + `VACUUM INTO`
  для SQLite (§17).
- Несколько серверов-профилей в клиентском конфиге → мульти-Pi.
- Веб-дашборд поверх существующего axum-сервера.

## 22. Дефолты, требующие подтверждения при реализации

- Диапазон портов 8000–8999 (настраиваемый в `agent.toml`).
- Шифрование env на диске (`age`/аналог) — ключ агента.
- Параметры health-check при деплое (стратегия — гибрид docker→HTTP→TCP, §8.2.5;
  `path`/`expect`/`timeout`/`interval`/`retries` в `[healthcheck]` `pi.toml`).
- Ретеншен rolling-логов (дней).
- Поэтапные таймауты деплоя (fetch 2м / build 30м / up 5м) и таймаут drain
  при `pi agent update`.
- Порог заполнения диска для агрессивного `builder prune` (85%).
- Ретеншен БД: деплоев на проект (50), stats_snapshots (дней).
- Порог длины маскируемых значений секретов (6 символов).
- Хостинг релизных бинарей для install-скрипта и `pi agent update`
  (+ публикация sha256-чексумм).

## 23. Роадмап версий (объём до 1.0)

Каждая версия — одна тема с проверяемым критерием готовности; берётся в работу
одним планом реализации. Ссылки указывают на секции спеки.

### v0.1 — MVP: деплой-ядро

**Критерий:** `pi deploy` проходит end-to-end на целевом Pi.

- Каркас workspace + слои + контракты; `domain`-сущности.
- `application`: `DeployProject` + юнит-тесты на моках (`Clock`/`IdGen`).
- `infrastructure`: `GitSource` (per-project deploy-key, §15.1),
  `DockerComposeRuntime`, `SqliteProjectRepo` + `SqliteHistory` (WAL, §18).
- Агент: axum на unix-сокете (§9); `/v1`: `version`, `deployments`
  (POST / GET / SSE-логи); порт-аллокация; генерация override (§12.1).
- CLI: `pi deploy` через SSH-туннель, `pi ls`.
- Вручную (одноразово): установка агента (сборка + systemd-юнит руками),
  cloudflared-конфиг (host-порт стабилен — правится один раз), `.env`
  кладётся в workdir руками.
- Логи: `tracing` → stderr/journald (минимум).

### v0.2 — Секреты + ingress

**Критерий:** добавление нового проекта не требует ручной настройки на Pi.

- `EncryptedFileStore` (age), `pi env send/ls` (+`--apply`, §10),
  маскировка секретов в логах (§8.1).
- `CloudflaredIngress`: upsert + `tunnel route dns` + diff-restart (§11).
- Health-check-гейт деплоя (docker → HTTP → TCP, §8).

### v0.3 — Устойчивость: CI-ready

**Критерий:** GitHub Actions деплоит без присмотра.

- Очередь latest-wins, поэтапные таймауты, `pi deploy --cancel`,
  свип при старте (§8.1).
- Build-семафор, GC диска + `pi gc`, ретеншен БД (§8.1, §18).
- Неинтерактивные флаги (`--host/--user/--key`), version-handshake warning
  (§9.1), пример GitHub Actions workflow в доках.

### v0.4 — Операционка

**Критерий:** проблема диагностируется штатными командами, без ssh-археологии.

- `pi doctor` (`RunDiagnostics` + `HostSystemProbe`), `pi agent logs/status`,
  rolling-файлы логов (§14).
- `pi stats` (`CompositeStats` + `sysinfo`), `start/stop/restart`,
  `pi rm` (+DNS-инструкция, §11), `pi status`.

### v0.5 — Установка и UX

**Критерий:** человек со стороны ставит тулу с нуля за ~10 минут.

- `install.sh` + sha256-чексуммы, публикация релизных бинарей.
- `pi agent setup` (идемпотентный полный бутстрап, §13),
  `pi agent update` (drain, §13) / `uninstall`.
- `pi setup` + `pi init` (inquire-мастера + авто-бутстрап SSH-ключей, §15).

### v1.0 — Стабилизация

- E2e деплой-тест; интеграционные тесты инфраструктуры (§20).
- Доки: README, бэкап-гайд (§17), пример CI.
- Полировка ошибок и сообщений; заморозка `/v1` API.

### Пост-1.0 (будущие обновления, по ожидаемой ценности)

1. GitHub webhook-receiver — деплой по пушу без CI-шага (`Trigger`).
2. Авто-откат к last-known-good + blue-green zero-downtime swap.
3. `pi agent backup`/`restore` (§17).
4. Авто-регистрация deploy-key через `gh`/GitHub API; авто-удаление
   DNS-записей через Cloudflare API (§11).
5. Registry-source: готовые образы, сборка вне Pi (`Source`).
6. Мульти-Pi: несколько серверов-профилей.
7. Веб-дашборд поверх существующего axum-сервера.
8. `Ingress`-альтернативы (Traefik/Caddy), mTLS/WireGuard-транспорт,
   git-polling.

Порядок пост-1.0 фиксирует приоритет, а не обязательство; пункты соответствуют
точкам расширения §21.

## 24. Тех-стек (итог)

**Язык/сборка:** Rust stable, Cargo workspace; кросс-сборка под `aarch64`/`armv7`.

**Общий стек:**

| Слой | Крейт |
|---|---|
| Async runtime | `tokio` |
| HTTP-сервер (агент) | `axum` + `tower` |
| HTTP-клиент (CLI→агент) | `reqwest` |
| CLI-парсинг | `clap` (derive) |
| Интерактив | `inquire` |
| Конфиг | `serde` + `toml` |
| Логи | `tracing` + `tracing-subscriber` + `tracing-appender` (+ journald-слой) |
| Ошибки | `thiserror` (на слоях) + `anyhow`/`color-eyre` в точках входа |
| Шифрование секретов | `age` |
| Метрики хоста | `sysinfo` |
| Моки в тестах | `mockall` |
| DTO/сериализация | `serde` / `serde_json` |

**Адаптеры контрактов (философия «обернуть проверенные CLI за контракты»):**

| Контракт | Адаптер | Реализация |
|---|---|---|
| `Source` | `GitSource` | `git` CLI (shell-out через `tokio::process`) |
| `ContainerRuntime` | `DockerComposeRuntime` | `docker compose` CLI (build/up/down/ps/logs) + `docker stats --format json` (метод `stats`) |
| `StatsProvider` | `CompositeStats` | контейнерные метрики из `ContainerRuntime` + хостовые из `sysinfo` (docker напрямую не шеллит) |
| `SecretStore` | `EncryptedFileStore` | `age` + файлы (`0600`) |
| `ProjectRepository` | `SqliteProjectRepo` | `rusqlite` (sync + `spawn_blocking`) |
| `DeploymentHistory` | `SqliteHistory` | `rusqlite` |
| `Ingress` | `CloudflaredIngress` | правка cloudflared ingress + `tunnel route dns` + restart юнита (без sudo) |
| `Transport` | `SshTunnelHttp` | системный `ssh -L <port>:/run/pi/agent.sock` + `reqwest` |
| `SystemProbe` | `HostSystemProbe` | проверки docker/cloudflared/прав/диска |
| `Clock` / `IdGen` | системные impl | `std::time` / `uuid` |

**Обоснование shell-out (git/docker/ssh):** инструменты уже есть на Pi,
переиспользуют системную конфигурацию и ключи, легче собираются на ARM, надёжнее
по auth. Контракты скрывают реализацию — заменить на нативные крейты (`git2`,
`bollard`, `russh`, `sqlx`) можно точечно, не трогая `application`/`domain`.
