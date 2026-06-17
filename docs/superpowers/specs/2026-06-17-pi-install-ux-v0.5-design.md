# pi v0.5 — Установка и UX (дизайн)

Дата: 2026-06-17. Базовая спека: `2026-06-09-pi-deploy-tool-design.md`
(§13 self-setup агента, §15 интерактив клиента, §16 команды, §17 каталоги,
§23 v0.5). Предшествующая версия: v0.4 (`2026-06-12-pi-operations-v0.4-design.md`).

**Критерий готовности:** ни на Пайке, ни на клиенте не нужно вручную
редактировать файлы. На Пайке — один `sudo pi agent setup`. На клиенте —
`pi setup` (профиль + SSH-ключ) и `pi init` (генерация `pi.toml`). На уже
работающей ручной установке `setup` идемпотентен: чинит недостающее
(`/var/log/pi`), ничего рабочего не ломает.

Полный «человек со стороны ставит с нуля за ~10 минут» (с `install.sh` и
публикацией релизных бинарей) сознательно перенесён в v0.6 — см. §1.

## 1. Скоуп

Входит:

- **Агент (локально на Пайке, `sudo`):**
  - `pi agent setup` — идемпотентный бутстрап (юзер `pi-agent`, каталоги,
    группы, systemd-юнит, опц. cloudflared). Политика **Adopt & preserve**
    (§4).
  - `pi agent uninstall [--purge]` — снятие сервиса/юнита/юзера; данные
    `/var/lib/pi` сохраняются без `--purge` (§5).
- **Клиент (Windows/dev-машина):**
  - `pi setup` — `inquire`-мастер: профиль сервера + авто-бутстрап SSH-ключа
    client→Pi + запись/merge `config.toml` + финальный тест связи (§6).
  - `pi init` — `inquire`-мастер: авто-детект и генерация `pi.toml`, включая
    новый `expose = private|lan` (§7).
- **Неинтерактивный режим** для всего: `--yes`, `--host/--user/--key`,
  `--name/--repo/...` (§6, §7).
- **Опционально (если дёшево):** интерактивное «отменить деплой?» на Ctrl+C
  во время `pi deploy`/`-f`-follow — перенос из v0.4 (§11).

НЕ входит (→ v0.6+, детально в §13):

- `install.sh` + публикация релизных бинарей (aarch64/armv7/Windows) + sha256.
  Бинарь по-прежнему ставится из исходников (`cargo install` на клиенте,
  `cargo build --release` на Пайке). Это меняет критерий «10 минут» —
  поэтому он перенесён вместе с релизами.
- `pi agent update` (drain + подмена бинаря, §13 базовой спеки). Путь
  обновления в v0.5: пересобрать → повторный `sudo pi agent setup` (чинит
  конфиг) → `sudo systemctl restart pi-agent`.
- Удалённый запуск `pi agent setup` по SSH с клиента. setup — локальная
  привилегированная операция на Пайке.

## 2. Ключевые решения

1. **`pi agent setup` — локально на Пайке под `sudo`.** Все шаги — root-операции
   на хосте (useradd, каталоги в `/var/lib` и `/etc`, systemd-юнит). Точно
   повторяет ручной флоу из README; тестируется и не требует sudo-over-ssh.
   Клиентскую часть (профиль, SSH-ключ) делает `pi setup`.
2. **Adopt & preserve по умолчанию (§4).** Инварианты never-touch: `secret.key`
   и `state.db` никогда не пересоздаются (их и так создаёт агент через
   exclusive-create — `EncryptedFileStore::open` / `open_or_create_identity`).
   Каталоги/юзер — только если отсутствуют. `agent.toml` — пишется только при
   отсутствии. systemd-юнит — канонический; при отличии бэкап в `*.bak`.
   Членство в группах и `authorized_keys` — только добавление.
3. **Канонические шаблоны = реальные рабочие файлы (§9).** systemd-юнит и
   `agent.toml` зафиксированы байт-в-байт по факту рабочей установки, поэтому
   повторный `setup` на ней — no-op (без `.bak`, без перезаписи).
4. **cloudflared и linger — opt-in.** На рабочей установке нет `[cloudflared]`
   и выключен linger (ручной ingress). Бутстрап cloudflared + `enable-linger`
   выполняются только при `--with-cloudflared`; по умолчанию не трогаются.
5. **Главный repair: `/var/log/pi`.** На рабочей установке каталога нет, а v0.4
   по умолчанию пишет rolling-логи в `/var/log/pi` и при недоступности тихо
   деградирует в stderr-only (commit `dafab06`). `setup` создаёт
   `/var/log/pi` (владелец `pi-agent`) и печатает подсказку перезапустить
   агент, чтобы включились файловые логи. Сам не рестартит (не мешает работе).
6. **`pi agent update` не реализуется** (см. §1). `setup` идемпотентен и
   закрывает «починить/донастроить конфигурацию».
7. **`pi setup` не устанавливает бинарь.** Бинарь — это и есть запускаемый `pi`.
   Установка бинаря из исходников остаётся ручной (документируется), её
   автоматизация (`install.sh`) — v0.6.

## 3. Поверхность: CLI

Новые команды (`main.rs`, clap-derive — как существующие):

| Команда | Поведение |
|---|---|
| `pi setup` | мастер настройки клиента: профиль сервера + SSH-ключ + `config.toml`; флаги `--host/--user/--key/--name/--default/--yes` для CI |
| `pi init` | мастер генерации `pi.toml` в текущем проекте; флаги `--name/--repo/--branch/--compose/--hostname/--port/--service/--expose/--env/--yes` |
| `pi agent setup` | локальный бутстрап Пайки под `sudo`; флаги `--user <login-user>`, `--with-cloudflared`, `--dry-run` |
| `pi agent uninstall` | снятие агента; флаг `--purge` (удалить и данные), без него — данные сохраняются; подтверждение именем хоста/`--yes` |

`pi agent setup`/`uninstall` — **локальные** операции на Пайке: **без**
`ConnectOpts`, диспатчатся напрямую (как `agent run`), вне tracing-инициализации
клиентских команд. `pi setup`/`pi init` — клиентские, без `ConnectOpts` в
обычном смысле (сами формируют профиль), но используют `SshExec`/тоннель для
теста связи.

Новых HTTP-эндпоинтов агента нет. v0.5 — только клиент и локальный бутстрап.

## 4. `pi agent setup` — поведение (Adopt & preserve)

Источник логин-юзера для членства в группе `pi-agent`: `--user`, иначе
`$SUDO_USER`. Запуск как root без `sudo` и без `--user` — ошибка с подсказкой.

| Артефакт | Действие | На рабочей установке |
|---|---|---|
| юзер `pi-agent` (`--system`, nologin, без home) | создать, если нет | есть (uid 999) → no-op |
| группа `pi-agent` → `docker` | `usermod -aG docker pi-agent`, если не член | уже член → no-op |
| логин-юзер → группа `pi-agent` | `usermod -aG pi-agent <user>`, если не член | `piuser` уже член → no-op |
| `/var/lib/pi` | `install -d -o pi-agent -g pi-agent`, если нет | есть (755, pi-agent) → no-op |
| **`/var/log/pi`** | `install -d -o pi-agent -g pi-agent`, если нет | **нет → создаётся (repair, §2.5)** |
| `/etc/pi` | создать, если нет | есть → no-op |
| `/etc/pi/agent.toml` | записать канон (§9), **только если отсутствует** | есть → не трогаем |
| `secret.key`, `state.db`(+`-wal`/`-shm`) | **никогда не трогаем** | владелец pi-agent, 0600/644 → не трогаем |
| `/etc/systemd/system/pi-agent.service` | записать канон (§9); при отличии байт-в-байт — бэкап `*.bak`; `daemon-reload`; `enable --now` | идентичен канону → no-op |
| linger для `pi-agent` | `loginctl enable-linger pi-agent` — **только при `--with-cloudflared`** | без флага → не трогаем |
| cloudflared (user-юнит + `config.yml` + `tunnel login`) | бутстрап — **только при `--with-cloudflared`** | без флага → не трогаем |
| зависимости `docker` / `docker compose` (+`cloudflared` при флаге) | проверить наличие → предупреждение и инструкция, **не падать молча** | docker есть → PASS |

Каталоги, создаваемые агентом самостоятельно при работе (`keys/`, `secrets/`,
`overrides/`, `workdirs/`, `.docker/`, `known_hosts`, `.config`/`.cache`),
`setup` не создаёт — это зона self-init агента (§13 базовой спеки).

Поведение:

- `--dry-run` печатает план (что создаст/изменит/пропустит), ничего не делает.
- Повторный запуск только дозаполняет недостающее; печатает итог
  (created/skipped/repaired) и, если создал `/var/log/pi`, подсказку
  `sudo systemctl restart pi-agent`.
- Агент **не рестартится** автоматически (не мешаем активным деплоям).

## 5. `pi agent uninstall`

- По умолчанию: `systemctl disable --now pi-agent`, удалить
  `/etc/systemd/system/pi-agent.service` (+`*.bak`), `daemon-reload`, удалить
  юзера `pi-agent`. **Данные `/var/lib/pi`, `/etc/pi`, `/var/log/pi`
  сохраняются.**
- `--purge`: дополнительно удалить `/var/lib/pi` (секреты, БД, ключи!),
  `/etc/pi`, `/var/log/pi`. Требует подтверждения (ввод имени хоста) либо
  `--yes`. Громкое предупреждение про необратимость (секреты/deploy-ключи).
- cloudflared user-юнит и linger снимаются только если были поставлены
  (детект); по умолчанию не трогаются.
- Идемпотентно: повторный `uninstall` доделывает остаток без ошибок.

## 6. `pi setup` — клиент

Мастер (`inquire`), неинтерактивный режим — флагами.

1. **Профиль:** alias (дефолт `home`), host/IP, SSH-user, путь к ключу.
   Авто-детект: ключи в `~/.ssh/` (`id_ed25519`/`id_rsa`/прочие приватные),
   существующий `~/.ssh/config` `Host`-блок (предложить переиспользовать
   HostName/User/IdentityFile). На рабочей установке детект найдёт `~/.ssh/pi`
   и блок `Host pihost.local`.
2. **Авто-бутстрап SSH-ключа client→Pi (§15.1 базовой спеки):**
   - если связь по ключу уже работает (`ssh -o BatchMode=yes <host> true`) —
     **adopt**, ничего не пушим (текущий кейс пользователя);
   - иначе если ключа нет — `ssh-keygen -t ed25519` (с подтверждением);
   - залить pubkey ssh-copy-id-эквивалентом: `ssh <user>@<host> "umask 077;
     mkdir -p ~/.ssh && cat >> ~/.ssh/authorized_keys"` со стандартным вводом
     pubkey (один раз спросит пароль; работает в Windows OpenSSH, где нет
     `ssh-copy-id`); только append;
   - проверить вход по ключу.
   - Fallback: если на Pi выключена парольная аутентификация и ключа нет —
     показать pubkey и инструкцию занести вне программы; `pi doctor`
     диагностирует.
3. **Запись `config.toml` (merge, Adopt & preserve):** прочитать существующий
   `%APPDATA%\pi\config.toml` (или `~/.config/pi/config.toml`), добавить/обновить
   профиль `[servers.<alias>]`, сохранить остальные профили и `default`
   (установить `default`, если его нет). На рабочей установке профиль `home`
   с `key = "~/.ssh/pi"` сохраняется как есть.
4. **Тест связи:** SSH-коннект → ping агента (`/v1/version` через тоннель,
   как существующие команды) → удалённый `pi doctor`. Печать PASS/FAIL.

Неинтерактив: `pi setup --host pihost.local --user piuser --key ~/.ssh/pi
--name home --yes` (пропускает промпты, использует детект-дефолты).

## 7. `pi init` — клиент

Мастер генерации `pi.toml` (схема 1, §12 базовой спеки) с авто-детектом:

| Поле | Детект |
|---|---|
| `project.name` | имя текущей папки |
| `source.repo` | `git remote get-url origin` |
| `source.branch` | текущая ветка |
| `build.compose` | найденный `docker-compose.yml`/`compose.yaml`/… |
| `ingress.service` | первый сервис с портами из compose (подсказка) |
| `ingress.port` | внутренний порт сервиса (подсказка) |
| `ingress.hostname` | опционально (пусто = без публичного ingress) |
| `ingress.expose` | `private` (дефолт) / `lan` — из lan-expose |
| `env.file` | `.env`, если есть |

- Валидация ввода; запись `pi.toml` (не перезаписывать существующий без
  подтверждения; при подтверждении — бэкап `pi.toml.bak`).
- В конце предложить выполнить `pi env send` и `pi deploy`.
- Неинтерактив: все поля флагами + `--yes`.

`pi.toml` генерируется через существующий модуль схемы (`cli/pitoml.rs`),
расширенный полем `expose`.

## 8. Архитектура и модули

Следуем текущим паттернам (`HostSystemProbe` с инжектируемым runner'ом из v0.4,
clap-derive в `main.rs`, тонкие команды в `cli/commands.rs`).

- `main.rs`: добавить `Cmd::Setup`, `Cmd::Init`, `AgentCmd::Setup`,
  `AgentCmd::Uninstall`. `agent setup/uninstall` диспатчатся напрямую (как
  `agent run`), до инициализации клиентского tracing.
- `crates/bin/src/agent/setup.rs` (новый) — бутстрап Пайки над трейтом
  `SystemRunner` (вызовы `useradd`/`usermod`/`install -d`/`loginctl`/
  `systemctl`/`id`/запись файлов). Чистая оркестрация: юнит-тесты на фейковом
  runner'е (последовательность + идемпотентность) на Windows; реальные
  root-операции — `#[ignore]`-интеграция.
- `crates/bin/src/agent/uninstall.rs` (новый) — над тем же `SystemRunner`.
- `crates/bin/src/cli/setup.rs` (новый) — мастер `pi setup` + SSH-бутстрап;
  переиспользует `cli/ssh.rs` (`SshExec`), `cli/tunnel.rs`, `cli/api.rs`
  (ping/doctor). Промпты — за трейтом `Prompter` (фейк в тестах).
- `crates/bin/src/cli/init.rs` (новый) — мастер `pi init` + авто-детект
  (`git`, fs, парс compose) + генерация через `cli/pitoml.rs`.
- `cli/config.rs`: добавить `Serialize` к `ClientConfig`/`ServerProfile` +
  `save_merged(profile)` (Adopt & preserve). Сейчас тип только `Deserialize`.
- `agent/config.rs`: канонический шаблон `agent.toml` как константа/функция
  (для записи при отсутствии); `AgentConfig` остаётся для чтения/валидации.
- Шаблоны systemd-юнита и `agent.toml` — константы в `agent/setup.rs`,
  совпадающие с §9.
- Новая зависимость: **`inquire`** (кроссплатформенные промпты, работает в
  PowerShell). Других новых зависимостей не предполагается.

## 9. Канонические шаблоны (= рабочая установка)

`/etc/systemd/system/pi-agent.service` (байт-в-байт по факту):

```ini
[Unit]
Description=pi deploy agent
After=network-online.target docker.service
Wants=network-online.target

[Service]
User=pi-agent
Group=pi-agent
ExecStart=/usr/local/bin/pi agent run --config /etc/pi/agent.toml
RuntimeDirectory=pi
RuntimeDirectoryMode=0750
Restart=on-failure
Environment=HOME=/var/lib/pi
Environment=XDG_CONFIG_HOME=/var/lib/pi/.config
Environment=XDG_CACHE_HOME=/var/lib/pi/.cache
WorkingDirectory=/var/lib/pi

[Install]
WantedBy=multi-user.target
```

`/etc/pi/agent.toml` (пишется только при отсутствии; совпадает с фактическим):

```toml
data_dir = "/var/lib/pi"
socket = "/run/pi/agent.sock"
port_min = 8000
port_max = 8999
build_concurrency = 1
history_keep = 50

[timeouts]
fetch = "2m"
build = "30m"
up = "5m"

[gc]
disk_threshold_percent = 85
```

(`[logs]` не добавляем: дефолт `dir=/var/log/pi` в коде уже корректен,
ключевое — создать сам каталог, §4.)

Клиентский `config.toml` (merge сохраняет такой профиль как есть):

```toml
default = "home"

[servers.home]
host = "pihost.local"
user = "piuser"
key = "~/.ssh/pi"
```

Расхождение README ↔ факт: README печатает `Environment`-строки после
`WorkingDirectory`; канон выше следует фактическому порядку рабочего юнита.
README приводится к канону отдельной правкой (§12), чтобы документация и шаблон
совпадали.

## 10. Тестирование (как в v0.4)

- `pi agent setup`/`uninstall` — юнит на фейковом `SystemRunner`: точная
  последовательность команд, идемпотентность (повтор → skip), `--dry-run`
  ничего не вызывает, отсутствие `/var/log/pi` → ровно один вызов создания
  (repair). Реальные root-операции — `#[ignore]`.
- `pi setup` — юнит на фейковых `Prompter` + `SshExec`: adopt (связь уже есть →
  без пуша), генерация ключа при отсутствии, append в `authorized_keys`,
  fallback при выключенной парольной аутентификации.
- merge `config.toml` — golden-string: добавление профиля сохраняет прочие и
  `default`.
- `pi init` — детект на временном git-репо/compose (фикстуры); генерация
  `pi.toml` с `expose`; не перезаписывает без подтверждения.
- Генерация systemd-юнита/`agent.toml` — сверка с §9 (golden-string).

## 11. Опционально: cancel по Ctrl+C

Перенос из v0.4: во время `pi deploy` (и `-f`-follow) перехват Ctrl+C →
интерактивный вопрос «отменить активный деплой?» → при подтверждении
`deploy --cancel`-семантика. Включается в v0.5 только если не раздувает объём;
иначе остаётся отдельным мелким пунктом. Низкий приоритет.

## 12. Сопутствующие правки

- README: секции ручной установки заменяются/дополняются `pi agent setup` /
  `pi setup` / `pi init`; ручной путь остаётся как «из исходников / fallback».
  Порядок строк systemd-юнита в README приводится к §9.
- `docs/install-agent-v0.1.md` — отметить, что `/var/log/pi` теперь создаёт
  `pi agent setup`.
- Статус в README: v0.5.

## 13. Перенесено в v0.6

Явный список вынесенного из v0.5 — с причиной и зацепками для будущего плана.

| Пункт | Что входит | Почему перенесли | Ссылки |
|---|---|---|---|
| Релизные бинари + `install.sh` | CI-сборка `aarch64`/`armv7` (Pi) + Windows, публикация в GitHub Releases с sha256; `curl -fsSL .../install.sh \| sh` (детект арки → `/usr/local/bin/pi` → сверка sha256) | Самый большой отдельный кусок (CI + хостинг артефактов); v0.5 ставит бинарь из исходников | §13, §22 базовой спеки |
| `pi agent update` (drain) | Перевод агента в draining (новые деплои → 503 + подсказка повторить), дожидание активных деплоев (drain-таймаут из `agent.toml`), затем подмена бинаря (sha256) + restart; прерванный деплой помечается `interrupted` свипом при старте | Бессмысленно без публикации релизов — нечего «скачивать»; требует протокольной работы в агенте | §13, §8.1 базовой спеки |
| Удалённый `pi agent setup` по SSH | `pi agent setup --server home` с клиента: привилегированный бутстрап Пайки удалённо (sudo-over-ssh) | Сложность (sudo-промпты, экранирование команд, тестируемость на Windows); в v0.5 setup — локальный под `sudo` | §2, решение 1 |
| Критерий «с нуля за ~10 минут» | Полный путь «поставил из репозитория → само настроилось → подняло сервис» для человека со стороны | Зависит от `install.sh`: установка бинаря из исходников ≠ 10 минут | §23 базовой спеки (исходный критерий v0.5) |

Остаётся за рамками и v0.6 — в «Точках расширения» (§21 базовой спеки):
`pi agent backup`/`restore`, авто-регистрация deploy-key через GitHub API,
авто-удаление DNS при `pi rm`, webhook-receiver, мульти-Pi профили.

Версия по завершении: **0.5.0**.
