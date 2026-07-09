# rpi — авто-настройка Cloudflare Tunnel и LAN-HTTPS одной тулой (дизайн)

Дата: 2026-07-09. База: `2026-06-09-pi-deploy-tool-design.md` (§9 setup, §11 ingress,
§12 rpi.toml), `2026-06-17-pi-lan-expose-design.md` (`expose`, `HostNetwork`),
`2026-07-07-prebuilt-binaries-design.md` (arch-детект/скачивание бинарей).

**Критерий готовности.** На чистой Pi одна команда `sudo rpi agent setup` с
Cloudflare API-токеном и доменом настраивает end-to-end, идемпотентно и без единого
ручного шага:

1. **Внешний доступ** — сервис с `[ingress] hostname = "app.example.com"` доступен
   из интернета по `https://app.example.com` через Cloudflare Tunnel (TLS на edge).
2. **LAN-доступ** — сервис с `[ingress] lan_hostname = "app.lan.example.com"`
   доступен из локальной сети по `https://app.lan.example.com` с настоящим
   доверенным wildcard-сертом (Let's Encrypt DNS-01), TLS терминирует Caddy на Pi.

Ни один из 6 ручных подводных камней из runbook'а (см. §2) не воспроизводится.
Проект без ingress ведёт себя ровно как раньше.

---

## 1. Скоуп

**Входит:**

- Полный авто-bootstrap Cloudflare Tunnel: установка бинаря, создание туннеля через
  **Cloudflare API** (без `cloudflared tunnel login`, без `cert.pem`), запись
  credentials-JSON, безопасная запись + валидация `config.yml`, каноничный фикс
  systemd `--user` через drop-in, enable/linger.
- Замена деплой-тайм `cloudflared tunnel route dns` (шелл, требует `cert.pem`) на
  прямой Cloudflare API-вызов.
- LAN-HTTPS через **Caddy** как host reverse-proxy на 80/443: установка кастомного
  бинаря (`caddy-dns/cloudflare`), базовый конфиг с глобальной wildcard-TLS
  автоматизацией (DNS-01/Cloudflare), admin API на `127.0.0.1:2019`, system-юнит.
- Публичная Cloudflare DNS-запись `*.<label>.<zone>` A → LAN-IP Pi (DNS-only).
- Деплой-тайм LAN-ingress: `CaddyIngress` добавляет/убирает роут через admin API.
- Единый Cloudflare API-токен как **единственный секрет** для туннеля, DNS и ACME.
- Новые поверхности: секции `[cloudflare]`/`[lan]` в `agent.toml`; поле
  `lan_hostname`/`lan` в `[ingress]` `rpi.toml`; флаги `rpi agent setup`.
- Версия `schema` в `agent.toml` (как в `rpi.toml`) + унифицированный фреймворк
  системных миграций `rpi agent migrate` (детект + ledger применённого): существующая
  `pi-to-rpi` рефакторится в него, `nginx-to-caddy` (staged/reversible) — новая запись.
- Migration-гайд `docs/migration-nginx-to-caddy.md` и подсказка оператору при
  обнаружении до-тульного сетапа (nginx на 80/443 / ручной cloudflared / certbot-серт).

**НЕ входит (YAGNI):**

- Управление роутером/локальным DNS-резолвером. LAN-резолв обеспечивается публичной
  Cloudflare-записью с приватным IP (решение пользователя об «утечке» внутреннего IP
  в публичный DNS — принято, вариант A).
- Origin-TLS (cloudflared → сервис по HTTPS). Между edge/Caddy и контейнером —
  простой HTTP на loopback, как сейчас.
- Remotely-managed туннели (Zero Trust dashboard). Только locally-managed
  (`config.yml`), как у пользователя сейчас.
- Авто-трансляция произвольных ручных nginx-конфигов при миграции (кастомные
  хедеры/сайты вне `rpi` — только перечисляем в отчёте, не переносим).
- Zero-downtime cutover (SO_REUSEPORT). Допускаем sub-second окно.
- Управление фаерволом (как и в LAN-expose — ответственность роутера).

---

## 2. Подводные камни runbook'а и как дизайн их закрывает

| # | Симптом (runbook) | Решение в дизайне |
|---|---|---|
| 1 | Плейсхолдер `pi-user` vs реальный `piuser` → `usermod` падает | login-user резолвится из `--user`/`$SUDO_USER`, валидируется `id -u`; плейсхолдеров нет |
| 2 | `cloudflared: command not found` — `--with-cloudflared` только скаффолдит юнит | bootstrap ставит `cloudflared` и `caddy` по arch → `/usr/local/bin`, идемпотентно |
| 3 | `tunnel login` спотыкается о HOME/cwd (`/home/pi-agent`, priv key в cwd) | **логина нет**: туннель создаётся Cloudflare API, credentials-JSON пишем сами |
| 4 | Раскладка `cert.pem` по каталогам rpi-agent | **`cert.pem` не нужен**: рантайму — только creds-JSON; `route dns` теперь через API |
| 5 | `config.yml` не создался (опечатка в редакторе) | safe-write (не через редактор) + `cloudflared tunnel ingress validate` |
| 6 | Табы в YAML → cloudflared не парсит | тот же safe-write только пробелами + валидация |
| 7 | `systemctl --user`: юнит не найден (passwd `home=/home/pi-agent`) | **drop-in** `user@<uid>.service.d/override.conf` с `XDG_CONFIG_HOME=/var/lib/rpi/.config` — **не** `usermod -d` |

**Почему drop-in, а не `usermod -d`** (ключевое ограничение из runbook'а): под uid 999
работают одновременно `rpi-agent.service` и контейнеры, запущенные под тем же uid
(напр. valkey). `usermod` отказывается менять пользователя с живыми процессами, а
уронить прод-valkey недопустимо. Drop-in `XDG_CONFIG_HOME` не перетирается PAM (в
отличие от `HOME`), задаёт базу поиска юнитов (`$XDG_CONFIG_HOME/systemd/user`),
`restart user@999` задевает только безобидные user-демоны, обратим и переживает ребут.

---

## 3. Архитектура: два контекста исполнения

Всё делится на два момента с разными правами — это определяет всю форму.

**Bootstrap** — `rpi agent setup`, **root** через sudo, **один раз**. Готовит хост:
ставит бинари, создаёт туннель, пишет system-юниты и drop-in, поднимает Caddy,
прогревает wildcard-серт, заводит DNS.

**Runtime агента** — `rpi-agent.service`, **uid 999, непривилегированный**, на
**каждом деплое**. Переконфигурирует ingress под проект **только через API**
(Cloudflare API / Caddy admin API). Никогда не пишет в `/etc/*` и не рестартит
ничего через sudo.

Именно второй контекст диктует выбор Caddy admin API и Cloudflare API вместо
шелла с sudo.

### Единая абстракция

Расширяем существующий трейт `Ingress` (`upsert`/`remove`, `infrastructure/cloudflared.rs`):

- **`Cloudflare` API-клиент** (`infrastructure/cloudflare.rs`, новый) — единственное
  место, где живёт токен. Умеет: `find_or_create_tunnel`, `put_dns` (CNAME на
  туннель; A на `*.<label>`). Используется bootstrap'ом и агентом. Он же заменяет
  шелл `cloudflared tunnel route dns` в `CloudflaredIngress`.
- **Два ingress-бэкенда за тем же трейтом**, компонуются в `CompositeIngress`:
  - `CloudflaredIngress` (публичный, есть): upsert `config.yml` + CNAME через API +
    restart cloudflared user-юнита. DNS теперь через API-клиент, а не шелл.
  - `CaddyIngress` (LAN, новый): PUT-роут в admin API (`<lan-host>` →
    `http://127.0.0.1:<host-port>`); TLS Caddy подтягивает сам под wildcard'ом. Ни
    DNS, ни reload, ни sudo на деплое.
- Проект в `rpi.toml` опт-инит любой бэкенд независимо (у board — оба). Сборка
  `CompositeIngress` из включённых секций `agent.toml` — в месте, где сейчас
  выбирается `CloudflaredIngress`/`DisabledIngress`.

Мелкие single-purpose юниты — тестируются по образцу `Sys`/`FakeSys`.

---

## 4. Поверхности

### 4.1. `agent.toml` (хост/аккаунт-уровень)

```toml
schema = 1                                       # НОВОЕ: версия формата agent.toml (как в rpi.toml)

[cloudflare]                                     # новое, фундамент Phase 1
zone = "example.com"
token_file = "/var/lib/rpi/cloudflare/token"     # секрет по ссылке, не инлайном
# account_id — если не задан, узнаётся из токена через API один раз и кэшируется

[cloudflared]                                    # есть; теперь управляется тулой целиком
config = "/var/lib/rpi/cloudflared/config.yml"
tunnel = "myboard"
# restart — как сейчас, дефолт ["systemctl","--user","restart","cloudflared"]

[lan]                                            # новое, Phase 2
enabled = true
domain_label = "lan"                             # → *.lan.example.com
ip = "auto"                                      # "auto" (HostNetwork) | "192.168.1.180"
acme_email = "you@example.com"
caddy_admin = "127.0.0.1:2019"
```

Токен — единственный секрет, лежит одним файлом `/var/lib/rpi/cloudflare/token`,
владелец `root`, группа **`rpi-secrets`** (только этот файл; члены — `rpi-agent` и
`caddy`), mode `0640`. Его читают агент (CNAME туннеля + LAN A-запись) и Caddy (ACME
DNS-01) — это **единственный** общий ресурс двух сервисов. Никогда не инлайнится в
`agent.toml`; хранение/чтение — через существующий `secretpath`/`secretsfile`.

`agent.toml` до этого спека версии не имел — добавляем `schema` (u32), как в `rpi.toml`.
Агент его валидирует (текущее — ок; будущее — ошибка; старое → config-migration через
общий фреймворк §7).

### 4.2. `rpi.toml` (на проект) — LAN-HTTPS ingress

```toml
[ingress]
service       = "client"
port          = 80
hostname      = "board.example.com"       # публично через туннель (как сегодня)
lan_hostname  = "board.lan.example.com"   # НОВОЕ: LAN-HTTPS через Caddy
# lan = true                              # сахар: деривит <project>.<label>.<zone>
# expose = "lan"                          # СУЩЕСТВУЮЩЕЕ и ОРТОГОНАЛЬНОЕ (см. ниже)
```

- `lan_hostname` — явный LAN-хост; валидируется, что он под `*.<label>.<zone>`
  (иначе wildcard-серт его не покрывает) → `anyhow::bail!` с подсказкой.
- `lan = true` — сахар: деривит `<project>.<label>.<zone>`; взаимоисключим с
  `lan_hostname`.
- `hostname` и LAN-поля независимы; у одного сервиса могут быть оба (кейс board).
- Ни того ни другого → LAN-ingress нет.

**Коллизия имён с `expose` — явно разводим.** Существующее `[ingress] expose =
"private"|"lan"` управляет **bind-адресом** контейнерного порта (`127.0.0.1` vs
`0.0.0.0`, сырой IP:port без TLS). Новое `lan_hostname`/`lan` управляет **HTTPS-
ingress через Caddy** (домен + доверенный TLS, прокси с хоста на `127.0.0.1:port`).
Это разные оси:

- `lan_hostname` работает при `expose = "private"` (дефолт): Caddy на хосте ходит на
  `127.0.0.1:<host-port>`, `0.0.0.0` не нужен.
- Комбинировать `expose = "lan"` (сырой порт наружу) и `lan_hostname` (HTTPS-домен)
  можно, но обычно не нужно — HTTPS-домен предпочтительнее сырого порта.

Слово «lan» в двух полях — читаемостный риск; оставляем осознанно ради обратной
совместимости `expose`, разница фиксируется в README. (Открытый пункт для ревью —
см. §12.)

### 4.3. CLI — `rpi agent setup`

Новые аргументы (`main.rs` `AgentCmd::Setup`, проброс в `agent::setup::run_cmd`):

```
rpi agent setup \
  --user piuser \
  --cf-token <tok> | --cf-token-file <path> | env CLOUDFLARE_API_TOKEN \
  --domain example.com \
  --acme-email you@example.com \
  --with-cloudflared \        # Phase 1: полный bootstrap туннеля
  --with-lan \                # Phase 2: Caddy + LAN-HTTPS
  --tunnel myboard \          # имя туннеля (дефолт деривится, напр. из hostname)
  --lan-ip 192.168.1.180 \    # override автодетекта HostNetwork
  --dry-run
```

**Обратная совместимость:** `--with-cloudflared` **без** `--cf-token`/`--domain`
сохраняет текущее поведение (scaffold юнита + warning «доделай руками»). **С**
токеном+доменом — полный авто-флоу. Ничего не ломается для существующих инсталляций.
`--with-lan` требует `--cf-token`, `--domain`, `--acme-email`.

Токен-скоупы (документируем в README): `Zone:DNS:Edit` + `Zone:Zone:Read` (для
резолва zone-id) + `Account:Cloudflare Tunnel:Edit`.

Миграции вынесены в отдельную унифицированную подкоманду `rpi agent migrate` (§7), а не
во флаг `setup` — так все миграции запускаются одинаково.

---

## 5. Bootstrap (пошагово, идемпотентно, adopt-or-create, копит `SetupReport`)

Все шаги идут через трейт `Sys` (тестируемо off-Linux). Расходящиеся файлы бэкапятся
в `.bak`, отсутствующие создаются, совпадающие — skip (как уже устроено в `setup.rs`).

### Phase 1 — туннель (расширяет текущий `cloudflared_bootstrap`)

1. Резолв+валидация login-user (`--user`/`$SUDO_USER`, `id -u`). *(камень 1)*
2. Install `cloudflared` по arch (arm64/arm/amd64) → `/usr/local/bin/cloudflared`
   (skip если есть и рабочий). *(камень 2)*
3. Cloudflare API: `find_or_create_tunnel(<name>)`. Если туннель уже есть (кейс
   переиспользования `myboard`) — adopt по имени/ID. Иначе генерим 32-байтный
   `tunnel_secret`, создаём туннель, конструируем credentials-JSON
   `{AccountTag, TunnelID, TunnelName, TunnelSecret}` и пишем в
   `/var/lib/rpi/cloudflared/<id>.json` (owner `rpi-agent`, `640`). *(камни 3,4)*
4. Safe-write `config.yml` (только пробелы, `tunnel`, `credentials-file`,
   `ingress` с обязательным catch-all `http_status:404`) → прогон
   `cloudflared tunnel ingress validate <config>`. *(камни 5,6)*
5. Вписать `[cloudflare]`+`[cloudflared]` в `agent.toml` (только отсутствующие ключи;
   расходящееся — бэкап `.bak`, как для unit сейчас).
6. systemd user-manager:
   - drop-in `/etc/systemd/system/user@<uid>.service.d/override.conf` с
     `XDG_CONFIG_HOME=/var/lib/rpi/.config` и `HOME=/var/lib/rpi`
     (`<uid>` = `id -u rpi-agent`); *(камень 7)*
   - `systemctl daemon-reload` + `systemctl restart user@<uid>`;
   - `loginctl enable-linger rpi-agent` (linger поднимает user-manager и туннель на
     загрузке без логина);
   - `systemctl --user ... enable --now cloudflared` под `rpi-agent` с корректным
     `XDG_RUNTIME_DIR=/run/user/<uid>`.

### Phase 2 — LAN / Caddy

7. Install кастомный `caddy` (с `caddy-dns/cloudflare`) по arch →
   `/usr/local/bin/caddy` (через Caddy download API `?p=github.com/caddy-dns/cloudflare`
   или прибандленный build; skip если есть и с нужным модулем — проверяем
   `caddy list-modules | grep dns.providers.cloudflare`). Создаём **отдельного**
   service-user `caddy` (**не** в группе `docker`), группу `rpi-secrets`
   (`rpi-agent`+`caddy`) на токен-файл, каталог `/etc/rpi/caddy` + data-dir (owner `caddy`).
8. Базовый Caddy-конфиг (JSON): admin `127.0.0.1:2019`; глобальная TLS-автоматизация с
   `subjects: ["*.<label>.<zone>"]` и issuer `acme` + DNS-01 provider `cloudflare`
   (токен из секрет-файла), `email`; HTTP-сервер с авто-редиректом на HTTPS. Плюс
   system-юнит `caddy.service` (`AmbientCapabilities=CAP_NET_BIND_SERVICE` для 80/443).
9. **Проверка портов 80/443:** свободны → `enable --now caddy`. Заняты (nginx) →
   **не трогаем**, печатаем подсказку про `rpi agent migrate --run nginx-to-caddy`
   (§7.6). Без миграции Caddy ставится/конфигурируется, но порты не отбирает.
10. Cloudflare API: `put_dns` `*.<label>.<zone>` A → `HostNetwork.primary_ipv4()` (или
    `[lan].ip`), proxied=false (grey/DNS-only). Adopt если запись уже есть.
11. Прогреть wildcard-серт: дёрнуть Caddy (admin API/loopback), чтобы он выпустил
    `*.<label>.<zone>` по DNS-01. Проверить наличие серта в хранилище.

Bootstrap печатает `SetupReport` (created/skipped/repaired/warnings/errors), как
сейчас; при непустых errors — ненулевой выход.

---

## 6. Деплой-тайм (агент, на проект)

Агент собирает `CompositeIngress` из включённых секций `agent.toml` и для каждого
объявленного в `rpi.toml` экспозишена зовёт `upsert` (при деплое) / `remove` (при
teardown):

- `hostname` → `CloudflaredIngress`: upsert правила в `config.yml` + CNAME
  `hostname → <id>.cfargotunnel.com` (proxied) через API + restart cloudflared.
  Откат локального конфига при неудаче — как уже реализовано (чтобы следующий деплой
  повторил).
- `lan_hostname`/`lan` → `CaddyIngress`: PUT-роут в admin API — matcher по host
  `<lan-host>`, handler `reverse_proxy → http://127.0.0.1:<host-port>` (дефолтные
  forwarded-хедеры Caddy дают паритет с nginx). TLS под wildcard'ом — без действий.
  **Ни DNS, ни reload, ни sudo.** Идемпотентно (PUT именованного роута).

---

## 7. Миграции — унифицированный фреймворк (`rpi agent migrate`)

Все системные миграции (identity/пути/юниты/прокси/формат конфига) идут через **один
детект-ориентированный фреймворк** и **один вход** `rpi agent migrate`, а не через
одноразовые флаги. Добавление будущей миграции не плодит флаги; оператор запускает их
всегда одинаково.

### 7.1. Три слоя версий/миграций — не смешиваем

- **Формат конфига (`agent.toml`):** поле `schema` (u32), как у `rpi.toml`. Агент
  валидирует; при `schema` ниже текущего — **config-migration** (переписать старый
  формат в новый). До этого спека `agent.toml` версии не имел — добавляем `schema = 1`.
- **Данные (`state.db`):** numbered-миграции в `sqlite.rs`, применяются
  **автоматически** на старте агента (напр. `ADD COLUMN lan_host`). Оператор не трогает.
- **Хост (system):** identity/юниты/пути/прокси — `pi-to-rpi`, `nginx-to-caddy`.
  Их унифицируем в фреймворке ниже; config-migration тоже идёт через него
  (её `detect` = `schema < N`).

### 7.2. Модель

```rust
#[async_trait]
trait Migration {
    fn id(&self) -> &str;              // slug: "pi-to-rpi", "nginx-to-caddy", "agent-toml-v2"
    fn description(&self) -> &str;
    fn disruptive(&self) -> bool;      // true => явный opt-in, возможен downtime
    async fn detect(&self, sys: &dyn Sys) -> MigrationState; // Applicable|Done|NotApplicable
    async fn apply(&self, sys: &dyn Sys, dry_run: bool) -> MigrationOutcome;
}
```

- **Реестр** — упорядоченный список всех миграций.
- **Ledger** применённого (id + timestamp): таблица в `state.db`. Это «текущая версия»
  хоста в терминах выполненных миграций → идемпотентность и статус «сделано / ждёт».
- **Детект-ориентированность:** применимость берётся из фактического состояния
  (`detect`), не только из номера версии — `nginx-to-caddy` срабатывает по факту (nginx
  на 80/443), config-migration — по `schema`. Ledger лишь фиксирует сделанное.

### 7.3. Поведение `rpi agent migrate` (root)

CLI: `rpi agent migrate [--list] [--dry-run] [--run <id>]... [--all --yes]`
(`main.rs` `AgentCmd::Migrate`). Флага `--migrate-proxy` нет — proxy-переезд это
миграция `nginx-to-caddy` в реестре.

- `detect()` по каждой; печать унифицированного плана/отчёта.
- **Неразрушающие + применимые** (`disruptive=false`: `pi-to-rpi`, config-migration) —
  применяются автоматически; их же зовёт обычный `setup` (шаг 0) и старт агента, чтобы
  апгрейд был zero-touch. **Та же** зарегистрированная миграция — без дублирования логики.
- **Разрушающие + применимые** (`disruptive=true`: `nginx-to-caddy`) — по умолчанию
  только **репортятся** подсказкой; применяются лишь по явному `--run <id>`
  (или `--all --yes`).
- `--dry-run` — план без изменений; `--list` — все миграции и их статус из ledger.

### 7.4. Миграция `pi-to-rpi` (неразрушающая, авто)

Существующий `setup.rs::migrate_pi_agent_if_present` рефакторится в реализацию
`Migration` (id `pi-to-rpi`). Логика без изменений (stop old unit → quiesce session →
group/usermod rename → move dirs → rewrite paths → backup unit); детект = «есть
`pi-agent`, нет `rpi-agent`»; по завершении — запись в ledger. Продолжает вызываться из
`setup` автоматически.

### 7.5. Миграция `nginx-to-caddy` (разрушающая, staged + reversible)

Ключ бесшовности: **DNS-01 не требует порта** — Caddy выпускает `*.<label>`-серт, пока
nginx ещё держит 80/443. Существующий certbot-серт **не переиспользуется для отдачи**
(иначе renew остался бы на certbot, против цели), а играет роль «старого сервера до
cutover» и кнопки отката.

**Фаза A — подготовка (nginx жив, downtime = 0):**
1. Ставим Caddy + юнит, HTTP/HTTPS-серверы на **временных портах** 8080/8443; 80/443 не
   трогаем; admin `127.0.0.1:2019`.
2. Caddy через DNS-01 выпускает `*.<label>.<zone>`-серт в своё хранилище; проверяем.
3. **Сеем роуты из задеплоенных проектов** (источник — реестр/`state.db`, не парсинг
   nginx): для каждого проекта с LAN-ingress — роут в Caddy через admin API.
4. **Паритет на временных портах:** `curl --resolve <host>:8443:127.0.0.1
   https://<host>:8443/` → тот же ответ, `server: caddy`.

**Фаза B — cutover (downtime ~ доли секунды):**
5. `systemctl stop nginx` — освобождаем 80/443.
6. Через admin API перекидываем Caddy-серверы на 80/443 (rebind — мс).
7. Верификация: `curl -I https://<host>/` → как раньше, `server: caddy`.

**Фаза C — финализация (только если верификация прошла):**
8. `systemctl disable nginx` — **остаётся установленным, конфиги целы** (кнопка отката).
9. `systemctl disable --now certbot.timer` — серты ведёт Caddy; `/etc/letsencrypt` **не
   удаляем** (осиротевший серт безвреден, timer выключен).
10. Запись в ledger; отчёт: что переехало, что осиротело, инструкция отката.

**Откат** (любой шаг верификации падает): Caddy на временные порты (или стоп),
`systemctl start nginx` → прод снова на nginx. Ничего не удалено; ledger миграцию
выполненной не отмечает.

**Ограничения (в отчёте):**
- Переезжают только vhost'ы задеплоенных через `rpi` проектов; руками сделанные
  nginx-сайты вне `rpi` — перечисляем, не переносим.
- HTTP→HTTPS-редирект и forwarded-хедеры Caddy делает сам; кастомные nginx-хедеры не
  транслируются (перечисляем).
- Rate-limit LE: новый `*.<label>`-серт при идентичном существующем — 1 «duplicate» из
  5/неделю; для разовой миграции ок.
- Два DNS-01 (certbot renew vs Caddy issue) не конфликтуют (каждый правит свою TXT по
  record-id).
- Повторный запуск после переезда — no-op: `detect` вернёт `Done`.
- Туннель миграцией не затрагивается (cloudflared не слушает 80/443).

### 7.6. Подсказка о миграции + migration-гайд

`rpi agent setup` и `rpi agent migrate --list` при **применимой разрушающей** миграции
печатают actionable-note (не падают, прод не трогают). Пример:

```
note: обнаружен nginx на 80/443 и certbot-серт *.lan.example.com — LAN-HTTPS ведётся
      вручную. Перенести под управление тулы:
        sudo rpi agent migrate --run nginx-to-caddy
      Что делает и как откатить — docs/migration-nginx-to-caddy.md
```

Отдельный **migration-гайд** `docs/migration-nginx-to-caddy.md` (конвенция
`docs/migration-*.md`, как `migration-v0.5-to-v0.6.md`) описывает: что переезжает,
пофазовый ход, downtime, паритет-чек, откат, что осиротеет и как снести.

---

## 8. Domain / Infrastructure / Application / Wire+CLI

**`domain`:**
- `contracts.rs`: расширить/добавить трейты `Ingress` (есть), новый
  `CloudflareApi: Send + Sync` (`find_or_create_tunnel`, `put_dns`) и
  `CaddyAdmin: Send + Sync` (`put_route`, `delete_route`). Под `#[automock]`/фейки.
- `entities.rs`: расширить `IngressConfig`/`ProjectConfig` полем `lan_host:
  Option<String>` (уже отрезолвленный из `lan_hostname`/`lan`).
- `error.rs`: переиспользуем `DomainError::Ingress`; при необходимости — вариант для
  API-ошибок Cloudflare.

**`infrastructure`:**
- `cloudflare.rs` (новый): HTTP-клиент к Cloudflare API (reqwest уже в зависимостях
  агента), держит токен, реализует `CloudflareApi`. Конструирование credentials-JSON.
- `caddy.rs` (новый): клиент admin API (`127.0.0.1:2019`), реализует `CaddyAdmin`;
  `CaddyIngress: Ingress`.
- `cloudflared.rs`: `CloudflaredIngress::route_dns_and_restart` — заменить шелл
  `cloudflared tunnel route dns` на `CloudflareApi::put_dns` (убирает зависимость от
  `cert.pem` на деплое). Логика upsert/rollback `config.yml` — без изменений.
- `hostnet.rs`: переиспользовать `UdpHostNetwork::primary_ipv4` (из LAN-expose) для
  `[lan].ip = "auto"`.
- `sqlite.rs`/`repo.rs`: numbered-миграция `ADD COLUMN lan_host TEXT` (по образцу
  `expose`), чтобы `rpi ls` показывал LAN-хост без редеплоя; плюс таблица **ledger**
  системных миграций (§7.2). Схемные миграции применяются автоматически на старте — это
  не операторская `migrate`.

**`bin/agent`:**
- `config.rs`: поле `schema: u32` в `AgentConfig` + валидация (`schema == CURRENT`,
  будущее → ошибка, старое → config-migration; по образцу `rpi.toml`); новые секции
  `CloudflareSection { zone, token_file, account_id? }` и
  `LanSection { enabled, domain_label, ip, acme_email, caddy_admin }`.
- `migrate.rs` (новый): трейт `Migration`, `MigrationState`/`MigrationOutcome`, реестр,
  ledger в `state.db`, раннер (§7). Реализации: `pi-to-rpi` (перенос из `setup.rs`),
  `nginx-to-caddy`, config-migration `agent.toml`.
- `setup.rs`: `cloudflared_bootstrap` → полный флоу (§5), новый `caddy_bootstrap`; шаг 0
  зовёт раннер неразрушающих миграций (вместо прямого `migrate_pi_agent_if_present`).
  Всё через `Sys` + тестируемый порт Cloudflare API (в bootstrap ходим напрямую, вне
  `Sys`, но за интерфейсом).
- сборка `CompositeIngress` из секций конфига там, где сейчас выбирается ingress.

**`bin/cli` + wire:**
- `main.rs`: новые аргументы `AgentCmd::Setup` (§4.3) + подкоманда `AgentCmd::Migrate`
  (§7.3), проброс в `run_cmd`/раннер миграций.
- `rpitoml.rs`: `IngressSection` — `#[serde(default)] lan_hostname: Option<String>`,
  `#[serde(default)] lan: bool`; резолв/валидация в `to_project_config` (под
  `*.<label>.<zone>`; взаимоисключение с `lan`).
- `proto.rs`: `ProjectDto`/`ProjectConfig` — прокинуть `lan_host` (`#[serde(default)]`,
  обратная совместимость как у `expose`/healthcheck).
- `commands.rs` (`rpi ls`): показать `lan_hostname` рядом с `hostname`.

---

## 9. Тесты

По образцу `Sys`/`FakeSys` и фейков — всё off-Linux:

- **`cloudflare.rs`:** конструирование credentials-JSON (поля/base64 secret);
  find-or-create (adopt существующего по имени vs create); `put_dns` idempotent
  (adopt существующей записи). Через фейк HTTP или мок трейта.
- **`caddy.rs`:** генерация базового конфига (subjects wildcard, DNS-01 issuer,
  admin addr); `put_route`/`delete_route` формируют корректный JSON; idempotent PUT.
- **`cloudflared.rs`:** upsert/rollback `config.yml` (есть); `route dns` теперь зовёт
  `CloudflareApi` (мок), не шелл.
- **`setup.rs`:** Phase 1 — порядок drop-in → daemon-reload → restart user@uid →
  enable-linger → enable cloudflared; adopt существующего туннеля (creds есть → не
  пересоздаём); safe-write config.yml + вызов `ingress validate`. Phase 2 — install
  caddy, порт-чек ветвится (свободно → enable; занято → подсказка без отбора портов),
  `put_dns` wildcard. dry-run ничего не пишет.
- **`migrate.rs`:** реестр/ledger — `detect` даёт Applicable/Done/NotApplicable;
  неразрушающие применяются авто, разрушающие только по `--run`; повторный прогон
  (`Done`) → no-op; ledger пишется только при успехе. `nginx-to-caddy` — фазы A/B/C, при
  провале верификации откат (nginx start, Caddy на temp-порты), certbot.timer/nginx
  только `disable`, не delete, ledger не отмечен. `pi-to-rpi` — существующие тесты
  переносятся на новый интерфейс без изменения поведения.
- **`rpitoml.rs`:** парсинг `lan_hostname`/`lan`; дефолт-отсутствие; валидация
  «под wildcard»; отказ на `lan_hostname` вне `*.<label>.<zone>`; взаимоисключение
  `lan`+`lan_hostname`.
- **`proto.rs`:** roundtrip `lan_host` через DTO; payload без него → `None`.
- **`config.rs`:** парсинг `[cloudflare]`/`[lan]` с дефолтами; отсутствие секций —
  фичи выключены; валидация `schema` (текущее — ок; будущее → ошибка; старое →
  config-migration применима).

---

## 10. Документация (README)

- Раздел «Cloudflare Tunnel» — переписать под авто-bootstrap: один вызов с токеном,
  что делает тула, скоупы токена; ручной путь оставить как fallback.
- Новый раздел «LAN HTTPS (Caddy)»: `lan_hostname`/`lan`, `[lan]` в `agent.toml`,
  как работает DNS-01 wildcard, что серт доверенный без своего CA.
- **Разъяснить `expose` vs `lan_hostname`** (bind-адрес vs HTTPS-ingress) — во
  избежание путаницы.
- Раздел «Миграции» (`rpi agent migrate`): унифицированный вход, `--list`/`--dry-run`/
  `--run`, `schema` в `agent.toml`; отдельный `docs/migration-nginx-to-caddy.md` (что
  делает переезд, downtime, откат, что осиротеет — certbot-серт/timer).
- Заметка безопасности: приватный LAN-IP публикуется в публичном DNS (вариант A);
  admin API Caddy — только loopback, без авторизации (single-tenant Pi).

---

## 11. Фазирование

Один спек, реализация двумя shippable-фазами; фундамент — в первой.

- **Phase 1 — полный авто-bootstrap Cloudflare Tunnel + фундамент.** Закрывает камни
  1–7: API-создание туннеля, drop-in systemd-фикс, DNS-через-API для существующего
  публичного ingress, install cloudflared. Фундамент: `Cloudflare` API-клиент + модель
  токена/секрета + секция `[cloudflare]` + `schema` в `agent.toml` + фреймворк миграций
  (`migrate.rs`, рефактор `pi-to-rpi` в него). Самодостаточна.
- **Phase 2 — LAN-HTTPS через Caddy.** Install/конфиг Caddy, wildcard-серт, `*.lan`
  DNS, `CaddyIngress`, поверхность `lan_hostname`/`lan`, миграция `nginx-to-caddy` (в
  фреймворке из Phase 1). Переиспользует фундамент Phase 1.

Каждая фаза → свой план (`writing-plans`) → имплементация → security-review (каталог
`docs/superpowers/security/`, как у прочих фич с секретами/сетью).

---

## 12. Принятые решения (ревью пройдено)

1. **Нейминг:** `lan_hostname` (+ сахар `lan`), рядом с существующим `expose` —
   ортогональная ось (bind-адрес vs HTTPS-ingress); разница фиксируется в README.
2. **Service-user Caddy:** **отдельный `caddy`**, **не** в группе `docker` (изоляция
   сетевого TLS-терминатора от root-эквивалентного `rpi-agent`); единственный общий
   ресурс — токен, через группу `rpi-secrets` на один файл.
3. **Вход миграций:** подкоманда `rpi agent migrate` (без флага-алиаса на `setup`);
   разрушающие — только по явному `--run <id>`.
4. **Ledger миграций:** таблица в `state.db` (одно место состояния).
