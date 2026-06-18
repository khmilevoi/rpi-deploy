# pi v0.6 — Скрипты установки и обновления из исходников (дизайн)

Дата: 2026-06-18. Базовая спека: `2026-06-09-pi-deploy-tool-design.md`
(§13 установка/self-setup, §16 команды, §23 роадмап). Предшествующая версия:
v0.5 (`2026-06-17-pi-install-ux-v0.5-design.md`, §13 — список переноса).

**Критерий готовности:** человек со стороны ставит тулу с нуля одной командой
(`curl … | sh -s -- --agent` на Пайке, `irm … | iex` на Windows-клиенте) —
скрипт ставит пререквизиты, клонирует репо, собирает из исходников, устанавливает
бинарь и поднимает/чинит агент. Повторное обновление — отдельной командой
(`update`), которая пулит и пересобирает.

## 1. Контекст и смена подхода

Исходный план v0.6 (из v0.5 §13) предполагал **пребилт-бинари + `install.sh` со
сверкой sha256**: CI кросс-сборки `aarch64`/`armv7`/Windows, публикация в GitHub
Releases, `install.sh` качает готовый бинарь.

**Решение пересмотрено:** сборка из исходников полностью устраивает. Пребилт-бинари,
GitHub Releases, sha256 и кросс-CI **не делаем**. Вместо этого — **скрипты**,
которые автоматизируют уже задокументированный в README ручной флоу
(rustup → `git clone` → `cargo build --release` → `install` → `pi agent setup`).
Это снимает весь вопрос про матрицу таргетов, хостинг артефактов и macOS-раннеры.

Заодно скрипт обновления закрывает практический кусок `pi agent update`
(пересобрать → переустановить → рестартнуть демон), но **без drain-протокола**:
рестарт `pi-agent` ловится существующим свипом `running→interrupted` (§8.1 базовой
спеки), отдельная протокольная работа в агенте не нужна.

## 2. Скоуп

Входит:

- **`scripts/install.sh`** (POSIX sh, Linux/macOS, запускается `curl … | sh`):
  bootstrap с провижинингом пререквизитов, клон в стандартную папку, сборка,
  установка. Роль — флаг `--agent` / `--client` (§4).
- **`scripts/update.sh`** (Linux/macOS): из клона `git pull` → сборка →
  переустановка (+ рестарт демона для `--agent`) (§5).
- **`scripts/install.ps1`** (PowerShell, Windows, запускается `irm … | iex`):
  bootstrap клиента (§6).
- **`scripts/update.ps1`** (Windows): обновление клиента (§6).
- **Сопутствующее:** `--dry-run`/`-DryRun` в каждом скрипте; линт
  (`shellcheck`/`PSScriptAnalyzer`); правки README; bump версии до `0.6.0` (§9–§11).

НЕ входит (→ v0.7+, §12):

- Пребилт-бинари + публикация в GitHub Releases + sha256 + кросс-CI.
- `pi agent update` как команда агента с draining/503-протоколом.
- Удалённый `pi agent setup` по SSH с клиента (sudo-over-ssh).

## 3. Ключевые решения

1. **Две ортогональные оси:** жизненный цикл (**install** = bootstrap с
   провижинингом всего необходимого; **update** = только pull+build+install) ×
   роль (**`--agent`** на Пайке: sudo, systemd, рестарт демона; **`--client`**:
   собрать и положить `pi` в PATH). Платформа задаёт реализацию: `.sh` для
   Linux/macOS, `.ps1` для Windows.
2. **Роль — явный флаг, не автодетект.** Из ОС роль не выводится надёжно
   (Linux-машина может быть и агентом, и клиентом). `install.sh` требует ровно
   один из `--agent`/`--client`. `--agent` на не-Linux/без systemd — ошибка.
3. **Сборка как логин-юзер, установка под sudo.** rustup/cargo — per-user
   (в `~/.cargo`, `~/.rustup`); клон и `cargo build` идут под обычным логин-юзером.
   Только финальные шаги агента (копирование бинаря в `/usr/local/bin`,
   `pi agent setup`, `systemctl`) вызывают `sudo`. Совпадает с моделью §9/§13
   базовой спеки (логин-юзер ≠ сервис-юзер `pi-agent`).
4. **Канон установки бинаря агента — `/usr/local/bin/pi`** (это `ExecStart`
   systemd-юнита, §9 v0.5). Клиент ставится через `cargo install --path`
   (в `~/.cargo/bin`, который rustup добавляет в PATH).
5. **Пререквизиты:** `install` ставит Rust-тулчейн (rustup) и build-зависимости
   (на Debian-like — `apt-get`); **Docker — детект + инструкция, не авто** (§7).
   `update` ничего не провиженит.
6. **Стандартная папка клона:** Linux/macOS — `${XDG_DATA_HOME:-$HOME/.local/share}/pi/src`;
   Windows — `%LOCALAPPDATA%\pi\src`. Per-user, без sudo на создание. Маркер
   роли `.pi-role` в корне клона запоминает `agent`/`client` для `update`.
7. **Идемпотентность.** `install`/`update` повторяемы: клон есть → `git pull`;
   `pi agent setup` идемпотентен (§4 v0.5); рестарт демона — только если активен.
8. **`--dry-run`** в каждом скрипте печатает разрешённый план (детект ОС, папка,
   шаги) и выходит, ничего не делая. Консистентно с `pi agent setup --dry-run`.

## 4. `scripts/install.sh` — поведение

Запуск:

```sh
curl -fsSL https://raw.githubusercontent.com/khmilevoi/pi-deploy/master/scripts/install.sh \
  | sh -s -- --agent          # на Пайке
# или
curl -fsSL https://raw.githubusercontent.com/khmilevoi/pi-deploy/master/scripts/install.sh \
  | sh -s -- --client         # dev-клиент на Linux/macOS
```

Аргументы: `--agent` | `--client` (ровно один, обязателен), `--dir <path>`
(override папки клона), `--ref <branch>` (ветка/таг, дефолт `master`),
`--dry-run`.

Шаги:

1. **Парсинг + валидация:** требуется ровно один из `--agent`/`--client`.
2. **Детект:** `uname -s` → Linux/Darwin; арка `uname -m`; дистрибутив
   (`command -v apt-get`); systemd (`command -v systemctl`). `--agent` без
   Linux+systemd — ошибка с подсказкой.
3. **Пререквизиты (provisioning):**
   - `git` нет → Debian-like: `sudo apt-get update && sudo apt-get install -y git`;
     иначе инструкция + выход.
   - (Linux) `cc`/`pkg-config` нет → Debian-like:
     `sudo apt-get install -y build-essential pkg-config`; иначе инструкция.
   - `cargo` нет → rustup:
     `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y`,
     затем `. "$HOME/.cargo/env"`.
   - **Docker (только `--agent`)** нет → печать точных команд
     (`curl -fsSL https://get.docker.com | sh`, `sudo usermod -aG docker pi-agent`)
     и **продолжение** (не падаем); `pi agent setup`/`pi doctor` это подсветят (§7).
4. **Клон/пул:** если `$DIR/.git` есть → `git -C "$DIR" pull --ff-only`; иначе
   `git clone --branch "$REF" https://github.com/khmilevoi/pi-deploy.git "$DIR"`.
   Записать `$DIR/.pi-role` = `agent`/`client`.
5. **Сборка:** `cargo build --release` в `$DIR`.
6. **Установка по роли:**
   - `--agent`: `sudo install -m 755 "$DIR/target/release/pi" /usr/local/bin/pi`
     → `sudo /usr/local/bin/pi agent setup` (идемпотентно создаёт юзера/каталоги/
     юнит и `enable --now pi-agent`, §4 v0.5).
   - `--client`: `cargo install --path "$DIR/crates/bin" --locked`.
7. **Next-steps:** `--agent` → `pi doctor`, напоминание про новый SSH-сеанс
   (членство в группе `pi-agent`); `--client` → `pi setup` / `pi init`.

## 5. `scripts/update.sh` — поведение

Запуск из стандартной папки клона (или `--dir`); роль читается из `.pi-role`,
override флагом `--agent`/`--client`. Аргументы: `--dir`, `--agent`/`--client`,
`--dry-run`.

1. Разрешить папку и роль (флаг > `.pi-role` > ошибка, если не определить).
2. `git -C "$DIR" pull --ff-only`.
3. `cargo build --release`.
4. Установка:
   - `--agent`: `sudo install -m 755 .../pi /usr/local/bin/pi`
     → `sudo pi agent setup` (починить конфиг)
     → если `systemctl is-active --quiet pi-agent` — `sudo systemctl restart pi-agent`
     (подхватить новый бинарь; рестарт ловится свипом, §8.1 базовой спеки).
   - `--client`: `cargo install --path "$DIR/crates/bin" --locked --force`.
5. Провижининга пререквизитов нет — это зона `install`.

`pi agent setup` делает `enable --now`, но **не** рестартит уже работающий юнит,
поэтому новый бинарь подхватывается явным `systemctl restart` в шаге 4.

## 6. `scripts/install.ps1` / `update.ps1` — Windows-клиент

Только роль **client** (агента на Windows нет, демонов нет).

```powershell
irm https://raw.githubusercontent.com/khmilevoi/pi-deploy/master/scripts/install.ps1 | iex
```

`install.ps1` (параметры `-Dir`, `-Ref`, `-DryRun`):

1. **Пререквизиты:** нет `git` → инструкция (`winget install Git.Git`) + выход;
   нет `cargo` → скачать `rustup-init.exe` (https://win.rustup.rs) и запустить `-y`,
   обновить PATH в текущей сессии.
2. **Клон/пул** в `%LOCALAPPDATA%\pi\src` (записать `.pi-role` = `client`).
3. `cargo install --path crates\bin --locked`.
4. Напоминание: бинарь в `%USERPROFILE%\.cargo\bin` (в PATH через rustup); при
   необходимости открыть новый PowerShell.

`update.ps1` (`-Dir`, `-DryRun`): `git pull --ff-only` →
`cargo install --path crates\bin --locked --force`.

## 7. Политика Docker (детект + инструкция)

`install.sh --agent` **не** ставит Docker автоматически (не пайпает сторонний
`get.docker.com` от root без спроса). При отсутствии `docker`:

- печатает точные команды для Debian-like
  (`curl -fsSL https://get.docker.com | sh` + `sudo usermod -aG docker pi-agent`);
- **продолжает** установку (build-тулчейн + сборка + `pi agent setup`);
- `pi agent setup` уже проверяет наличие docker и предупреждает (§4 v0.5),
  `pi doctor` диагностирует — поэтому пропуск Docker не маскируется молча.

Build-тулчейн (rustup + `build-essential`/`pkg-config`) ставится автоматически —
он нужен прямо сейчас для `cargo build` и неинвазивен (rustup per-user, apt-deps
дёшевы).

## 8. Архитектура и расположение

- Новая папка `scripts/` в корне репозитория: `install.sh`, `update.sh`,
  `install.ps1`, `update.ps1`. Самодостаточные (никаких внешних зависимостей,
  кроме системных утилит).
- `install.sh`/`update.sh` — переиспользуют общую функцию установки (можно вынести
  внутр. хелпер-функции в одном файле каждый; общий код дублируется минимально —
  скрипты короткие, без подключаемых библиотек, чтобы оставаться `curl`-able).
- Rust-код **не меняется** по логике: только bump версии workspace до `0.6.0`
  (`Cargo.toml` `[workspace.package] version`). Сами скрипты вызывают уже
  существующие `pi agent setup` / `cargo` — новых команд/эндпоинтов нет.

## 9. Стандартные пути и маркер роли

| Платформа | Папка клона | Установка бинаря |
|---|---|---|
| Linux/macOS, `--agent` | `${XDG_DATA_HOME:-$HOME/.local/share}/pi/src` | `/usr/local/bin/pi` (sudo) |
| Linux/macOS, `--client` | та же | `~/.cargo/bin/pi` (`cargo install`) |
| Windows, client | `%LOCALAPPDATA%\pi\src` | `%USERPROFILE%\.cargo\bin\pi` |

`.pi-role` в корне клона хранит `agent`/`client`; `update.*` читают его, флаг
переопределяет.

## 10. Тестирование

Скрипты (bash/PowerShell) сложно юнит-тестировать как Rust-код. Уровень проверки:

- **`--dry-run`/`-DryRun`** — основной автоматизируемый smoke: печатает
  разрешённый план (роль, ОС, папка, последовательность шагов) и выходит `0`,
  ничего не выполняя. Должен работать без root и без Docker.
- **Линт:** `shellcheck scripts/*.sh`; `Invoke-ScriptAnalyzer scripts/*.ps1`.
- **Ручной acceptance** (документируется как чек-лист): `install.sh --agent` с
  нуля на Пайке → `pi doctor` зелёный; `update.sh` после нового коммита →
  `pi-agent` рестартнул и поднялся; `install.ps1` на Windows → `pi --help`.
- **Регрессия:** существующий `cargo test --workspace` проходит (bump версии
  ничего не ломает).

## 11. Сопутствующие правки

- **README:** новая секция «Install / update via scripts» **над** ручным путём;
  ручная сборка («Build And Install The Binary», `cross`) остаётся как fallback.
  Команды: `curl … | sh -s -- --agent`, `update.sh`, `irm … | iex`. Статус → v0.6.
- **Версия** workspace → `0.6.0`.
- Build-тайм оговорка: сборка из исходников на Пайке занимает несколько минут
  (rusqlite bundled, age, reqwest); «~10 минут» — ориентир, зависит от модели Pi.

## 12. Перенесено в v0.7+

| Пункт | Почему отложено | Ссылки |
|---|---|---|
| Пребилт-бинари + GitHub Releases + sha256 + кросс-CI | Решено собирать из исходников; отдельный большой кусок (CI + хостинг артефактов) | v0.5 §13 |
| `pi agent update` (drain/503-протокол) | `update.sh` закрывает практическую потребность пересборкой; полноценный drain требует протокольной работы в агенте | v0.5 §13, §8.1 базовой |
| Удалённый `pi agent setup` по SSH | Сложность sudo-over-ssh; `install.sh` — локальный bootstrap на Пайке | v0.5 §13, реш. 1 |

Остаётся за рамками (точки расширения §21 базовой спеки): `pi agent backup`/
`restore`, авто-регистрация deploy-key через GitHub API, авто-удаление DNS при
`pi rm`, webhook-receiver, мульти-Pi профили.

Версия по завершении: **0.6.0**.
