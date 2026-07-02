# pi v0.6 — Установка через npm (дизайн)

Дата: 2026-07-03. **Заменяет** `2026-06-18-pi-install-scripts-v0.6-design.md`
(curl|sh-скрипты сборки; подход отменён, см. §1). Базовая спека:
`2026-06-09-pi-deploy-tool-design.md` (§13 установка/self-setup, §16 команды,
§23 роадмап). Предшествующая версия: v0.5
(`2026-06-17-pi-install-ux-v0.5-design.md`).

**Критерий готовности:** установка и обновление — через npm:

```sh
npm install -g rpi-deploy          # клиент на dev-машине (Linux/macOS/Windows)
sudo npm install -g rpi-deploy     # агент на Пайке (Node >= 18)
sudo pi agent setup                # затем как обычно
```

Обновление: `npm install -g rpi-deploy@latest` (на Пайке с `sudo`, затем
`sudo pi agent setup` — он подхватывает новый бинарь и рестартит демон).

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

Скрипты `install.sh`/`update.sh`/`install.ps1`/`update.ps1` не создаются;
план `2026-07-02-pi-install-scripts-v0.6.md` отменён и удалён. Вместе со
скриптами умерли: интерактивный роль-промпт, маркер `.pi-role`, exit-code
контракт установщика. Роль теперь определяется использованием после
установки (`sudo pi agent setup` против `pi setup`), а не при установке.

## 2. Скоуп

Входит:

- **npm-пакет `rpi-deploy`** в корне репозитория: `package.json`,
  Node-шим `bin/pi.js`, `scripts/postinstall.js` (§4–§6).
- **Правка Rust** (единственная): `pi agent setup` получает self-install шаг —
  копирует собственный бинарь в `/usr/local/bin/pi` и рестартит активный
  `pi-agent` при смене бинаря (§7); улучшенное сообщение об отсутствии Docker.
- **Сопутствующее:** README (секция «Install via npm»), bump версии workspace
  и пакета до `0.6.0`, публикация вручную `npm publish` (§9, §11).

НЕ входит:

- Пребилт-бинари, платформ-пакеты (`optionalDependencies`), GitHub Releases,
  sha256, кросс-CI — не нужны при сборке из исходников (в v0.7+ только если
  время сборки на Пайке станет неприемлемым).
- curl|sh-скрипты — отменены, не переносятся.
- `pi agent update` как команда с drain/503-протоколом (→ v0.7+, §12).
- Удалённый `pi agent setup` по SSH (→ v0.7+).

## 3. Ключевые решения

1. **Имя пакета `rpi-deploy`** (имя `pi` на npm занято), команда — `pi`
   (`bin: {"pi": "bin/pi.js"}`; коллизий bin-имён npm не контролирует, это
   осознанный риск, как у любого CLI).
2. **Исходники в тарболе.** `files` в package.json: `crates/`, `Cargo.toml`,
   `Cargo.lock`, `bin/`, `scripts/postinstall.js`. Ни `target/`, ни доки, ни
   плагины в тарбол не попадают. git на машине пользователя не нужен вообще.
3. **`postinstall` собирает из исходников:** `cargo build --release --locked`,
   бинарь копируется в `dist/pi` (`dist/pi.exe`), затем `target/` **удаляется**
   — иначе на SD-карте Пайки в `node_modules` осталось бы ~1–2 ГБ. Цена:
   каждое обновление пересобирает с нуля (кэш реестра cargo в `~/.cargo`
   остаётся, поэтому зависимости не перекачиваются).
4. **Node-шим `bin/pi.js`** запускает `dist/pi` с пробросом аргументов,
   stdio и кода выхода. `engines.node >= 18`. Шим — постоянная точка входа
   (никакой подмены файла бинарём: на Windows npm всё равно генерирует
   `.cmd`-обёртки).
5. **Тулчейн:** нет `cargo` → postinstall сам ставит rustup (`-y`, per-user;
   при `sudo npm i -g` на Пайке — для root в `/root/.cargo`, это осознанный
   компромисс). Нет `cc`/`pkg-config` на Linux → понятная инструкция
   (`sudo apt-get install -y build-essential pkg-config`) и провал установки;
   postinstall **не** запускает apt сам. На Windows отдельной проверки MSVC
   нет — при падении cargo печатается подсказка про Visual Studio Build Tools
   (C++ workload). На macOS без `cc` — подсказка про
   `xcode-select --install`.
6. **Docker не проверяется при установке** — пакет один для обеих ролей,
   установка роль-агностична. Требование Docker остаётся зоной
   `pi agent setup`; его сообщение об ошибке дополняется командой
   `curl -fsSL https://get.docker.com | sh`.
7. **Канон бинаря агента — по-прежнему `/usr/local/bin/pi`** (`ExecStart`
   юнита не меняется). npm кладёт пакет в `node_modules`, путь нестабилен,
   поэтому `pi agent setup` копирует **собственный** бинарь
   (`std::env::current_exe()`) в `/usr/local/bin/pi` (§7).
8. **Обновление = `npm install -g rpi-deploy@latest`.** На Пайке затем
   `sudo pi agent setup`: он кладёт новый бинарь и рестартит активный юнит.
   Рестарт ловится существующим свипом `running→interrupted` (§8.1 базовой
   спеки) — drain-протокол не нужен.
9. **Публикация вручную:** `npm publish` из корня репо. `prepublishOnly`
   проверяет, что версия `package.json` совпадает с версией workspace в
   `Cargo.toml` (простой Node-скрипт, падает при расхождении).
10. **Удаление:** клиент — `npm uninstall -g rpi-deploy`; агент — сначала
    `sudo pi agent uninstall` (v0.5), затем `sudo npm uninstall -g rpi-deploy`.
    Копия `/usr/local/bin/pi` удаляется существующим `pi agent uninstall`.

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
  "bin": { "pi": "bin/pi.js" },
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
   После установки cargo берётся из `~/.cargo/bin` (PATH дополняется в
   env дочернего процесса).
2. **Проверка C-тулчейна (Linux/macOS):** нет `cc` или (Linux) `pkg-config` →
   напечатать команду установки (`sudo apt-get install -y build-essential
   pkg-config` на Debian-like; `xcode-select --install` на macOS) и выйти с
   ненулевым кодом (npm пометит установку проваленной).
3. **Сборка:** `cargo build --release --locked` в корне пакета
   (`process.cwd()` = каталог пакета при postinstall), stdout/stderr —
   inherit, чтобы пользователь видел прогресс. Провал сборки на Windows
   дополняется подсказкой про Visual Studio Build Tools (C++ workload).
4. **Установка бинаря в пакет:** скопировать
   `target/release/pi[.exe]` → `dist/pi[.exe]`, выставить `0o755`.
5. **Очистка:** рекурсивно удалить `target/` (см. §3.3).
6. Печать next-steps: клиент — `pi setup` / `pi init`; Пайка —
   `sudo pi agent setup` (и что Docker должен быть установлен заранее).

Ошибки на каждом шаге — короткое `error: …` с конкретной командой для
исправления; никакого автозапуска apt/brew/winget.

Замечание про root: при `sudo npm install -g` на Пайке npm (≥7) выполняет
postinstall от root — rustup уходит в `/root/.cargo`, сборка идёт от root.
Это принятый компромисс единого канала установки (§3.5).

## 6. `bin/pi.js` — шим

- Определяет путь `../dist/pi` (`pi.exe` на win32) относительно самого себя
  (`__dirname`), запускает через `child_process.spawnSync(bin, argv,
  {stdio: 'inherit'})`, транслирует код выхода (`process.exit(status)`);
  завершение по сигналу → выход с кодом `128+signal` (POSIX-конвенция).
- Если `dist/pi` отсутствует (например, установка с `--ignore-scripts`) —
  сообщение «binary not built; reinstall without --ignore-scripts:
  npm install -g rpi-deploy» и выход `1`.
- Никакой логики ролей/обновлений в шиме нет.

## 7. Правка Rust: self-install в `pi agent setup`

Новый шаг в начале установочной последовательности setup (до записи юнита и
`enable --now`):

1. `current = std::env::current_exe()?` (это реальный ELF: шим запускает
   бинарь дочерним процессом, так что `current_exe` внутри процесса —
   `dist/pi`, не Node).
2. Если `current` уже `/usr/local/bin/pi` — шаг пропускается (ручная
   установка из исходников продолжает работать как раньше).
3. Иначе сравнить с `/usr/local/bin/pi` побайтно (отсутствие файла считается
   отличием); при отличии — атомарно скопировать
   (`/usr/local/bin/.pi.tmp` → rename), `chmod 755`.
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
sudo pi agent setup              # копия бинаря в /usr/local/bin, юнит, старт
pi doctor
```

**Чистая Пайка без Docker:** `npm install -g` успешен (Docker не нужен для
сборки); `sudo pi agent setup` падает с сообщением, включающим
`curl -fsSL https://get.docker.com | sh`. После установки Docker — повторный
`sudo pi agent setup`.

**Dev-машина (Linux/macOS/Windows):**

```sh
npm install -g rpi-deploy
pi setup
pi init
```

**Обновление на Пайке:**

```sh
sudo npm install -g rpi-deploy@latest   # пересборка
sudo pi agent setup                     # новый бинарь + рестарт демона
```

**Linux без build-тулчейна:** postinstall падает с
`sudo apt-get install -y build-essential pkg-config`; после установки —
повторный `npm install -g rpi-deploy`.

**Установка с `--ignore-scripts`:** `pi` печатает «binary not built…» и
выходит `1`.

## 9. Архитектура и расположение

```
package.json              # npm-манифест (§4)
bin/pi.js                 # Node-шим (§6)
scripts/postinstall.js    # сборка при установке (§5)
scripts/check-version.js  # prepublishOnly: версия пакета == версии workspace
dist/                     # создаётся postinstall'ом; в git не попадает (.gitignore)
crates/bin/src/agent/setup.rs  # + self-install шаг (§7)
```

JS-файлы — без зависимостей и без сборки (plain Node, CommonJS). ESLint/CI
для них не заводится; проверка — `node --check` в тестах плана.

## 10. Тестирование

- **Юнит (Rust):** новый self-install шаг setup — тесты на tempdir (§7);
  регрессия `cargo test --workspace`.
- **Смоук (Windows, локально):** `npm pack` → `npm install -g ./rpi-deploy-0.6.0.tgz`
  → `pi --help` работает через шим; `target/` внутри пакета удалён;
  `npm uninstall -g rpi-deploy`.
- **Смоук (WSL Ubuntu):** тот же tgz, `npm install -g` под обычным юзером с
  локальным prefix — сборка, шим, uninstall.
- **`node --check`** для `bin/pi.js`, `scripts/postinstall.js`,
  `scripts/check-version.js`.
- **Ручная acceptance (реальное железо, перед публикацией):** чистая Пайка с
  Docker и без; dev-машины Linux/macOS/Windows; обновление на Пайке с
  рестартом демона; `--ignore-scripts` кейс.

## 11. Сопутствующие правки

- **README:** секция «Install via npm» над ручным путём: одна команда для
  клиента, `sudo npm i -g` + `sudo pi agent setup` для Пайки, требование
  Node ≥ 18 (`sudo apt-get install -y nodejs npm` на Raspberry Pi OS) и
  Docker для агента; обновление через `npm i -g rpi-deploy@latest`. Ручная
  сборка остаётся fallback'ом. Статус → v0.6.
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
| `pi agent update` (drain/503) | v0.7+ | `npm i -g @latest` + self-install в setup закрывают практику; рестарт ловится свипом |
| Удалённый `pi agent setup` по SSH | v0.7+ | Сложность sudo-over-ssh |
| Автоустановка Docker | Отклонено (без изменений) | Docker — ручной пререквизит; зона `pi agent setup` |

Версия по завершении: **0.6.0**.
