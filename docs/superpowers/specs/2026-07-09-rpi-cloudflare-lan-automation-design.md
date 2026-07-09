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
- Staged-reversible миграция host-nginx → Caddy на 80/443 по явному `--migrate-proxy`.

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
mode `0640`, владелец `root:rpi-agent`; `caddy`-сервис читает его как член группы
`rpi-agent` (или через `EnvironmentFile` своего юнита). Никогда не инлайнится в
`agent.toml`. Хранение/чтение — через существующий `secretpath`/`secretsfile`.

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
  --migrate-proxy \           # явно забрать 80/443 у nginx (см. §7)
  --dry-run
```

**Обратная совместимость:** `--with-cloudflared` **без** `--cf-token`/`--domain`
сохраняет текущее поведение (scaffold юнита + warning «доделай руками»). **С**
токеном+доменом — полный авто-флоу. Ничего не ломается для существующих инсталляций.
`--with-lan` требует `--cf-token`, `--domain`, `--acme-email`.

Токен-скоупы (документируем в README): `Zone:DNS:Edit` + `Zone:Zone:Read` (для
резолва zone-id) + `Account:Cloudflare Tunnel:Edit`.

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
   `caddy list-modules | grep dns.providers.cloudflare`). Создаём service-user
   `caddy` (или переиспользуем `rpi-agent`), каталог `/etc/rpi/caddy` + data-dir.
8. Базовый Caddy-конфиг (JSON): admin `127.0.0.1:2019`; глобальная TLS-автоматизация с
   `subjects: ["*.<label>.<zone>"]` и issuer `acme` + DNS-01 provider `cloudflare`
   (токен из секрет-файла), `email`; HTTP-сервер с авто-редиректом на HTTPS. Плюс
   system-юнит `caddy.service` (`AmbientCapabilities=CAP_NET_BIND_SERVICE` для 80/443).
9. **Проверка портов 80/443:** свободны → `enable --now caddy`. Заняты (nginx) →
   **не трогаем**, warning + требование `--migrate-proxy` (см. §7). Без флага Caddy
   ставится/конфигурируется, но порты не отбирает.
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

## 7. Миграция host-nginx → Caddy (`--migrate-proxy`, staged + reversible)

Ключ бесшовности: **DNS-01 не требует порта** — Caddy выпускает `*.<label>`-серт,
пока nginx ещё держит 80/443. Существующий certbot-серт при этом **не переиспользуется
для отдачи** (иначе renew остался бы на certbot, что противоречит цели), а играет
роль «старого сервера до cutover» и кнопки отката.

**Фаза A — подготовка (nginx жив, downtime = 0):**
1. Ставим Caddy + юнит, но HTTP/HTTPS-серверы на **временных портах** 8080/8443;
   80/443 не трогаем; admin на `127.0.0.1:2019`.
2. Caddy через DNS-01 выпускает `*.<label>.<zone>`-серт в своё хранилище; проверяем.
3. **Сеем роуты из задеплоенных проектов** (канонический источник — реестр
   проектов/`state.db`, не парсинг nginx): для каждого проекта с LAN-ingress — роут
   в Caddy через admin API.
4. **Паритет на временных портах:** `curl --resolve <host>:8443:127.0.0.1
   https://<host>:8443/` → тот же ответ, `server: caddy`.

**Фаза B — cutover (downtime ~ доли секунды):**
5. `systemctl stop nginx` — освобождаем 80/443.
6. Через admin API перекидываем Caddy-серверы с 8080/8443 на 80/443 (rebind — мс).
7. Верификация на реальных портах: `curl -I https://<host>/` → 2xx/4xx как раньше,
   `server: caddy`.

**Фаза C — финализация (только если верификация прошла):**
8. `systemctl disable nginx` — **остаётся установленным, конфиги целы** (кнопка отката).
9. `systemctl disable --now certbot.timer` — серты ведёт Caddy. `/etc/letsencrypt`
   **не удаляем** (осиротевший серт безвреден, авто-продлевался бы — но timer выключен).
10. Отчёт: что переехало, что осиротело, инструкция отката.

**Откат** (любой шаг верификации падает): вернуть Caddy на временные порты (или
остановить), `systemctl start nginx` → прод снова на nginx. Ничего не удалено.

**Ограничения (честно, в отчёте):**
- Переезжают только vhost'ы задеплоенных через `rpi` проектов. Руками сделанные
  nginx-сайты вне `rpi` — перечисляем в warning, не переносим.
- HTTP→HTTPS редирект и forwarded-хедеры Caddy делает сам; кастомные nginx-хедеры не
  транслируются (перечисляем).
- Rate-limit LE: выпуск нового `*.<label>`-серта при существующем идентичном — 1
  «duplicate certificate» из лимита 5/неделю; для разовой миграции не проблема.
- Два DNS-01 (certbot renew vs Caddy issue) не конфликтуют: каждый добавляет/удаляет
  **свою** TXT по record-id через Cloudflare API.
- Повторный `--migrate-proxy` после переезда — no-op (Caddy на 80/443, nginx disabled),
  только сверка роутов.
- Туннель миграцией не затрагивается (cloudflared не слушает 80/443).

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

**`bin/agent`:**
- `config.rs`: новые секции `CloudflareSection { zone, token_file, account_id? }` и
  `LanSection { enabled, domain_label, ip, acme_email, caddy_admin }`; `CloudflaredSection`
  — как есть.
- `setup.rs`: расширить `SetupOpts`/`cloudflared_bootstrap` до полного флоу (§5),
  добавить `caddy_bootstrap`, `migrate_proxy`. Всё через `Sys` + новый тонкий порт для
  Cloudflare API (в bootstrap ходим напрямую, вне трейта `Sys`, но за тестируемым
  интерфейсом).
- сборка `CompositeIngress` из секций конфига там, где сейчас выбирается ingress.

**`bin/cli` + wire:**
- `main.rs`: новые аргументы `AgentCmd::Setup` (§4.3), проброс в `run_cmd`.
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
  caddy, порт-чек ветвится (свободно → enable; занято → warning без отбора портов),
  `put_dns` wildcard. `--migrate-proxy` — последовательность фаз A/B/C, при провале
  верификации откат (nginx start, Caddy на temp-порты), certbot.timer/nginx только
  `disable`, не delete. dry-run ничего не пишет.
- **`rpitoml.rs`:** парсинг `lan_hostname`/`lan`; дефолт-отсутствие; валидация
  «под wildcard»; отказ на `lan_hostname` вне `*.<label>.<zone>`; взаимоисключение
  `lan`+`lan_hostname`.
- **`proto.rs`:** roundtrip `lan_host` через DTO; payload без него → `None`.
- **`config.rs`:** парсинг `[cloudflare]`/`[lan]` с дефолтами; отсутствие секций —
  фичи выключены.

---

## 10. Документация (README)

- Раздел «Cloudflare Tunnel» — переписать под авто-bootstrap: один вызов с токеном,
  что делает тула, скоупы токена; ручной путь оставить как fallback.
- Новый раздел «LAN HTTPS (Caddy)»: `lan_hostname`/`lan`, `[lan]` в `agent.toml`,
  как работает DNS-01 wildcard, что серт доверенный без своего CA.
- **Разъяснить `expose` vs `lan_hostname`** (bind-адрес vs HTTPS-ingress) — во
  избежание путаницы.
- Раздел «Миграция nginx → Caddy»: что делает `--migrate-proxy`, downtime, откат,
  что осиротеет (certbot-серт/timer).
- Заметка безопасности: приватный LAN-IP публикуется в публичном DNS (вариант A);
  admin API Caddy — только loopback, без авторизации (single-tenant Pi).

---

## 11. Фазирование

Один спек, реализация двумя shippable-фазами; фундамент — в первой.

- **Phase 1 — полный авто-bootstrap Cloudflare Tunnel.** Закрывает камни 1–7:
  API-создание туннеля, drop-in systemd-фикс, DNS-через-API для существующего
  публичного ingress, install cloudflared. Строит фундамент: `Cloudflare` API-клиент
  + модель токена/секрета + секция `[cloudflare]`. Самодостаточна.
- **Phase 2 — LAN-HTTPS через Caddy.** Install/конфиг Caddy, wildcard-серт, `*.lan`
  DNS, `CaddyIngress`, поверхность `lan_hostname`/`lan`, миграция `--migrate-proxy`.
  Переиспользует фундамент Phase 1.

Каждая фаза → свой план (`writing-plans`) → имплементация → security-review (каталог
`docs/superpowers/security/`, как у прочих фич с секретами/сетью).

---

## 12. Открытые пункты для ревью

1. **Нейминг `lan` vs `expose`.** Оставлен `lan_hostname`/`lan` рядом с существующим
   `expose = "lan"`. Слово «lan» в двух ролях. Альтернатива — назвать HTTPS-поле
   иначе (`https_host`, `lan_tls_host`), но тогда теряется симметрия с `hostname`.
   Решение по умолчанию — `lan_hostname`; готов сменить.
2. **Service-user для Caddy.** Отдельный `caddy` (изоляция) vs переиспользовать
   `rpi-agent` (меньше сущностей, проще доступ к токену). Дефолт — отдельный `caddy`
   в группе `rpi-agent` для чтения токена.
3. **Миграция текущего бокса.** По умолчанию `--migrate-proxy` — явный ручной шаг;
   авто-миграцию на существующем nginx-боксе не делаем. Ок?
