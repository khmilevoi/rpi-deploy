# pi v0.6 — Скрипты установки и обновления из исходников (дизайн)

Дата: 2026-06-18. Базовая спека: `2026-06-09-pi-deploy-tool-design.md`
(§13 установка/self-setup, §16 команды, §23 роадмап). Предшествующая версия:
v0.5 (`2026-06-17-pi-install-ux-v0.5-design.md`, §13 — список переноса).

**Критерий готовности:** есть две дорожки установки агента:

- **Safe default**: `curl … | sh -s -- --agent` ставит build-пререквизиты,
  клонирует репо, собирает из исходников и устанавливает бинарь в
  `/usr/local/bin/pi`. Если Docker отсутствует, скрипт **останавливается до**
  `pi agent setup`, печатает команды установки Docker и команду повторного
  запуска, выходит с кодом `2`.
- **Full bootstrap**: `curl … | sh -s -- --agent --install-docker` на
  Debian-like Pi дополнительно ставит Docker, затем запускает
  `sudo /usr/local/bin/pi agent setup` и поднимает/чинит агент.

Windows-клиент ставится одной командой (`irm … | iex`), повторное обновление —
отдельной командой (`update`), которая пулит branch-клон и пересобирает.

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
  установка. Роль — флаг `--agent` / `--client`; полный Docker-bootstrap агента
  — opt-in флаг `--install-docker` (§4, §7).
- **`scripts/update.sh`** (Linux/macOS): из branch-клона `git pull` → сборка →
  переустановка (+ рестарт демона для `--agent`) (§5).
- **`scripts/install.ps1`** (PowerShell, Windows, запускается `irm … | iex`):
  bootstrap клиента (§6).
- **`scripts/update.ps1`** (Windows): обновление клиента из branch-клона (§6).
- **Сопутствующее:** `--dry-run`/`-DryRun` в каждом скрипте; exit-code contract
  (§10); линт (`shellcheck`/`PSScriptAnalyzer`); правки README; bump версии до
  `0.6.0` (§9–§12).

НЕ входит (→ v0.7+, §13):

- Пребилт-бинари + публикация в GitHub Releases + sha256 + кросс-CI.
- `pi agent update` как команда агента с draining/503-протоколом.
- Удалённый `pi agent setup` по SSH с клиента (sudo-over-ssh).
- Изменение логики Rust-кода `pi agent setup`; v0.6 меняет только скрипты,
  README/spec и версию workspace.

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
   systemd-юнита, §9 v0.5). Все вызовы agent setup из скриптов используют
   абсолютный путь: `sudo /usr/local/bin/pi agent setup`. Клиент ставится через
   `cargo install --path … --locked --force`.
5. **Docker — safe default + opt-in full bootstrap.** Без `--install-docker`
   скрипт не ставит Docker автоматически. Если Docker отсутствует, safe default
   устанавливает бинарь и выходит с кодом `2` до `pi agent setup`, потому что
   текущий Rust `pi agent setup` может упасть на отсутствии группы `docker`.
   `--install-docker` разрешает инвазивную установку Docker на Debian-like Pi.
6. **Стандартная папка клона:** Linux/macOS — `${XDG_DATA_HOME:-$HOME/.local/share}/pi/src`;
   Windows — `%LOCALAPPDATA%\pi\src`. Per-user, без sudo на создание. Маркер
   роли `.pi-role` в корне клона запоминает `agent`/`client` для `update`;
   скрипты добавляют `.pi-role` в `$DIR/.git/info/exclude`, чтобы собственный
   маркер не делал клон dirty.
7. **`--ref` — branch или tag, без commit SHA.** `install.*` умеет поставить
   branch/tag. Tagged install считается pinned; `update.*` работает только на
   branch checkout и для detached/tag выходит с понятной ошибкой.
8. **Идемпотентность без потери данных.** `install`/`update` повторяемы только на
   clean worktree. При staged/unstaged/untracked изменениях скрипт выходит с
   ошибкой и ничего не сбрасывает. `reset --hard` не используется.
9. **`--dry-run`** в каждом скрипте печатает разрешённый план (роль, ОС, ref,
   папка, шаги, exit-code outcomes) и выходит, ничего не делая. Консистентно с
   `pi agent setup --dry-run`.

## 4. `scripts/install.sh` — поведение

Запуск:

```sh
curl -fsSL https://raw.githubusercontent.com/khmilevoi/pi-deploy/master/scripts/install.sh \
  | sh -s -- --agent                  # safe default на Пайке

curl -fsSL https://raw.githubusercontent.com/khmilevoi/pi-deploy/master/scripts/install.sh \
  | sh -s -- --agent --install-docker # full bootstrap на Debian-like Pi

curl -fsSL https://raw.githubusercontent.com/khmilevoi/pi-deploy/master/scripts/install.sh \
  | sh -s -- --client                 # dev-клиент на Linux/macOS
```

Аргументы: `--agent` | `--client` (ровно один, обязателен), `--dir <path>`
(override папки клона), `--ref <branch-or-tag>` (дефолт `master`),
`--install-docker` (только с `--agent`), `--dry-run`.

Шаги:

1. **Парсинг + валидация:** требуется ровно один из `--agent`/`--client`.
   `--install-docker` допустим только вместе с `--agent`.
2. **Детект:** `uname -s` → Linux/Darwin; арка `uname -m`; дистрибутив
   (`command -v apt-get`); systemd (`command -v systemctl`). `--agent` без
   Linux+systemd — ошибка с подсказкой.
3. **Пререквизиты (provisioning):**
   - `git` нет → Debian-like: `sudo apt-get update && sudo apt-get install -y git`;
     иначе инструкция + выход `1`.
   - (Linux) `cc`/`pkg-config` нет → Debian-like:
     `sudo apt-get install -y build-essential pkg-config`; иначе инструкция + выход `1`.
   - `cargo` нет → rustup:
     `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y`,
     затем `. "$HOME/.cargo/env"`.
4. **Клон/ref:**
   - если `$DIR` отсутствует → `git clone --branch "$REF" https://github.com/khmilevoi/pi-deploy.git "$DIR"`;
   - если `$DIR/.git` есть → сначала убедиться, что `.pi-role` добавлен в
     `$DIR/.git/info/exclude`, затем проверить clean worktree через
     `git status --porcelain`;
     при любых локальных изменениях выйти `1` с подсказкой commit/stash/remove или
     использовать другой `--dir`;
   - для существующего клона → `git fetch --tags origin`, затем:
     - branch: checkout branch, настроить tracking к `origin/<ref>` только если
       локальной branch ещё нет, затем `git pull --ff-only`;
     - tag: checkout tag в detached state;
     - ref не найден как branch/tag → ошибка `1`.
   - записать `$DIR/.pi-role` = `agent`/`client`.
5. **Сборка:** `cargo build --release` в `$DIR`.
6. **Установка по роли:**
   - `--agent`: `sudo install -m 755 "$DIR/target/release/pi" /usr/local/bin/pi`.
     После установки бинаря:
     - если Docker есть → `sudo /usr/local/bin/pi agent setup`;
     - если Docker отсутствует и `--install-docker` не задан → напечатать
       safe-stop инструкции и выйти `2` **до** `pi agent setup`;
     - если Docker отсутствует и `--install-docker` задан → на Debian-like выполнить
       `curl -fsSL https://get.docker.com | sh`, затем
       `sudo /usr/local/bin/pi agent setup`; на неподдержанной ОС — инструкция + выход `1`.
   - `--client`: `cargo install --path "$DIR/crates/bin" --locked --force`.
7. **Next-steps:** `--agent` → `pi doctor`, напоминание про новый SSH-сеанс
   (членство в группе `pi-agent`); `--client` → `pi setup` / `pi init`.

Safe-stop сообщение при отсутствии Docker должно быть конкретным:

```text
Docker is required before pi agent setup can run.
The pi binary is installed at /usr/local/bin/pi.

Install Docker manually:
  curl -fsSL https://get.docker.com | sh

Then rerun:
  curl -fsSL https://raw.githubusercontent.com/khmilevoi/pi-deploy/master/scripts/install.sh | sh -s -- --agent

Or allow the installer to do it:
  curl -fsSL https://raw.githubusercontent.com/khmilevoi/pi-deploy/master/scripts/install.sh | sh -s -- --agent --install-docker
```

## 5. `scripts/update.sh` — поведение

Запуск из стандартной папки клона (или `--dir`); роль читается из `.pi-role`,
override флагом `--agent`/`--client`. Аргументы: `--dir`, `--agent`/`--client`,
`--dry-run`.

1. Разрешить папку и роль (флаг > `.pi-role` > ошибка, если не определить).
2. Убедиться, что `.pi-role` добавлен в `$DIR/.git/info/exclude`, затем проверить
   clean worktree (`git status --porcelain` пустой). При локальных изменениях —
   ошибка `1`, без `reset --hard`.
3. Проверить, что checkout находится на branch. Detached/tag install считается
   pinned: `update.sh` выходит `1` с сообщением
   `This install is pinned; rerun install with --ref <new-tag>`.
4. `git -C "$DIR" pull --ff-only`.
5. `cargo build --release`.
6. Установка:
   - `--agent`: `sudo install -m 755 "$DIR/target/release/pi" /usr/local/bin/pi`
     → `sudo /usr/local/bin/pi agent setup` (починить конфиг)
     → если `systemctl is-active --quiet pi-agent` — `sudo systemctl restart pi-agent`
     (подхватить новый бинарь; рестарт ловится свипом, §8.1 базовой спеки).
   - `--client`: `cargo install --path "$DIR/crates/bin" --locked --force`.
7. Провижининга пререквизитов нет — это зона `install`.

`pi agent setup` делает `enable --now`, но **не** рестартит уже работающий юнит,
поэтому новый бинарь подхватывается явным `systemctl restart` в шаге 6.

## 6. `scripts/install.ps1` / `update.ps1` — Windows-клиент

Только роль **client** (агента на Windows нет, демонов нет).

Default one-liner:

```powershell
irm https://raw.githubusercontent.com/khmilevoi/pi-deploy/master/scripts/install.ps1 | iex
```

Параметризованный one-liner:

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/khmilevoi/pi-deploy/master/scripts/install.ps1))) -DryRun
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/khmilevoi/pi-deploy/master/scripts/install.ps1))) -Ref v0.6.0
```

`install.ps1` (параметры `-Dir`, `-Ref`, `-DryRun`; default `-Ref master`):

1. **Пререквизиты:** нет `git` → инструкция (`winget install Git.Git`) + выход
   `1`; нет `cargo` → скачать `rustup-init.exe` (https://win.rustup.rs) и
   запустить `-y`, обновить PATH в текущей сессии.
2. **Клон/ref** в `%LOCALAPPDATA%\pi\src`:
   - новый клон → `git clone --branch <Ref> ...`;
   - существующий клон → добавить `.pi-role` в `.git/info/exclude`,
     clean-worktree check, `git fetch --tags origin`, checkout branch/tag по тем
     же правилам, что `install.sh`;
   - записать `.pi-role` = `client`.
3. `cargo install --path crates\bin --locked --force`.
4. Напоминание: бинарь в `%USERPROFILE%\.cargo\bin` (в PATH через rustup); при
   необходимости открыть новый PowerShell.

`update.ps1` (`-Dir`, `-DryRun`): ensure `.pi-role` is excluded →
clean-worktree check → reject detached/tag checkout → `git pull --ff-only` →
`cargo install --path crates\bin --locked --force`.

## 7. Политика Docker

`install.sh --agent` **не** ставит Docker автоматически (не пайпает сторонний
`get.docker.com` от root без явного флага). При отсутствии `docker`:

- без `--install-docker`: бинарь устанавливается, `pi agent setup` не запускается,
  скрипт печатает команды Docker + rerun и выходит `2`;
- с `--install-docker`: на Debian-like выполнить
  `curl -fsSL https://get.docker.com | sh`, затем продолжить к
  `sudo /usr/local/bin/pi agent setup`;
- на не-Debian-like с `--install-docker`: не пытаться угадать пакетный менеджер,
  напечатать ручную инструкцию и выйти `1`.

Причина safe-stop: в v0.6 Rust-код `pi agent setup` не меняется. В текущем v0.5
`setup` добавляет `pi-agent` в группу `docker`; если группы нет, это hard error.
Поэтому safe default обязан остановиться до `setup`, а не маскировать ошибку.

Build-тулчейн (rustup + `build-essential`/`pkg-config`) ставится автоматически —
он нужен прямо сейчас для `cargo build` и неинвазивен (rustup per-user, apt-deps
дёшевы).

## 8. Примеры поведения

**Чистая Pi, safe default, Docker отсутствует:**

```sh
curl -fsSL .../scripts/install.sh | sh -s -- --agent
```

Скрипт ставит build deps/Rust, клонирует, собирает, устанавливает
`/usr/local/bin/pi`, затем выходит `2` до `pi agent setup` и печатает Docker +
rerun инструкции.

**Чистая Pi, full bootstrap:**

```sh
curl -fsSL .../scripts/install.sh | sh -s -- --agent --install-docker
```

Скрипт ставит build deps/Rust/Docker, собирает, устанавливает бинарь, запускает
`sudo /usr/local/bin/pi agent setup`, затем предлагает `pi doctor`.

**Safe default после ручной установки Docker:**

```sh
curl -fsSL .../scripts/install.sh | sh -s -- --agent
```

Скрипт проверяет clean branch-клон, делает `git pull --ff-only`, пересобирает,
переустанавливает `/usr/local/bin/pi` и запускает `sudo /usr/local/bin/pi agent setup`.

**Windows client default:**

```powershell
irm .../scripts/install.ps1 | iex
```

Скрипт ставит/подхватывает rustup, клонирует и выполняет
`cargo install --path crates\bin --locked --force`.

**Windows client dry-run/ref:**

```powershell
& ([scriptblock]::Create((irm .../scripts/install.ps1))) -DryRun
& ([scriptblock]::Create((irm .../scripts/install.ps1))) -Ref v0.6.0
```

**Pinned tag install:**

```sh
curl -fsSL .../scripts/install.sh | sh -s -- --client --ref v0.6.0
```

Скрипт checkout-ит tag в detached state. `update.*` на таком клоне выходит с
сообщением, что install pinned; обновление делается повторным
`install --ref <new-tag>`.

**Локальные изменения в клоне:**

```text
error: install directory has local changes.
Commit/stash/remove them, or choose another --dir.
```

Скрипт не делает `reset --hard` и не удаляет файлы.

## 9. Архитектура и расположение

- Новая папка `scripts/` в корне репозитория: `install.sh`, `update.sh`,
  `install.ps1`, `update.ps1`. Самодостаточные (никаких внешних зависимостей,
  кроме системных утилит).
- `.sh` скрипты — POSIX `sh`, без bash-специфики.
- `install.sh`/`update.sh` — переиспользуют общие идеи, но не source-ят общий
  файл: каждый скрипт должен оставаться самодостаточным для `curl`/запуска из
  клона. Небольшое дублирование helper-функций допустимо.
- `.ps1` скрипты — обычные PowerShell-скрипты с `param(...)`; параметры для
  remote one-liner документируются через `scriptblock`.
- Rust-код **не меняется** по логике: только bump версии workspace до `0.6.0`
  (`Cargo.toml` `[workspace.package] version`). Сами скрипты вызывают уже
  существующие `pi agent setup` / `cargo` — новых команд/эндпоинтов нет.

## 10. Стандартные пути, маркеры и exit codes

| Платформа | Папка клона | Установка бинаря |
|---|---|---|
| Linux/macOS, `--agent` | `${XDG_DATA_HOME:-$HOME/.local/share}/pi/src` | `/usr/local/bin/pi` (sudo) |
| Linux/macOS, `--client` | та же | `~/.cargo/bin/pi` (`cargo install --force`) |
| Windows, client | `%LOCALAPPDATA%\pi\src` | `%USERPROFILE%\.cargo\bin\pi` |

`.pi-role` в корне клона хранит `agent`/`client`; `update.*` читают его, флаг
переопределяет. Скрипты должны добавлять `.pi-role` в `$DIR/.git/info/exclude`
до clean-worktree checks; остальные untracked файлы остаются ошибкой.

Exit codes:

| Код | Значение |
|---|---|
| `0` | install/update полностью выполнен или dry-run успешно напечатал план |
| `2` | expected safe stop: `install.sh --agent` обнаружил отсутствие Docker, бинарь установлен, `pi agent setup` не запускался |
| `1` | ошибка: unknown ref, dirty worktree, missing non-auto prerequisite, cargo/git failure, non-ff update, unsupported platform, pinned tag update, etc. |

## 11. Тестирование

Скрипты (bash/PowerShell) сложно юнит-тестировать как Rust-код. Уровень проверки:

- **`--dry-run`/`-DryRun` smoke:** печатает разрешённый план (роль, ОС, ref,
  папка, последовательность шагов, exit-code outcomes) и выходит `0`, ничего не
  выполняя. Должен работать без root и без Docker.
- **Линт:** `shellcheck scripts/*.sh`; `Invoke-ScriptAnalyzer scripts/*.ps1`.
- **Ручная acceptance matrix:**
  - `install.sh --agent --dry-run` без root/Docker → exit `0`, только план.
  - `install.sh --agent` без Docker → `/usr/local/bin/pi` установлен, safe stop
    before `pi agent setup`, exit `2`, инструкция Docker + rerun.
  - `install.sh --agent --install-docker` на Debian-like Pi → Docker + binary +
    `pi agent setup`; `pi doctor` без Docker-related FAIL.
  - `install.sh --client` повторно → успех через `cargo install --force`.
  - `install.sh --client` на macOS без build tools → понятная инструкция
    поставить Xcode Command Line Tools / compiler toolchain, exit `1`.
  - `install.ps1` повторно → успех через `cargo install --force`.
  - `update.*` на branch → pull/build/install.
  - `update.*` на tag/detached → clear pinned-install error, exit `1`.
  - dirty worktree → clear error, exit `1`, no checkout/reset.
- **Регрессия:** существующий `cargo test --workspace` проходит (bump версии
  ничего не ломает).

## 12. Сопутствующие правки

- **README:** новая секция «Install / update via scripts» **над** ручным путём;
  ручная сборка («Build And Install The Binary», `cross`) остаётся как fallback.
  Команды:
  - safe default agent: `curl … | sh -s -- --agent`;
  - full bootstrap agent: `curl … | sh -s -- --agent --install-docker`;
  - update: `scripts/update.sh`;
  - Windows default: `irm … | iex`;
  - Windows параметризованный запуск: `& ([scriptblock]::Create((irm …))) -DryRun`.
  Статус → v0.6.
- **Версия** workspace → `0.6.0`.
- Build-тайм оговорка: сборка из исходников на Пайке занимает несколько минут
  (rusqlite bundled, age, reqwest); «~10 минут» — ориентир, зависит от модели Pi.

## 13. Перенесено в v0.7+

| Пункт | Почему отложено | Ссылки |
|---|---|---|
| Пребилт-бинари + GitHub Releases + sha256 + кросс-CI | Решено собирать из исходников; отдельный большой кусок (CI + хостинг артефактов) | v0.5 §13 |
| `pi agent update` (drain/503-протокол) | `update.sh` закрывает практическую потребность пересборкой; полноценный drain требует протокольной работы в агенте | v0.5 §13, §8.1 базовой |
| Удалённый `pi agent setup` по SSH | Сложность sudo-over-ssh; `install.sh` — локальный bootstrap на Пайке | v0.5 §13, реш. 1 |
| Изменение Rust `pi agent setup` под warning-only Docker dependency checks | v0.6 сознательно не меняет Rust-логику; safe default останавливается до `setup`, full bootstrap ставит Docker заранее | review fixes |

Остаётся за рамками (точки расширения §21 базовой спеки): `pi agent backup`/
`restore`, авто-регистрация deploy-key через GitHub API, авто-удаление DNS при
`pi rm`, webhook-receiver, мульти-Pi профили.

Версия по завершении: **0.6.0**.
