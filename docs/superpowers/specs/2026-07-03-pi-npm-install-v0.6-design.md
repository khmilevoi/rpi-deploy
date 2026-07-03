# pi v0.6 — Установка через npm, команда `rpi` (дизайн)

Дата: 2026-07-03. **Заменяет** `2026-06-18-pi-install-scripts-v0.6-design.md`
(curl|sh-скрипты сборки; подход отменён, см. §1). Базовая спека:
`2026-06-09-pi-deploy-tool-design.md` (§13 установка/self-setup, §16 команды,
§23 роадмап). Предшествующая версия: v0.5
(`2026-06-17-pi-install-ux-v0.5-design.md`).

**Критерий готовности:** установка и обновление — через npm; команда — `rpi`:

```sh
npm install -g rpi-deploy          # клиент на dev-машине (Linux/macOS/Windows)
sudo npm install -g rpi-deploy     # агент на Пайке (Node >= 18)
sudo rpi agent setup               # затем как обычно
```

Обновление: `npm install -g rpi-deploy@latest` (на Пайке с `sudo`, затем
`sudo rpi agent setup` — он подхватывает новый бинарь и рестартит демон).

## 1. Контекст и смена подхода

v0.6 менял русло дважды. Исходный план — пребилты + GitHub Releases; затем —
curl|sh-скрипты сборки из исходников с интерактивным выбором роли; теперь —
**один канал `npm install -g`** для обеих ролей. Мотив: npm даёт привычные
install/update/uninstall и версионирование из коробки, без поддержки
собственных скриптов.

Философия «сборка из исходников, без пребилтов, без CI и без GitHub Releases»
**сохраняется**: npm-тарбол содержит исходники, `postinstall` собирает их на
машине пользователя. Кросс-компиляция и хостинг артефактов по-прежнему не
нужны.

Вместе с переездом на npm **CLI-команда переименовывается `pi` → `rpi`**
(`rpi deploy`, `sudo rpi agent setup`, …) — в пару к имени пакета
`rpi-deploy` и имени репозитория. Границы переименования — см. §3.2.

Скрипты `install.sh`/`update.sh`/`install.ps1`/`update.ps1` не создаются;
план `2026-07-02-pi-install-scripts-v0.6.md` отменён и удалён. Вместе со
скриптами умерли: интерактивный роль-промпт, маркер `.pi-role`, exit-code
контракт установщика. Роль теперь определяется использованием после
установки (`sudo rpi agent setup` против `rpi setup`), а не при установке.

## 2. Скоуп

Входит:

- **npm-пакет `rpi-deploy`** в корне репозитория: `package.json`,
  Node-шим `bin/rpi.js`, `scripts/postinstall.js` (§4–§6).
- **Переименование команды `pi` → `rpi`** (§3.2): `[[bin]] name`,
  clap-имя, `ExecStart` юнита, канонический путь `/usr/local/bin/rpi`,
  user-facing строки (подсказки/сообщения), README и доки skills.
- **Правка Rust:** `rpi agent setup` получает self-install шаг — копирует
  собственный бинарь в `/usr/local/bin/rpi` и рестартит активный `pi-agent`
  при смене бинаря (§7); улучшенное сообщение об отсутствии Docker.
- **Сопутствующее:** README (секция «Install via npm» + свип команд), bump
  версии workspace и пакета до `0.6.0`, LICENSE (MIT), публикация вручную
  `npm publish` (§9, §11).

НЕ входит:

- Пребилт-бинари, платформ-пакеты (`optionalDependencies`), GitHub Releases,
  sha256, кросс-CI — не нужны при сборке из исходников (в v0.7+ только если
  время сборки на Пайке станет неприемлемым).
- curl|sh-скрипты — отменены, не переносятся.
- Переименование внутренних имён: юзер/группа/юнит `pi-agent`, `/etc/pi`,
  `/var/lib/pi`, `/var/log/pi`, `/run/pi`, `pi.toml` — **остаются как есть**
  (§3.2).
- `pi agent update` как команда с drain/503-протоколом (→ v0.7+, §12).
- Удалённый agent setup по SSH (→ v0.7+).

## 3. Ключевые решения

1. **Имя пакета `rpi-deploy`** (имя `pi` на npm занято), команда — `rpi`
   (`bin: {"rpi": "bin/rpi.js"}`). На npm существует чужой пакет `rpi`
   (v0.0.3) — имя *пакета* нам не нужно, а коллизия *bin-имени* возможна,
   только если пользователь поставит оба глобально; осознанный риск.
2. **Переименование — только команда.** Бинарь/CLI — `rpi`; канон агента —
   `/usr/local/bin/rpi`; все подсказки и доки говорят `rpi …`. Внутренние
   идентификаторы НЕ меняются: юзер/группа/юнит `pi-agent`, пути `/etc/pi`,
   `/var/lib/pi`, `/var/log/pi`, `/run/pi`, проектный `pi.toml`, имена
   Rust-крейтов (`pi`, `pi-domain`, …). Существующая v0.5-Пайка мигрирует
   штатно: `ExecStart` в каноничном юните изменился → `write_unit_with_backup`
   сделает `.bak` и перепишет юнит; старый `/usr/local/bin/pi` остаётся на
   диске (удалить вручную: `sudo rm /usr/local/bin/pi`), README упоминает это.
3. **Исходники в тарболе.** `files` в package.json: `crates/`, `Cargo.toml`,
   `Cargo.lock`, `bin/`, `scripts/postinstall.js`, `scripts/check-version.js`.
   Ни `target/`, ни доки, ни плагины в тарбол не попадают. git на машине
   пользователя не нужен вообще.
4. **`postinstall` собирает из исходников:** `cargo build --release --locked`,
   бинарь копируется в `dist/rpi` (`dist/rpi.exe`), затем `target/`
   **удаляется** — иначе на SD-карте Пайки в `node_modules` осталось бы
   ~1–2 ГБ. Цена: каждое обновление пересобирает с нуля (кэш реестра cargo в
   `~/.cargo` остаётся, поэтому зависимости не перекачиваются).
5. **Node-шим `bin/rpi.js`** запускает `dist/rpi` с пробросом аргументов,
   stdio и кода выхода. `engines.node >= 18`. Шим — постоянная точка входа
   (никакой подмены файла бинарём: на Windows npm всё равно генерирует
   `.cmd`-обёртки).
6. **Тулчейн:** нет `cargo` → postinstall сам ставит rustup (`-y`, per-user;
   при `sudo npm i -g` на Пайке — для root в `/root/.cargo`, это осознанный
   компромисс). Нет `cc`/`pkg-config` на Linux → понятная инструкция
   (`sudo apt-get install -y build-essential pkg-config`) и провал установки;
   postinstall **не** запускает apt сам. На Windows отдельной проверки MSVC
   нет — при падении cargo печатается подсказка про Visual Studio Build Tools
   (C++ workload). На macOS без `cc` — подсказка про `xcode-select --install`.
7. **Docker не проверяется при установке** — пакет один для обеих ролей,
   установка роль-агностична. Требование Docker остаётся зоной
   `rpi agent setup`; его сообщение об ошибке дополняется командой
   `curl -fsSL https://get.docker.com | sh`.
8. **Канон бинаря агента — `/usr/local/bin/rpi`** (`ExecStart` юнита меняется
   на `/usr/local/bin/rpi agent run --config /etc/pi/agent.toml`). npm кладёт
   пакет в `node_modules`, путь нестабилен, поэтому `rpi agent setup` копирует
   **собственный** бинарь (`std::env::current_exe()`) в `/usr/local/bin/rpi`
   (§7).
9. **Обновление = `npm install -g rpi-deploy@latest`.** На Пайке затем
   `sudo rpi agent setup`: он кладёт новый бинарь и рестартит активный юнит.
   Рестарт ловится существующим свипом `running→interrupted` (§8.1 базовой
   спеки) — drain-протокол не нужен.
10. **Публикация вручную:** `npm publish` из корня репо. `prepublishOnly`
    проверяет, что версия `package.json` совпадает с версией workspace в
    `Cargo.toml` (простой Node-скрипт, падает при расхождении).
11. **Удаление:** клиент — `npm uninstall -g rpi-deploy`; агент — сначала
    `sudo rpi agent uninstall` (v0.5), затем `sudo npm uninstall -g rpi-deploy`.
    `rpi agent uninstall` бинарь в `/usr/local/bin` не трогает (как и в v0.5).

## 4. `package.json`

```json
{
  "name": "rpi-deploy",
  "version": "0.6.0",
  "description": "Deployment tool for Docker Compose projects on Raspberry Pi. Builds the Rust CLI from source on install.",
  "license": "MIT",
  "repository": {
    "type": "git",
    "url": "git+https://github.com/khmilevoi/rpi-deploy.git"
  },
  "bin": { "rpi": "bin/rpi.js" },
  "files": [
    "bin/",
    "scripts/postinstall.js",
    "scripts/check-version.js",
    "crates/",
    "Cargo.toml",
    "Cargo.lock"
  ],
  "scripts": {
    "postinstall": "node scripts/postinstall.js",
    "prepublishOnly": "node scripts/check-version.js"
  },
  "engines": { "node": ">=18" },
  "os": ["linux", "darwin", "win32"]
}
```

`check-version.js` нужен только при публикации, но включается в `files` рядом
с `postinstall.js` (пара сотен байт), чтобы `prepublishOnly` работал и из
распакованного тарбола. В корне репозитория добавляется файл `LICENSE` (MIT) —
публикация в npm делает код публичным, и поле `license` должно ссылаться на
реальный файл; npm включает `LICENSE`, `README.md` и `package.json` в тарбол
автоматически.

## 5. `scripts/postinstall.js` — поведение

Шаги (Node ≥ 18, без внешних зависимостей):

1. **Резолв cargo.** Кандидаты: `cargo` в `PATH`, затем
   `$HOME/.cargo/bin/cargo` (`%USERPROFILE%\.cargo\bin\cargo.exe`). Если не
   найден — **автоустановка rustup**:
   - Linux/macOS: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y`
     (через `sh -c`); требуется `curl`, при его отсутствии — инструкция + провал;
   - Windows: скачать `https://win.rustup.rs/x86_64` (или `aarch64` по
     `process.arch`) во временный файл через `fetch`, запустить с `-y`.
   После установки cargo берётся из `~/.cargo/bin` (путь резолвится явно).
2. **Проверка C-тулчейна (Linux/macOS):** нет `cc` или (Linux) `pkg-config` →
   напечатать команду установки (`sudo apt-get install -y build-essential
   pkg-config` на Debian-like; `xcode-select --install` на macOS) и выйти с
   ненулевым кодом (npm пометит установку проваленной).
3. **Сборка:** `cargo build --release --locked` в корне пакета
   (`process.cwd()` = каталог пакета при postinstall), stdout/stderr —
   inherit, чтобы пользователь видел прогресс. Провал сборки на Windows
   дополняется подсказкой про Visual Studio Build Tools (C++ workload).
4. **Установка бинаря в пакет:** скопировать
   `target/release/rpi[.exe]` → `dist/rpi[.exe]`, выставить `0o755`.
5. **Очистка:** рекурсивно удалить `target/` (см. §3.4).
6. Печать next-steps: клиент — `rpi setup` / `rpi init`; Пайка —
   `sudo rpi agent setup` (и что Docker должен быть установлен заранее).

Ошибки на каждом шаге — короткое `error: …` с конкретной командой для
исправления; никакого автозапуска apt/brew/winget.

Замечание про root: при `sudo npm install -g` на Пайке npm (≥7) выполняет
postinstall от root — rustup уходит в `/root/.cargo`, сборка идёт от root.
Это принятый компромисс единого канала установки (§3.6).

## 6. `bin/rpi.js` — шим

- Определяет путь `../dist/rpi` (`rpi.exe` на win32) относительно самого себя
  (`__dirname`), запускает через `child_process.spawnSync(bin, argv,
  {stdio: 'inherit'})`, транслирует код выхода (`process.exit(status)`);
  завершение по сигналу → выход с кодом `128+signal` (POSIX-конвенция).
- Если `dist/rpi` отсутствует (например, установка с `--ignore-scripts`) —
  сообщение «binary not built; reinstall without --ignore-scripts:
  npm install -g rpi-deploy» и выход `1`.
- Никакой логики ролей/обновлений в шиме нет.

## 7. Правка Rust: rename + self-install в `rpi agent setup`

**Rename (§3.2):** `[[bin]] name = "rpi"` в `crates/bin/Cargo.toml` (имя
*пакета* `pi` остаётся — оно внутреннее); clap `name = "rpi"` в `main.rs`;
`ExecStart=/usr/local/bin/rpi agent run --config /etc/pi/agent.toml` в
каноне юнита; все user-facing строки (подсказки `run \`pi gc\``, сообщения
`sudo pi agent setup`, и т.п.) и rustdoc-упоминания команд — на `rpi …`.

**Self-install** — новый шаг в начале установочной последовательности setup
(до записи юнита и `enable --now`):

1. `current = std::env::current_exe()?` (это реальный ELF: шим запускает
   бинарь дочерним процессом, так что `current_exe` внутри процесса —
   `dist/rpi`, не Node).
2. Если `current` уже `/usr/local/bin/rpi` — шаг пропускается (ручная
   установка из исходников продолжает работать как раньше).
3. Иначе сравнить с `/usr/local/bin/rpi` побайтно (отсутствие файла считается
   отличием); при отличии — атомарно скопировать
   (`/usr/local/bin/.rpi.tmp` → rename), `chmod 755`.
4. Если бинарь заменён и `pi-agent` активен (`systemctl is-active`) —
   `systemctl restart pi-agent` в конце setup (после остальных шагов).
5. `--dry-run` печатает этот шаг как и остальные, не выполняя.

Сообщение об ошибке «нет группы docker / нет docker» в setup дополняется
строкой: `Install Docker first: curl -fsSL https://get.docker.com | sh`.

Юнит-тесты: путь-уже-канонический (шаг пропущен), бинарь отличается
(скопирован+флаг рестарта), бинарь совпадает (ничего). Логика сравнения и
копирования выносится в функцию, тестируемую на tempdir без root.

## 8. Примеры поведения

**Чистая Пайка (Node уже стоит, Docker стоит):**

```sh
sudo npm install -g rpi-deploy   # rustup для root при необходимости, сборка ~10 мин
sudo rpi agent setup             # копия бинаря в /usr/local/bin/rpi, юнит, старт
rpi doctor
```

**Чистая Пайка без Docker:** `npm install -g` успешен (Docker не нужен для
сборки); `sudo rpi agent setup` падает с сообщением, включающим
`curl -fsSL https://get.docker.com | sh`. После установки Docker — повторный
`sudo rpi agent setup`.

**Апгрейд существующей v0.5-Пайки:** `sudo npm install -g rpi-deploy` →
`sudo rpi agent setup` → юнит с новым `ExecStart` записывается с бэкапом
(`pi-agent.service.bak`), бинарь копируется в `/usr/local/bin/rpi`, демон
рестартится. Старый `/usr/local/bin/pi` остаётся; удалить вручную.

**Dev-машина (Linux/macOS/Windows):**

```sh
npm install -g rpi-deploy
rpi setup
rpi init
```

**Обновление на Пайке:**

```sh
sudo npm install -g rpi-deploy@latest   # пересборка
sudo rpi agent setup                    # новый бинарь + рестарт демона
```

**Linux без build-тулчейна:** postinstall падает с
`sudo apt-get install -y build-essential pkg-config`; после установки —
повторный `npm install -g rpi-deploy`.

**Установка с `--ignore-scripts`:** `rpi` печатает «binary not built…» и
выходит `1`.

## 9. Архитектура и расположение

```
package.json              # npm-манифест (§4)
bin/rpi.js                # Node-шим (§6)
scripts/postinstall.js    # сборка при установке (§5)
scripts/check-version.js  # prepublishOnly: версия пакета == версии workspace
dist/                     # создаётся postinstall'ом; в git не попадает (.gitignore)
crates/bin/Cargo.toml     # [[bin]] name = "rpi" (имя пакета pi остаётся)
crates/bin/src/main.rs    # clap name = "rpi"
crates/bin/src/agent/setup.rs  # + self-install шаг (§7), новый ExecStart
```

JS-файлы — без зависимостей и без сборки (plain Node, CommonJS). ESLint/CI
для них не заводится; проверка — `node --check` в тестах плана.

## 10. Тестирование

- **Юнит (Rust):** новый self-install шаг setup — тесты на tempdir (§7);
  обновлённые ассерты канона юнита (`ExecStart=/usr/local/bin/rpi …`);
  регрессия `cargo test --workspace`.
- **Смоук (Windows, локально):** `npm pack` → `npm install -g ./rpi-deploy-0.6.0.tgz`
  → `rpi --help` работает через шим; `target/` внутри пакета удалён;
  `npm uninstall -g rpi-deploy`.
- **Смоук (WSL Ubuntu):** тот же tgz, `npm install -g` под обычным юзером с
  локальным prefix — сборка, шим, uninstall.
- **`node --check`** для `bin/rpi.js`, `scripts/postinstall.js`,
  `scripts/check-version.js`.
- **Ручная acceptance (реальное железо, перед публикацией):** чистая Пайка с
  Docker и без; апгрейд v0.5-Пайки (юнит с бэкапом, рестарт); dev-машины
  Linux/macOS/Windows; обновление на Пайке; `--ignore-scripts` кейс.

## 11. Сопутствующие правки

- **README:** секция «Install via npm» над ручным путём: одна команда для
  клиента, `sudo npm i -g` + `sudo rpi agent setup` для Пайки, требование
  Node ≥ 18 (`sudo apt-get install -y nodejs npm` на Raspberry Pi OS) и
  Docker для агента; обновление через `npm i -g rpi-deploy@latest`; заметка
  про миграцию с v0.5 (старый `/usr/local/bin/pi`). Ручная сборка остаётся
  fallback'ом. **Глобальный свип команд** `pi …` → `rpi …` по всему README и
  `plugins/pi/skills/*/SKILL.md`; `pi.toml`, `pi-agent`, пути `/etc/pi` и
  т.п. НЕ трогаются. Статус → v0.6.
- **Версия:** workspace `Cargo.toml` и `package.json` → `0.6.0`.
- **.gitignore:** `dist/`, `node_modules/`.
- **LICENSE:** файл MIT в корне репозитория (§4).
- **Оговорка:** сборка на Пайке — несколько минут (~10 на младших моделях),
  теперь внутри `npm install`; прогресс cargo виден (stdio inherit).

## 12. Перенесено в v0.7+ / отменено

| Пункт | Статус | Почему |
|---|---|---|
| curl\|sh-скрипты install/update | **Отменено** | Заменены npm-каналом |
| Пребилты + платформ-пакеты/Releases + кросс-CI | v0.7+ (по необходимости) | Сборка из исходников устраивает; вернуться, если время сборки на Пайке станет проблемой |
| `rpi agent update` (drain/503) | v0.7+ | `npm i -g @latest` + self-install в setup закрывают практику; рестарт ловится свипом |
| Удалённый agent setup по SSH | v0.7+ | Сложность sudo-over-ssh |
| Автоустановка Docker | Отклонено (без изменений) | Docker — ручной пререквизит; зона `rpi agent setup` |
| Полный ребренд внутренних имён (`rpi-agent`, `/etc/rpi`, `rpi.toml`) | Отклонено | Миграция живого агента (state.db, владельцы, группы) не окупается; команда переименована, идентификаторы остались |

Версия по завершении: **0.6.0**.
