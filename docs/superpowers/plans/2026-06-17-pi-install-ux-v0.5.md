# pi v0.5 — Установка и UX — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Убрать ручное редактирование файлов при установке: на Пайке один `sudo pi agent setup`, на клиенте `pi setup` (профиль + SSH-ключ) и `pi init` (генерация `pi.toml`).

**Architecture:** Все изменения — только в крейте `crates/bin` (CLI + локальный бутстрап агента). Новых доменных сущностей, use-cases и HTTP-эндпоинтов нет. Системные эффекты (команды ОС, файлы, промпты, SSH) изолированы за трейтами (`Sys`, `Prompter`), поэтому логика юнит-тестируется на Windows фейками; реальные root/SSH-операции — `#[ignore]`. Канонические шаблоны (`agent.toml`, systemd-юнит) совпадают байт-в-байт с рабочей установкой, поэтому повторный `setup` — no-op.

**Tech Stack:** Rust 2021, `clap` (derive), `inquire` (промпты), `tokio::process`, `toml`, `dirs`, `anyhow`. Тесты — стандартные `#[test]`/`#[tokio::test]` с фейковыми трейтами.

Базовая спека: `docs/superpowers/specs/2026-06-17-pi-install-ux-v0.5-design.md`.

## Global Constraints

- Версия workspace по завершении: **`0.5.0`** (`Cargo.toml` `[workspace.package] version`).
- Edition `2021`; зависимости — через `[workspace.dependencies]`, крейт ссылается `{ workspace = true }`.
- **Никогда не трогать** `secret.key` и `state.db` (агент создаёт сам, exclusive-create). `setup`/`uninstall` без `--purge` их не пишут и не удаляют.
- Канонические шаблоны (§9 спеки) — **байт-в-байт**: systemd-юнит и `agent.toml` ниже не менять.
- Политика **Adopt & preserve**: каталоги/юзер/группы — только если отсутствуют; `agent.toml` — только при отсутствии; systemd-юнит — при отличии бэкап `*.bak`; членство в группах и `authorized_keys` — только добавление.
- cloudflared + linger — только при `--with-cloudflared`; по умолчанию не трогаются.
- Все системные эффекты — через трейты; юнит-тесты обязаны проходить на Windows. Реальные root/SSH/`ssh-keygen` операции помечаются `#[ignore]`.
- Тест-команды: `cargo test -p pi <name>`. Крейт-пакет называется `pi`.

## File Structure

**Создаются:**
- `crates/bin/src/cli/prompt.rs` — трейт `Prompter` + `InquirePrompter`.
- `crates/bin/src/cli/init.rs` — `pi init`: детект, резолв полей, рендер и запись `pi.toml`.
- `crates/bin/src/cli/keys.rs` — детект/генерация/заливка SSH-ключа клиент→Pi.
- `crates/bin/src/cli/setup.rs` — `pi setup`: мастер + ключ + сохранение профиля + тест связи.
- `crates/bin/src/agent/setup.rs` — трейт `Sys`, `HostSys`, шаблоны, оркестрация `setup`.
- `crates/bin/src/agent/uninstall.rs` — оркестрация `uninstall`.

**Модифицируются:**
- `crates/bin/src/cli/config.rs` — `Serialize` + `save_merged`.
- `crates/bin/src/cli/mod.rs` — `pub mod prompt; init; keys; setup;`.
- `crates/bin/src/agent/mod.rs` — `pub mod setup; uninstall;`.
- `crates/bin/src/main.rs` — команды `Setup`, `Init`, `AgentCmd::Setup`, `AgentCmd::Uninstall` + диспатч.
- `Cargo.toml` (root) — `inquire` в `[workspace.dependencies]`; версия `0.5.0`.
- `crates/bin/Cargo.toml` — `inquire = { workspace = true }`.
- `README.md`, `docs/install-agent-v0.1.md` — документация.

---

### Task 1: Запись клиентского конфига с merge (Adopt & preserve)

**Files:**
- Modify: `crates/bin/src/cli/config.rs`
- Test: `crates/bin/src/cli/config.rs` (модуль `#[cfg(test)]`)

**Interfaces:**
- Produces: `ClientConfig::save_merged(name: &str, profile: ServerProfile, make_default: bool) -> anyhow::Result<PathBuf>`; `ServerProfile`/`ClientConfig` теперь `Serialize`.
- Consumes: ничего нового.

- [ ] **Step 1: Failing-тест на merge, сохраняющий чужие профили и default**

В `#[cfg(test)] mod tests` добавить:

```rust
#[test]
fn save_merged_preserves_other_profiles_and_default() {
    let existing = r#"
default = "home"

[servers.home]
host = "pihost.local"
user = "piuser"
key = "~/.ssh/pi"
"#;
    let mut cfg = ClientConfig::parse(existing).unwrap();
    cfg.upsert(
        "work",
        ServerProfile { host: "10.0.0.2".into(), user: "deploy".into(), key: None },
        false,
    );
    let rendered = toml::to_string(&cfg).unwrap();
    let reparsed = ClientConfig::parse(&rendered).unwrap();
    assert_eq!(reparsed.default.as_deref(), Some("home"), "default preserved");
    assert_eq!(reparsed.servers.len(), 2);
    assert_eq!(reparsed.servers["home"].host, "pihost.local");
    assert_eq!(reparsed.servers["work"].host, "10.0.0.2");
    assert_eq!(reparsed.servers["work"].key, None);
}

#[test]
fn upsert_sets_default_only_when_requested_and_absent() {
    let mut cfg = ClientConfig { default: None, servers: Default::default() };
    cfg.upsert("home", ServerProfile { host: "h".into(), user: "u".into(), key: None }, true);
    assert_eq!(cfg.default.as_deref(), Some("home"));
    cfg.upsert("work", ServerProfile { host: "h2".into(), user: "u".into(), key: None }, true);
    assert_eq!(cfg.default.as_deref(), Some("home"), "existing default not overwritten");
}
```

- [ ] **Step 2: Запустить — убедиться, что не компилируется/падает**

Run: `cargo test -p pi save_merged_preserves`
Expected: FAIL — нет `upsert`, нет `Serialize` для `ClientConfig`.

- [ ] **Step 3: Добавить derive `Serialize` и метод `upsert`/`save_merged`**

В шапке заменить derive и поля:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct ClientConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default)]
    pub servers: HashMap<String, ServerProfile>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerProfile {
    pub host: String,
    pub user: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}
```

В `impl ClientConfig` добавить:

```rust
/// Insert or replace a profile; set it as `default` only when asked and no
/// default exists yet (Adopt & preserve).
pub fn upsert(&mut self, name: &str, profile: ServerProfile, make_default: bool) {
    self.servers.insert(name.to_string(), profile);
    if make_default && self.default.is_none() {
        self.default = Some(name.to_string());
    }
}

/// Load the existing config (or empty), upsert the profile, write it back.
pub fn save_merged(
    name: &str,
    profile: ServerProfile,
    make_default: bool,
) -> anyhow::Result<PathBuf> {
    let path = ClientConfig::path()?;
    let mut cfg = match std::fs::read_to_string(&path) {
        Ok(text) => ClientConfig::parse(&text)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            ClientConfig { default: None, servers: HashMap::new() }
        }
        Err(e) => return Err(anyhow::anyhow!("cannot read {}: {e}", path.display())),
    };
    cfg.upsert(name, profile, make_default);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, toml::to_string(&cfg)?)?;
    Ok(path)
}
```

- [ ] **Step 4: Запустить тесты**

Run: `cargo test -p pi cli::config`
Expected: PASS (новые + существующие `select_*`, `connect_opts_*`).

- [ ] **Step 5: Commit**

```bash
git add crates/bin/src/cli/config.rs
git commit -m "feat(cli): make client config writable with adopt-and-preserve merge"
```

---

### Task 2: Абстракция промптов (`Prompter` + `inquire`)

**Files:**
- Create: `crates/bin/src/cli/prompt.rs`
- Modify: `crates/bin/src/cli/mod.rs`, `crates/bin/Cargo.toml`, `Cargo.toml`
- Test: `crates/bin/src/cli/prompt.rs`

**Interfaces:**
- Produces: `trait Prompter { fn text(&mut self,&str,Option<&str>)->Result<String>; fn confirm(&mut self,&str,bool)->Result<bool>; fn select(&mut self,&str,&[String],usize)->Result<String> }`; `struct InquirePrompter`.
- Consumes: ничего.

- [ ] **Step 1: Добавить зависимость `inquire`**

В корневом `Cargo.toml`, секция `[workspace.dependencies]`, добавить строку:

```toml
inquire = "0.7"
```

В `crates/bin/Cargo.toml`, секция `[dependencies]`, добавить:

```toml
inquire = { workspace = true }
```

- [ ] **Step 2: Failing-тест на скриптованный фейк-промптер**

Создать `crates/bin/src/cli/prompt.rs`:

```rust
use anyhow::Result;

/// Abstracts interactive prompts so wizard logic is testable without a TTY.
pub trait Prompter {
    fn text(&mut self, label: &str, default: Option<&str>) -> Result<String>;
    fn confirm(&mut self, label: &str, default: bool) -> Result<bool>;
    fn select(&mut self, label: &str, options: &[String], default: usize) -> Result<String>;
}

#[cfg(test)]
pub struct ScriptedPrompter {
    pub texts: std::collections::VecDeque<String>,
    pub confirms: std::collections::VecDeque<bool>,
    pub selects: std::collections::VecDeque<String>,
}

#[cfg(test)]
impl Prompter for ScriptedPrompter {
    fn text(&mut self, _label: &str, default: Option<&str>) -> Result<String> {
        Ok(self.texts.pop_front().unwrap_or_else(|| default.unwrap_or("").to_string()))
    }
    fn confirm(&mut self, _label: &str, default: bool) -> Result<bool> {
        Ok(self.confirms.pop_front().unwrap_or(default))
    }
    fn select(&mut self, _label: &str, options: &[String], default: usize) -> Result<String> {
        Ok(self.selects.pop_front().unwrap_or_else(|| options[default].clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_prompter_returns_defaults_when_empty() {
        let mut p = ScriptedPrompter {
            texts: Default::default(),
            confirms: Default::default(),
            selects: Default::default(),
        };
        assert_eq!(p.text("name", Some("rateme")).unwrap(), "rateme");
        assert!(!p.confirm("ok?", false).unwrap());
        assert_eq!(p.select("x", &["a".into(), "b".into()], 1).unwrap(), "b");
    }
}
```

- [ ] **Step 3: Запустить тест**

Run: `cargo test -p pi cli::prompt`
Expected: PASS.

- [ ] **Step 4: Реальный `InquirePrompter` + регистрация модуля**

В конец `prompt.rs` (перед `#[cfg(test)]`) добавить:

```rust
pub struct InquirePrompter;

impl Prompter for InquirePrompter {
    fn text(&mut self, label: &str, default: Option<&str>) -> Result<String> {
        let mut q = inquire::Text::new(label);
        if let Some(d) = default {
            q = q.with_default(d);
        }
        Ok(q.prompt()?)
    }
    fn confirm(&mut self, label: &str, default: bool) -> Result<bool> {
        Ok(inquire::Confirm::new(label).with_default(default).prompt()?)
    }
    fn select(&mut self, label: &str, options: &[String], default: usize) -> Result<String> {
        Ok(inquire::Select::new(label, options.to_vec())
            .with_starting_cursor(default)
            .prompt()?)
    }
}
```

В `crates/bin/src/cli/mod.rs` добавить строку `pub mod prompt;` (в алфавитном порядке после `pitoml`).

- [ ] **Step 5: Сборка + тест**

Run: `cargo test -p pi cli::prompt`
Expected: PASS, крейт компилируется с `inquire`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/bin/Cargo.toml crates/bin/src/cli/prompt.rs crates/bin/src/cli/mod.rs
git commit -m "feat(cli): add Prompter abstraction over inquire"
```

---

### Task 3: Рендеринг `pi.toml`

**Files:**
- Create: `crates/bin/src/cli/init.rs`
- Modify: `crates/bin/src/cli/mod.rs`
- Test: `crates/bin/src/cli/init.rs`

**Interfaces:**
- Produces: `struct InitFields { name, repo, branch, compose, service, port: u16, hostname: Option<String>, expose: Option<String>, env_file: Option<String> }`; `fn render_pi_toml(&InitFields) -> String`.
- Consumes: ничего.

- [ ] **Step 1: Failing-тест на рендер (golden-string)**

Создать `crates/bin/src/cli/init.rs`:

```rust
/// Resolved field set for `pi init`, ready to render into pi.toml (§7).
pub struct InitFields {
    pub name: String,
    pub repo: String,
    pub branch: String,
    pub compose: String,
    pub service: String,
    pub port: u16,
    pub hostname: Option<String>,
    pub expose: Option<String>,
    pub env_file: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> InitFields {
        InitFields {
            name: "rateme".into(),
            repo: "git@github.com:isskelo/rateme.git".into(),
            branch: "main".into(),
            compose: "docker-compose.yml".into(),
            service: "web".into(),
            port: 3000,
            hostname: Some("rateme.example.com".into()),
            expose: None,
            env_file: Some(".env".into()),
        }
    }

    #[test]
    fn render_is_parseable_and_round_trips() {
        let text = render_pi_toml(&sample());
        let parsed = crate::cli::pitoml::PiToml::parse(&text).unwrap();
        let cfg = parsed.to_project_config();
        assert_eq!(cfg.name, "rateme");
        assert_eq!(cfg.service, "web");
        assert_eq!(cfg.container_port, 3000);
        assert_eq!(cfg.hostname.as_deref(), Some("rateme.example.com"));
        assert!(text.starts_with("schema = 1\n"));
    }

    #[test]
    fn lan_expose_and_no_hostname_render() {
        let mut f = sample();
        f.hostname = None;
        f.expose = Some("lan".into());
        let text = render_pi_toml(&f);
        assert!(!text.contains("hostname"));
        assert!(text.contains("expose = \"lan\""));
        let cfg = crate::cli::pitoml::PiToml::parse(&text).unwrap().to_project_config();
        assert_eq!(cfg.expose, pi_domain::entities::ExposeMode::Lan);
    }
}
```

- [ ] **Step 2: Запустить — fail (нет `render_pi_toml`)**

Run: `cargo test -p pi cli::init`
Expected: FAIL — `render_pi_toml` не найден; модуль ещё не подключён.

- [ ] **Step 3: Реализовать `render_pi_toml` и подключить модуль**

В `init.rs` (над `#[cfg(test)]`) добавить:

```rust
use std::fmt::Write as _;

/// Render canonical pi.toml text (schema 1, §12) from resolved fields.
pub fn render_pi_toml(f: &InitFields) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "schema = 1\n");
    let _ = writeln!(s, "[project]");
    let _ = writeln!(s, "name = \"{}\"\n", f.name);
    let _ = writeln!(s, "[source]");
    let _ = writeln!(s, "repo = \"{}\"", f.repo);
    let _ = writeln!(s, "branch = \"{}\"\n", f.branch);
    let _ = writeln!(s, "[build]");
    let _ = writeln!(s, "compose = \"{}\"\n", f.compose);
    let _ = writeln!(s, "[ingress]");
    if let Some(h) = &f.hostname {
        let _ = writeln!(s, "hostname = \"{h}\"");
    }
    let _ = writeln!(s, "service = \"{}\"", f.service);
    let _ = writeln!(s, "port = {}", f.port);
    if let Some(e) = &f.expose {
        let _ = writeln!(s, "expose = \"{e}\"");
    }
    if let Some(env) = &f.env_file {
        let _ = writeln!(s, "\n[env]");
        let _ = writeln!(s, "file = \"{env}\"");
    }
    s
}
```

В `crates/bin/src/cli/mod.rs` добавить `pub mod init;` (после `pub mod config;`).

- [ ] **Step 4: Запустить тесты**

Run: `cargo test -p pi cli::init`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bin/src/cli/init.rs crates/bin/src/cli/mod.rs
git commit -m "feat(init): render canonical pi.toml from resolved fields"
```

---

### Task 4: Авто-детект и резолв полей `pi init`

**Files:**
- Modify: `crates/bin/src/cli/init.rs`
- Test: `crates/bin/src/cli/init.rs`

**Interfaces:**
- Consumes: `Prompter` (Task 2); `InitFields`/`render_pi_toml` (Task 3).
- Produces: `struct DetectedDefaults { name, repo: Option<String>, branch: Option<String>, compose: Option<String>, env_file: Option<String> }`; `fn detect_defaults(&Path) -> DetectedDefaults`; `struct InitFlags {...}`; `fn resolve_init_fields(&InitFlags,&DetectedDefaults,&mut dyn Prompter) -> Result<InitFields>`.

- [ ] **Step 1: Failing-тест на резолв (флаги > детект > промпт)**

Добавить в `mod tests`:

```rust
use crate::cli::prompt::ScriptedPrompter;

fn detected() -> DetectedDefaults {
    DetectedDefaults {
        name: "rateme".into(),
        repo: Some("git@github.com:isskelo/rateme.git".into()),
        branch: Some("main".into()),
        compose: Some("docker-compose.yml".into()),
        env_file: Some(".env".into()),
    }
}

#[test]
fn yes_mode_uses_detected_without_prompting() {
    let flags = InitFlags { yes: true, ..InitFlags::default() };
    let mut p = ScriptedPrompter {
        texts: Default::default(),
        confirms: Default::default(),
        selects: Default::default(),
    };
    // service/port/hostname have no detected default -> in --yes they come from flags or fall back.
    let flags = InitFlags { service: Some("web".into()), port: Some(3000), ..flags };
    let f = resolve_init_fields(&flags, &detected(), &mut p).unwrap();
    assert_eq!(f.name, "rateme");
    assert_eq!(f.repo, "git@github.com:isskelo/rateme.git");
    assert_eq!(f.service, "web");
    assert_eq!(f.port, 3000);
}

#[test]
fn flags_override_detected() {
    let flags = InitFlags {
        yes: true,
        name: Some("other".into()),
        service: Some("api".into()),
        port: Some(8080),
        expose: Some("lan".into()),
        ..InitFlags::default()
    };
    let mut p = ScriptedPrompter {
        texts: Default::default(),
        confirms: Default::default(),
        selects: Default::default(),
    };
    let f = resolve_init_fields(&flags, &detected(), &mut p).unwrap();
    assert_eq!(f.name, "other");
    assert_eq!(f.service, "api");
    assert_eq!(f.expose.as_deref(), Some("lan"));
}

#[test]
fn interactive_selects_expose_mode() {
    // yes = false -> expose is chosen via select (no --expose flag)
    let flags = InitFlags { service: Some("web".into()), port: Some(3000), ..InitFlags::default() };
    let mut p = ScriptedPrompter {
        texts: Default::default(),
        confirms: Default::default(),
        selects: ["lan".to_string()].into_iter().collect(),
    };
    let f = resolve_init_fields(&flags, &detected(), &mut p).unwrap();
    assert_eq!(f.expose.as_deref(), Some("lan"), "expose chosen via select");
}
```

- [ ] **Step 2: Запустить — fail**

Run: `cargo test -p pi cli::init`
Expected: FAIL — нет `InitFlags`/`DetectedDefaults`/`resolve_init_fields`.

- [ ] **Step 3: Реализовать детект, флаги и резолв**

В `init.rs` (над `#[cfg(test)]`) добавить:

```rust
use std::path::Path;
use crate::cli::prompt::Prompter;

#[derive(Default)]
pub struct InitFlags {
    pub name: Option<String>,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub compose: Option<String>,
    pub service: Option<String>,
    pub port: Option<u16>,
    pub hostname: Option<String>,
    pub expose: Option<String>,
    pub env_file: Option<String>,
    pub yes: bool,
}

pub struct DetectedDefaults {
    pub name: String,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub compose: Option<String>,
    pub env_file: Option<String>,
}

/// Best-effort auto-detection of pi.toml defaults from the project dir (§7).
pub fn detect_defaults(cwd: &Path) -> DetectedDefaults {
    let name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("app")
        .to_string();
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git").args(args).current_dir(cwd).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let repo = git(&["remote", "get-url", "origin"]).filter(|s| !s.is_empty());
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).filter(|s| !s.is_empty() && s != "HEAD");
    let compose = ["docker-compose.yml", "compose.yaml", "compose.yml", "docker-compose.yaml"]
        .into_iter()
        .find(|f| cwd.join(f).exists())
        .map(String::from);
    let env_file = cwd.join(".env").exists().then(|| ".env".to_string());
    DetectedDefaults { name, repo, branch, compose, env_file }
}

fn ask_text(flag: &Option<String>, det: Option<&str>, label: &str, yes: bool, p: &mut dyn Prompter) -> anyhow::Result<String> {
    if let Some(v) = flag {
        return Ok(v.clone());
    }
    if yes {
        return det
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("--yes: missing value for {label}; pass it as a flag"));
    }
    p.text(label, det)
}

/// Resolve final fields: flags win, then detected defaults, then prompt (§7).
pub fn resolve_init_fields(
    flags: &InitFlags,
    det: &DetectedDefaults,
    p: &mut dyn Prompter,
) -> anyhow::Result<InitFields> {
    let name = ask_text(&flags.name, Some(&det.name), "project name", flags.yes, p)?;
    let repo = ask_text(&flags.repo, det.repo.as_deref(), "git repo url", flags.yes, p)?;
    let branch = ask_text(&flags.branch, det.branch.as_deref().or(Some("main")), "branch", flags.yes, p)?;
    let compose = ask_text(&flags.compose, det.compose.as_deref().or(Some("docker-compose.yml")), "compose file", flags.yes, p)?;
    let service = ask_text(&flags.service, None, "ingress service (compose service name)", flags.yes, p)?;
    let port = match flags.port {
        Some(p) => p,
        None if flags.yes => anyhow::bail!("--yes: missing --port"),
        None => p.text("container port", Some("3000"))?.trim().parse()?,
    };
    let hostname = match &flags.hostname {
        Some(h) if !h.is_empty() => Some(h.clone()),
        Some(_) => None,
        None if flags.yes => None,
        None => {
            let v = p.text("public hostname (empty = no public ingress)", None)?;
            (!v.trim().is_empty()).then(|| v.trim().to_string())
        }
    };
    let expose = match &flags.expose {
        Some(e) => Some(e.clone()),
        None if flags.yes => None,
        None => {
            let choice = p.select("expose mode", &["private".into(), "lan".into()], 0)?;
            (choice == "lan").then(|| "lan".to_string())
        }
    };
    let env_file = flags.env_file.clone().or_else(|| det.env_file.clone());
    Ok(InitFields { name, repo, branch, compose, service, port, hostname, expose, env_file })
}
```

- [ ] **Step 4: Запустить тесты**

Run: `cargo test -p pi cli::init`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bin/src/cli/init.rs
git commit -m "feat(init): detect project defaults and resolve fields with flags/prompts"
```

---

### Task 5: Команда `pi init` (clap + запись файла)

**Files:**
- Modify: `crates/bin/src/cli/init.rs`, `crates/bin/src/main.rs`
- Test: `crates/bin/src/main.rs`

**Interfaces:**
- Consumes: `resolve_init_fields`, `render_pi_toml`, `detect_defaults`, `InquirePrompter`.
- Produces: `cli::init::run(flags: InitFlags) -> anyhow::Result<()>`; clap `Cmd::Init`.

- [ ] **Step 1: Реализовать `run` (детект → резолв → запись с бэкапом)**

В `init.rs` добавить:

```rust
use crate::cli::prompt::InquirePrompter;

/// Entrypoint for `pi init`: detect, resolve, write ./pi.toml (backup if present).
pub async fn run(flags: InitFlags) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let det = detect_defaults(&cwd);
    let mut prompter = InquirePrompter;
    let fields = resolve_init_fields(&flags, &det, &mut prompter)?;
    let text = render_pi_toml(&fields);

    let path = cwd.join("pi.toml");
    if path.exists() {
        let overwrite = flags.yes || prompter.confirm("pi.toml exists — overwrite? (a .bak is kept)", false)?;
        if !overwrite {
            println!("aborted: pi.toml left unchanged");
            return Ok(());
        }
        std::fs::rename(&path, cwd.join("pi.toml.bak"))?;
    }
    std::fs::write(&path, &text)?;
    println!("wrote {}", path.display());
    println!("next: `pi env send` (if you use secrets), then `pi deploy`");
    Ok(())
}
```

- [ ] **Step 2: Failing-тест на парсинг clap для `pi init`**

В `main.rs`, модуль `#[cfg(test)] mod tests`, добавить:

```rust
#[test]
fn init_flags_parse() {
    let cli = Cli::try_parse_from([
        "pi", "init", "--name", "rateme", "--port", "3000", "--expose", "lan", "--yes",
    ])
    .unwrap();
    match cli.cmd {
        Cmd::Init(args) => {
            assert_eq!(args.name.as_deref(), Some("rateme"));
            assert_eq!(args.port, Some(3000));
            assert_eq!(args.expose.as_deref(), Some("lan"));
            assert!(args.yes);
        }
        _ => panic!("expected init"),
    }
}
```

- [ ] **Step 3: Запустить — fail**

Run: `cargo test -p pi init_flags_parse`
Expected: FAIL — нет варианта `Cmd::Init`.

- [ ] **Step 4: Добавить `InitArgs` + вариант + диспатч в `main.rs`**

В `main.rs` над `enum Cmd` добавить структуру аргументов:

```rust
#[derive(clap::Args)]
struct InitArgs {
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    repo: Option<String>,
    #[arg(long)]
    branch: Option<String>,
    #[arg(long)]
    compose: Option<String>,
    #[arg(long)]
    service: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    hostname: Option<String>,
    #[arg(long)]
    expose: Option<String>,
    #[arg(long = "env")]
    env_file: Option<String>,
    #[arg(long)]
    yes: bool,
}
```

В `enum Cmd` добавить вариант:

```rust
    /// Generate pi.toml in the current project (wizard; flags for CI)
    Init(InitArgs),
```

В `match cli.cmd` добавить рукав (рядом с другими):

```rust
        Cmd::Init(a) => {
            cli::init::run(cli::init::InitFlags {
                name: a.name,
                repo: a.repo,
                branch: a.branch,
                compose: a.compose,
                service: a.service,
                port: a.port,
                hostname: a.hostname,
                expose: a.expose,
                env_file: a.env_file,
                yes: a.yes,
            })
            .await
        }
```

- [ ] **Step 5: Запустить тест и сборку**

Run: `cargo test -p pi init_flags_parse && cargo build -p pi`
Expected: PASS + успешная сборка.

- [ ] **Step 6: Commit**

```bash
git add crates/bin/src/cli/init.rs crates/bin/src/main.rs
git commit -m "feat(init): wire `pi init` command with file write and backup"
```

---

### Task 6: SSH-бутстрап ключа клиент→Pi

**Files:**
- Create: `crates/bin/src/cli/keys.rs`
- Modify: `crates/bin/src/cli/mod.rs`
- Test: `crates/bin/src/cli/keys.rs`

**Interfaces:**
- Consumes: `ServerProfile` (config.rs); `expand_home` (tunnel.rs).
- Produces: `fn detect_ssh_keys(&Path) -> Vec<PathBuf>`; `fn pubkey_path(&Path) -> PathBuf`; `async fn generate_key(&Path) -> Result<()>`; `async fn push_pubkey(&ServerProfile, &Path) -> Result<()>`.

- [ ] **Step 1: Failing-тест на детект ключей и путь pubkey**

Создать `crates/bin/src/cli/keys.rs`:

```rust
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pubkey_path_appends_pub() {
        assert_eq!(pubkey_path(Path::new("/home/u/.ssh/pi")), PathBuf::from("/home/u/.ssh/pi.pub"));
    }

    #[test]
    fn detect_finds_private_keys_skips_pub_and_known_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = dir.path();
        std::fs::write(ssh.join("pi"), "k").unwrap();
        std::fs::write(ssh.join("pi.pub"), "k").unwrap();
        std::fs::write(ssh.join("id_ed25519"), "k").unwrap();
        std::fs::write(ssh.join("known_hosts"), "k").unwrap();
        std::fs::write(ssh.join("config"), "k").unwrap();
        let mut found = detect_ssh_keys(ssh);
        found.sort();
        assert_eq!(found, vec![ssh.join("id_ed25519"), ssh.join("pi")]);
    }
}
```

- [ ] **Step 2: Запустить — fail**

Run: `cargo test -p pi cli::keys`
Expected: FAIL — модуль не подключён, функций нет.

- [ ] **Step 3: Реализовать детект/генерацию/заливку и подключить модуль**

В `keys.rs` (над `#[cfg(test)]`) добавить:

```rust
use crate::cli::config::ServerProfile;
use crate::cli::tunnel::expand_home;

/// Private-key candidates in an .ssh dir: skip *.pub, known_hosts, config, authorized_keys.
pub fn detect_ssh_keys(ssh_dir: &Path) -> Vec<PathBuf> {
    let skip = ["known_hosts", "config", "authorized_keys"];
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(ssh_dir) else { return out };
    for e in entries.flatten() {
        let path = e.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if name.ends_with(".pub") || skip.contains(&name) || name.starts_with('.') {
            continue;
        }
        if path.is_file() {
            out.push(path);
        }
    }
    out
}

pub fn pubkey_path(key: &Path) -> PathBuf {
    let mut s = key.as_os_str().to_os_string();
    s.push(".pub");
    PathBuf::from(s)
}

/// Generate an ed25519 keypair at `path` (no passphrase).
pub async fn generate_key(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let status = tokio::process::Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-f"])
        .arg(path)
        .status()
        .await?;
    if !status.success() {
        anyhow::bail!("ssh-keygen failed");
    }
    Ok(())
}

/// Append our pubkey to the Pi's authorized_keys (ssh-copy-id equivalent;
/// works on Windows OpenSSH which lacks ssh-copy-id). Interactive: may prompt
/// for the Pi password once.
pub async fn push_pubkey(profile: &ServerProfile, pubkey: &Path) -> anyhow::Result<()> {
    let pubkey_text = std::fs::read_to_string(pubkey)?;
    let mut cmd = tokio::process::Command::new("ssh");
    if let Some(key) = &profile.key {
        cmd.arg("-i").arg(expand_home(key));
    }
    cmd.args([
        "-o", "StrictHostKeyChecking=accept-new",
        &format!("{}@{}", profile.user, profile.host),
        "umask 077; mkdir -p ~/.ssh && cat >> ~/.ssh/authorized_keys",
    ]);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::inherit());
    cmd.stderr(std::process::Stdio::inherit());
    let mut child = cmd.spawn()?;
    use tokio::io::AsyncWriteExt;
    child.stdin.take().unwrap().write_all(pubkey_text.as_bytes()).await?;
    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("failed to copy public key to {}", profile.host);
    }
    Ok(())
}
```

В `crates/bin/src/cli/mod.rs` добавить `pub mod keys;` (после `pub mod init;`).

- [ ] **Step 4: Запустить тесты**

Run: `cargo test -p pi cli::keys`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bin/src/cli/keys.rs crates/bin/src/cli/mod.rs
git commit -m "feat(setup): ssh key detect/generate/copy helpers (windows-friendly)"
```

---

### Task 7: Команда `pi setup` (мастер клиента)

**Files:**
- Create: `crates/bin/src/cli/setup.rs`
- Modify: `crates/bin/src/cli/mod.rs`, `crates/bin/src/main.rs`
- Test: `crates/bin/src/cli/setup.rs`, `crates/bin/src/main.rs`

**Interfaces:**
- Consumes: `ServerProfile`/`ClientConfig::save_merged` (Task 1), `SshExec` (ssh.rs), `keys::*` (Task 6), `cli::commands::doctor` (existing), `Prompter`/`InquirePrompter` (Task 2).
- Produces: `struct SetupFlags {...}`; `async fn run(SetupFlags) -> Result<()>`; `fn resolve_profile(&SetupFlags, keys: &[PathBuf], &mut dyn Prompter) -> Result<(String, ServerProfile, bool)>` (offers detected keys via `select`); clap `Cmd::Setup`.

- [ ] **Step 1: Failing-тест на резолв профиля из флагов**

Создать `crates/bin/src/cli/setup.rs`:

```rust
use crate::cli::config::ServerProfile;
use crate::cli::prompt::Prompter;

#[derive(Default)]
pub struct SetupFlags {
    pub host: Option<String>,
    pub user: Option<String>,
    pub key: Option<String>,
    pub name: Option<String>,
    pub default: bool,
    pub yes: bool,
}

/// Resolve (alias, profile, make_default) from flags, prompting when needed.
/// `keys` are SSH key candidates (from `detect_ssh_keys`), offered via `select`.
pub fn resolve_profile(
    flags: &SetupFlags,
    keys: &[std::path::PathBuf],
    p: &mut dyn Prompter,
) -> anyhow::Result<(String, ServerProfile, bool)> {
    let name = match &flags.name {
        Some(n) => n.clone(),
        None if flags.yes => "home".to_string(),
        None => {
            let v = p.text("server alias", Some("home"))?;
            if v.trim().is_empty() { "home".to_string() } else { v.trim().to_string() }
        }
    };
    let host = match &flags.host {
        Some(h) => h.clone(),
        None if flags.yes => anyhow::bail!("--yes: missing --host"),
        None => p.text("Pi host or IP", None)?,
    };
    let user = match &flags.user {
        Some(u) => u.clone(),
        None if flags.yes => anyhow::bail!("--yes: missing --user"),
        None => p.text("SSH login user", None)?,
    };
    let key = match &flags.key {
        Some(k) => Some(k.clone()),
        None if flags.yes => None,
        None if keys.is_empty() => Some("~/.ssh/id_ed25519".to_string()),
        None => {
            let mut opts: Vec<String> = keys.iter().map(|p| p.display().to_string()).collect();
            let generate = "(generate a new key at ~/.ssh/id_ed25519)";
            opts.push(generate.into());
            let choice = p.select("SSH key", &opts, 0)?;
            if choice == generate {
                Some("~/.ssh/id_ed25519".to_string())
            } else {
                Some(choice)
            }
        }
    };
    let make_default = flags.default || flags.yes;
    Ok((name, ServerProfile { host, user, key }, make_default))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::prompt::ScriptedPrompter;

    #[test]
    fn resolve_profile_from_flags_yes() {
        let flags = SetupFlags {
            host: Some("pihost.local".into()),
            user: Some("piuser".into()),
            key: Some("~/.ssh/pi".into()),
            name: Some("home".into()),
            default: false,
            yes: true,
        };
        let mut p = ScriptedPrompter {
            texts: Default::default(), confirms: Default::default(), selects: Default::default(),
        };
        let (name, profile, make_default) = resolve_profile(&flags, &[], &mut p).unwrap();
        assert_eq!(name, "home");
        assert_eq!(profile.host, "pihost.local");
        assert_eq!(profile.key.as_deref(), Some("~/.ssh/pi"));
        assert!(make_default);
    }

    #[test]
    fn yes_without_host_errors() {
        let flags = SetupFlags { user: Some("u".into()), yes: true, ..SetupFlags::default() };
        let mut p = ScriptedPrompter {
            texts: Default::default(), confirms: Default::default(), selects: Default::default(),
        };
        assert!(resolve_profile(&flags, &[], &mut p).is_err());
    }

    #[test]
    fn interactive_selects_ssh_key() {
        use std::path::PathBuf;
        let flags = SetupFlags {
            host: Some("pihost.local".into()),
            user: Some("piuser".into()),
            ..SetupFlags::default()
        };
        let keys = vec![PathBuf::from("/home/u/.ssh/pi"), PathBuf::from("/home/u/.ssh/id_ed25519")];
        let mut p = ScriptedPrompter {
            texts: Default::default(),
            confirms: Default::default(),
            selects: ["/home/u/.ssh/pi".to_string()].into_iter().collect(),
        };
        let (_, profile, _) = resolve_profile(&flags, &keys, &mut p).unwrap();
        assert_eq!(profile.key.as_deref(), Some("/home/u/.ssh/pi"));
    }
}
```

- [ ] **Step 2: Запустить — fail (модуль не подключён)**

Run: `cargo test -p pi cli::setup`
Expected: FAIL — `cli::setup` не объявлен в `mod.rs`.

- [ ] **Step 3: Подключить модуль + реализовать `run`**

В `crates/bin/src/cli/mod.rs` добавить `pub mod setup;` (после `pub mod sse;` или в алфавитном порядке).

В `setup.rs` добавить (над `#[cfg(test)]`):

```rust
use crate::cli::config::{ClientConfig, ConnectOpts};
use crate::cli::prompt::InquirePrompter;
use crate::cli::ssh::SshExec;

/// Entrypoint for `pi setup`: profile + key bootstrap + save + connectivity test.
pub async fn run(flags: SetupFlags) -> anyhow::Result<()> {
    let mut prompter = InquirePrompter;
    let ssh_dir = dirs::home_dir().map(|h| h.join(".ssh")).unwrap_or_default();
    let detected_keys = crate::cli::keys::detect_ssh_keys(&ssh_dir);
    let (name, profile, make_default) = resolve_profile(&flags, &detected_keys, &mut prompter)?;

    // Key bootstrap: adopt if SSH already works, else offer to generate+push.
    let ssh = SshExec { profile: &profile };
    if ssh.check().await.is_err() && !flags.yes {
        if let Some(key) = &profile.key {
            let key_path = crate::cli::tunnel::expand_home(key);
            let pubkey = crate::cli::keys::pubkey_path(&key_path);
            if !key_path.exists()
                && prompter.confirm(&format!("no key at {} — generate one?", key_path.display()), true)?
            {
                crate::cli::keys::generate_key(&key_path).await?;
            }
            if pubkey.exists()
                && prompter.confirm("copy public key to the Pi now? (asks Pi password once)", true)?
            {
                crate::cli::keys::push_pubkey(&profile, &pubkey).await?;
            }
        }
    }

    let path = ClientConfig::save_merged(&name, profile.clone(), make_default)?;
    println!("saved profile '{name}' to {}", path.display());

    // Connectivity test reuses the existing doctor path against the new profile.
    println!("testing connection...");
    if let Err(e) = ssh.check().await {
        println!("ssh check failed: {e}");
        println!("fix SSH access, then run `pi doctor --server {name}`");
        return Ok(());
    }
    let connect = ConnectOpts { server: Some(name.clone()), host: None, user: None, key: None };
    crate::cli::commands::doctor(connect).await
}
```

- [ ] **Step 4: Failing-тест clap для `pi setup` + диспатч**

В `main.rs` `mod tests` добавить:

```rust
#[test]
fn setup_flags_parse() {
    let cli = Cli::try_parse_from([
        "pi", "setup", "--host", "pihost.local", "--user", "piuser", "--key", "~/.ssh/pi", "--yes",
    ])
    .unwrap();
    match cli.cmd {
        Cmd::Setup(a) => {
            assert_eq!(a.host.as_deref(), Some("pihost.local"));
            assert_eq!(a.user.as_deref(), Some("piuser"));
            assert!(a.yes);
        }
        _ => panic!("expected setup"),
    }
}
```

Run: `cargo test -p pi setup_flags_parse`
Expected: FAIL — нет `Cmd::Setup`.

- [ ] **Step 5: Добавить `SetupArgs` + вариант + диспатч**

В `main.rs` над `enum Cmd`:

```rust
#[derive(clap::Args)]
struct SetupArgs {
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    user: Option<String>,
    #[arg(long)]
    key: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    default: bool,
    #[arg(long)]
    yes: bool,
}
```

В `enum Cmd`:

```rust
    /// Configure a server profile on this machine (wizard; flags for CI)
    Setup(SetupArgs),
```

В `match cli.cmd`:

```rust
        Cmd::Setup(a) => {
            cli::setup::run(cli::setup::SetupFlags {
                host: a.host,
                user: a.user,
                key: a.key,
                name: a.name,
                default: a.default,
                yes: a.yes,
            })
            .await
        }
```

- [ ] **Step 6: Тест + сборка**

Run: `cargo test -p pi cli::setup setup_flags_parse && cargo build -p pi`
Expected: PASS + сборка.

- [ ] **Step 7: Commit**

```bash
git add crates/bin/src/cli/setup.rs crates/bin/src/cli/mod.rs crates/bin/src/main.rs
git commit -m "feat(setup): wire `pi setup` wizard with key bootstrap and doctor test"
```

---

### Task 8: Абстракция системы `Sys` + `HostSys`

**Files:**
- Create: `crates/bin/src/agent/setup.rs`
- Modify: `crates/bin/src/agent/mod.rs`
- Test: `crates/bin/src/agent/setup.rs`

**Interfaces:**
- Produces: `#[async_trait] trait Sys { async fn run(&self,&str,&[&str])->Result<String,String>; fn exists(&self,&Path)->bool; fn read(&self,&Path)->Option<String>; fn write(&self,&Path,&str)->Result<(),String> }`; `struct HostSys`; `async fn user_exists(&dyn Sys,&str)->bool`; `async fn in_group(&dyn Sys,&str,&str)->bool`.
- Consumes: `async_trait`.

- [ ] **Step 1: Failing-тест на хелперы поверх фейка**

Создать `crates/bin/src/agent/setup.rs`:

```rust
use std::path::Path;
use async_trait::async_trait;

/// All OS effects setup needs, behind a trait so logic is testable off-Linux.
#[async_trait]
pub trait Sys: Send + Sync {
    async fn run(&self, program: &str, args: &[&str]) -> Result<String, String>;
    fn exists(&self, path: &Path) -> bool;
    fn read(&self, path: &Path) -> Option<String>;
    fn write(&self, path: &Path, content: &str) -> Result<(), String>;
}

/// True if a system user exists (`id -u <name>` succeeds).
pub async fn user_exists(sys: &dyn Sys, name: &str) -> bool {
    sys.run("id", &["-u", name]).await.is_ok()
}

/// True if `user` is a member of `group` (parsed from `id -nG <user>`).
pub async fn in_group(sys: &dyn Sys, user: &str, group: &str) -> bool {
    matches!(sys.run("id", &["-nG", user]).await, Ok(s) if s.split_whitespace().any(|g| g == group))
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FakeSys {
        pub paths: HashSet<String>,
        pub files: HashMap<String, String>,
        pub ok: HashMap<String, String>,   // "program a b" -> stdout
        pub err: HashSet<String>,          // "program a b" that fail
        pub calls: Mutex<Vec<String>>,
        pub writes: Mutex<Vec<(String, String)>>,
    }

    impl FakeSys {
        pub fn key(program: &str, args: &[&str]) -> String {
            std::iter::once(program).chain(args.iter().copied()).collect::<Vec<_>>().join(" ")
        }
        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Sys for FakeSys {
        async fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
            let k = FakeSys::key(program, args);
            self.calls.lock().unwrap().push(k.clone());
            if self.err.contains(&k) {
                return Err(format!("fake error: {k}"));
            }
            Ok(self.ok.get(&k).cloned().unwrap_or_default())
        }
        fn exists(&self, path: &Path) -> bool {
            self.paths.contains(path.to_str().unwrap())
        }
        fn read(&self, path: &Path) -> Option<String> {
            self.files.get(path.to_str().unwrap()).cloned()
        }
        fn write(&self, path: &Path, content: &str) -> Result<(), String> {
            self.writes.lock().unwrap().push((path.to_string_lossy().into(), content.into()));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::fake::FakeSys;

    #[tokio::test]
    async fn user_exists_reflects_id_result() {
        let mut sys = FakeSys::default();
        sys.ok.insert(FakeSys::key("id", &["-u", "pi-agent"]), "999".into());
        assert!(user_exists(&sys, "pi-agent").await);

        let mut absent = FakeSys::default();
        absent.err.insert(FakeSys::key("id", &["-u", "pi-agent"]));
        assert!(!user_exists(&absent, "pi-agent").await);
    }

    #[tokio::test]
    async fn in_group_parses_id_ng() {
        let mut sys = FakeSys::default();
        sys.ok.insert(FakeSys::key("id", &["-nG", "piuser"]), "piuser sudo docker pi-agent".into());
        assert!(in_group(&sys, "piuser", "docker").await);
        assert!(!in_group(&sys, "piuser", "wheel").await);
    }
}
```

- [ ] **Step 2: Запустить — fail (модуль не подключён)**

Run: `cargo test -p pi agent::setup`
Expected: FAIL — `agent::setup` не объявлен.

- [ ] **Step 3: Подключить модуль + реальный `HostSys`**

В `crates/bin/src/agent/mod.rs` добавить `pub mod setup;`.

В `setup.rs` (над `#[cfg(test)]`) добавить:

```rust
pub struct HostSys;

#[async_trait]
impl Sys for HostSys {
    async fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
        let out = tokio::process::Command::new(program)
            .args(args)
            .output()
            .await
            .map_err(|e| format!("spawn {program}: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
    fn write(&self, path: &Path, content: &str) -> Result<(), String> {
        std::fs::write(path, content).map_err(|e| format!("write {}: {e}", path.display()))
    }
}
```

- [ ] **Step 4: Запустить тесты**

Run: `cargo test -p pi agent::setup`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bin/src/agent/setup.rs crates/bin/src/agent/mod.rs
git commit -m "feat(agent-setup): Sys abstraction with HostSys and id helpers"
```

---

### Task 9: Канонические шаблоны + запись юнита с бэкапом

**Files:**
- Modify: `crates/bin/src/agent/setup.rs`
- Test: `crates/bin/src/agent/setup.rs`

**Interfaces:**
- Consumes: `Sys` (Task 8).
- Produces: `const UNIT: &str`; `const AGENT_TOML: &str`; `const UNIT_PATH/AGENT_TOML_PATH: &str`; `enum WriteAction { Wrote, Skipped, BackedUp }`; `fn write_unit_with_backup(&dyn Sys, dry_run: bool) -> Result<WriteAction, String>`.

- [ ] **Step 1: Failing-тест: идентичный юнит → Skipped; отличный → BackedUp**

Добавить в `mod tests`:

```rust
#[test]
fn unit_template_matches_spec_byte_for_byte() {
    assert!(UNIT.starts_with("[Unit]\nDescription=pi deploy agent\n"));
    assert!(UNIT.contains("ExecStart=/usr/local/bin/pi agent run --config /etc/pi/agent.toml\n"));
    assert!(UNIT.contains("Environment=XDG_CACHE_HOME=/var/lib/pi/.cache\n"));
    assert!(UNIT.ends_with("WantedBy=multi-user.target\n"));
}

#[tokio::test]
async fn write_unit_skips_when_identical() {
    let mut sys = FakeSys::default();
    sys.paths.insert(UNIT_PATH.into());
    sys.files.insert(UNIT_PATH.into(), UNIT.into());
    let action = write_unit_with_backup(&sys, false).unwrap();
    assert!(matches!(action, WriteAction::Skipped));
    assert!(sys.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn write_unit_backs_up_when_different() {
    let mut sys = FakeSys::default();
    sys.paths.insert(UNIT_PATH.into());
    sys.files.insert(UNIT_PATH.into(), "old=unit\n".into());
    let action = write_unit_with_backup(&sys, false).unwrap();
    assert!(matches!(action, WriteAction::BackedUp));
    let writes = sys.writes.lock().unwrap();
    assert!(writes.iter().any(|(p, _)| p.ends_with("pi-agent.service.bak")), "backup written");
    assert!(writes.iter().any(|(p, c)| p == UNIT_PATH && c == UNIT), "canonical written");
}
```

- [ ] **Step 2: Запустить — fail**

Run: `cargo test -p pi agent::setup`
Expected: FAIL — нет `UNIT`/`write_unit_with_backup`.

- [ ] **Step 3: Добавить константы и функцию**

В `setup.rs` (над `#[cfg(test)]`) добавить:

```rust
pub const UNIT_PATH: &str = "/etc/systemd/system/pi-agent.service";
pub const AGENT_TOML_PATH: &str = "/etc/pi/agent.toml";

/// Canonical systemd unit — byte-for-byte the working install (spec §9).
pub const UNIT: &str = "\
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
";

/// Canonical agent.toml — written only when /etc/pi/agent.toml is absent (spec §9).
pub const AGENT_TOML: &str = "\
data_dir = \"/var/lib/pi\"
socket = \"/run/pi/agent.sock\"
port_min = 8000
port_max = 8999
build_concurrency = 1
history_keep = 50

[timeouts]
fetch = \"2m\"
build = \"30m\"
up = \"5m\"

[gc]
disk_threshold_percent = 85
";

pub enum WriteAction {
    Wrote,
    Skipped,
    BackedUp,
}

/// Write the canonical unit; back up to *.bak only if an existing file differs.
pub fn write_unit_with_backup(sys: &dyn Sys, dry_run: bool) -> Result<WriteAction, String> {
    let path = Path::new(UNIT_PATH);
    if sys.exists(path) {
        if sys.read(path).as_deref() == Some(UNIT) {
            return Ok(WriteAction::Skipped);
        }
        if dry_run {
            return Ok(WriteAction::BackedUp);
        }
        let bak = format!("{UNIT_PATH}.bak");
        if let Some(old) = sys.read(path) {
            sys.write(Path::new(&bak), &old)?;
        }
        sys.write(path, UNIT)?;
        return Ok(WriteAction::BackedUp);
    }
    if !dry_run {
        sys.write(path, UNIT)?;
    }
    Ok(WriteAction::Wrote)
}
```

- [ ] **Step 4: Запустить тесты**

Run: `cargo test -p pi agent::setup`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bin/src/agent/setup.rs
git commit -m "feat(agent-setup): canonical unit/agent.toml templates and backup writer"
```

---

### Task 10: Оркестрация `pi agent setup`

**Files:**
- Modify: `crates/bin/src/agent/setup.rs`
- Test: `crates/bin/src/agent/setup.rs`

**Interfaces:**
- Consumes: `Sys`, `user_exists`, `in_group`, templates, `write_unit_with_backup`.
- Produces: `struct SetupOpts { login_user: String, with_cloudflared: bool, dry_run: bool }`; `struct SetupReport { created, skipped, repaired, warnings: Vec<String> }` (+ `fn print(&self)`); `async fn setup(&dyn Sys, &SetupOpts) -> SetupReport`.

- [ ] **Step 1: Failing-тест: свежая установка вызывает нужные команды**

Добавить в `mod tests`:

```rust
fn fresh_sys() -> FakeSys {
    let mut sys = FakeSys::default();
    // user absent, no dirs/files exist; group lookups succeed but show no membership.
    sys.err.insert(FakeSys::key("id", &["-u", "pi-agent"]));
    sys.ok.insert(FakeSys::key("id", &["-nG", "pi-agent"]), "pi-agent".into());
    sys.ok.insert(FakeSys::key("id", &["-nG", "piuser"]), "piuser sudo".into());
    sys.ok.insert(FakeSys::key("docker", &["version", "--format", "{{.Server.Version}}"]), "27.0".into());
    sys.ok.insert(FakeSys::key("docker", &["compose", "version"]), "v2".into());
    sys
}

#[tokio::test]
async fn fresh_install_creates_user_dirs_unit() {
    let sys = fresh_sys();
    let opts = SetupOpts { login_user: "piuser".into(), with_cloudflared: false, dry_run: false };
    let report = setup(&sys, &opts).await;
    let calls = sys.calls();
    assert!(calls.iter().any(|c| c.starts_with("useradd --system")), "creates pi-agent");
    assert!(calls.iter().any(|c| c == "usermod -aG docker pi-agent"));
    assert!(calls.iter().any(|c| c == "usermod -aG pi-agent piuser"));
    assert!(calls.iter().any(|c| c.contains("install -d -o pi-agent -g pi-agent /var/lib/pi")));
    assert!(calls.iter().any(|c| c.contains("install -d -o pi-agent -g pi-agent /var/log/pi")));
    assert!(calls.iter().any(|c| c == "systemctl daemon-reload"));
    assert!(calls.iter().any(|c| c == "systemctl enable --now pi-agent"));
    assert!(report.warnings.is_empty(), "docker present -> no warnings");
}

#[tokio::test]
async fn repairs_only_missing_var_log_pi_on_working_install() {
    let mut sys = FakeSys::default();
    // user exists and is in both groups; all dirs exist EXCEPT /var/log/pi; unit identical.
    sys.ok.insert(FakeSys::key("id", &["-u", "pi-agent"]), "999".into());
    sys.ok.insert(FakeSys::key("id", &["-nG", "pi-agent"]), "pi-agent docker".into());
    sys.ok.insert(FakeSys::key("id", &["-nG", "piuser"]), "piuser sudo docker pi-agent".into());
    sys.ok.insert(FakeSys::key("docker", &["version", "--format", "{{.Server.Version}}"]), "27.0".into());
    sys.ok.insert(FakeSys::key("docker", &["compose", "version"]), "v2".into());
    for p in ["/var/lib/pi", "/etc/pi", UNIT_PATH, AGENT_TOML_PATH] {
        sys.paths.insert(p.into());
    }
    sys.files.insert(UNIT_PATH.into(), UNIT.into());
    sys.files.insert(AGENT_TOML_PATH.into(), AGENT_TOML.into());
    let opts = SetupOpts { login_user: "piuser".into(), with_cloudflared: false, dry_run: false };
    let report = setup(&sys, &opts).await;
    let calls = sys.calls();
    assert!(!calls.iter().any(|c| c.starts_with("useradd")), "user not recreated");
    assert!(!calls.iter().any(|c| c.starts_with("usermod")), "groups untouched");
    assert!(calls.iter().any(|c| c.contains("install -d -o pi-agent -g pi-agent /var/log/pi")));
    assert!(report.repaired.iter().any(|r| r.contains("/var/log/pi")));
    assert!(sys.writes.lock().unwrap().is_empty(), "agent.toml/unit untouched");
}

#[tokio::test]
async fn dry_run_makes_no_changes() {
    let sys = fresh_sys();
    let opts = SetupOpts { login_user: "piuser".into(), with_cloudflared: false, dry_run: true };
    let _ = setup(&sys, &opts).await;
    let calls = sys.calls();
    assert!(calls.iter().all(|c| c.starts_with("id ") || c.starts_with("docker ")), "only probes ran: {calls:?}");
    assert!(sys.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn missing_docker_warns_not_fails() {
    let mut sys = fresh_sys();
    sys.ok.remove(&FakeSys::key("docker", &["version", "--format", "{{.Server.Version}}"]));
    sys.err.insert(FakeSys::key("docker", &["version", "--format", "{{.Server.Version}}"]));
    let opts = SetupOpts { login_user: "piuser".into(), with_cloudflared: false, dry_run: false };
    let report = setup(&sys, &opts).await;
    assert!(report.warnings.iter().any(|w| w.contains("docker")));
}
```

- [ ] **Step 2: Запустить — fail**

Run: `cargo test -p pi agent::setup`
Expected: FAIL — нет `SetupOpts`/`SetupReport`/`setup`.

- [ ] **Step 3: Реализовать `setup`**

В `setup.rs` (над `#[cfg(test)]`) добавить:

```rust
pub struct SetupOpts {
    pub login_user: String,
    pub with_cloudflared: bool,
    pub dry_run: bool,
}

#[derive(Default)]
pub struct SetupReport {
    pub created: Vec<String>,
    pub skipped: Vec<String>,
    pub repaired: Vec<String>,
    pub warnings: Vec<String>,
}

impl SetupReport {
    pub fn print(&self) {
        for c in &self.created { println!("created: {c}"); }
        for r in &self.repaired { println!("repaired: {r}"); }
        for s in &self.skipped { println!("ok (already present): {s}"); }
        for w in &self.warnings { println!("warning: {w}"); }
        if self.repaired.iter().any(|r| r.contains("/var/log/pi")) {
            println!("note: run `sudo systemctl restart pi-agent` to activate file logs");
        }
    }
}

async fn ensure_dir(sys: &dyn Sys, path: &str, owner_group: Option<&str>, dry: bool, rep: &mut SetupReport, repair: bool) {
    if sys.exists(Path::new(path)) {
        rep.skipped.push(path.to_string());
        return;
    }
    if !dry {
        let args: Vec<&str> = match owner_group {
            Some(og) => vec!["-d", "-o", og, "-g", og, path],
            None => vec!["-d", path],
        };
        let _ = sys.run("install", &args).await;
    }
    if repair { rep.repaired.push(path.to_string()); } else { rep.created.push(path.to_string()); }
}

/// Idempotent agent bootstrap (spec §4). Adopt & preserve; never touches
/// secret.key/state.db. Returns a report; does not restart the agent.
pub async fn setup(sys: &dyn Sys, opts: &SetupOpts) -> SetupReport {
    let mut rep = SetupReport::default();
    let dry = opts.dry_run;

    // 1. service user
    if user_exists(sys, "pi-agent").await {
        rep.skipped.push("user pi-agent".into());
    } else {
        if !dry {
            let _ = sys.run("useradd", &["--system", "--no-create-home", "--shell", "/usr/sbin/nologin", "pi-agent"]).await;
        }
        rep.created.push("user pi-agent".into());
    }

    // 2. pi-agent in docker group
    if in_group(sys, "pi-agent", "docker").await {
        rep.skipped.push("pi-agent in docker group".into());
    } else {
        if !dry { let _ = sys.run("usermod", &["-aG", "docker", "pi-agent"]).await; }
        rep.created.push("pi-agent in docker group".into());
    }

    // 3. login user in pi-agent group
    if in_group(sys, &opts.login_user, "pi-agent").await {
        rep.skipped.push(format!("{} in pi-agent group", opts.login_user));
    } else {
        if !dry { let _ = sys.run("usermod", &["-aG", "pi-agent", &opts.login_user]).await; }
        rep.created.push(format!("{} in pi-agent group", opts.login_user));
    }

    // 4-6. directories
    ensure_dir(sys, "/var/lib/pi", Some("pi-agent"), dry, &mut rep, false).await;
    ensure_dir(sys, "/var/log/pi", Some("pi-agent"), dry, &mut rep, true).await; // repair (§2.5)
    ensure_dir(sys, "/etc/pi", None, dry, &mut rep, false).await;

    // 7. agent.toml (only if absent)
    if sys.exists(Path::new(AGENT_TOML_PATH)) {
        rep.skipped.push(AGENT_TOML_PATH.into());
    } else {
        if !dry { let _ = sys.write(Path::new(AGENT_TOML_PATH), AGENT_TOML); }
        rep.created.push(AGENT_TOML_PATH.into());
    }

    // 8. systemd unit + enable
    match write_unit_with_backup(sys, dry) {
        Ok(WriteAction::Skipped) => rep.skipped.push(UNIT_PATH.into()),
        Ok(WriteAction::BackedUp) => rep.repaired.push(format!("{UNIT_PATH} (backed up to .bak)")),
        Ok(WriteAction::Wrote) => rep.created.push(UNIT_PATH.into()),
        Err(e) => rep.warnings.push(format!("unit: {e}")),
    }
    if !dry {
        let _ = sys.run("systemctl", &["daemon-reload"]).await;
        let _ = sys.run("systemctl", &["enable", "--now", "pi-agent"]).await;
    }

    // 9. cloudflared (opt-in) — implemented in Task 13.
    if opts.with_cloudflared {
        cloudflared_bootstrap(sys, dry, &mut rep).await;
    }

    // 10. dependency checks (warn, never fail)
    if sys.run("docker", &["version", "--format", "{{.Server.Version}}"]).await.is_err() {
        rep.warnings.push("docker not available — install Docker Engine and add pi-agent to the docker group".into());
    }
    if sys.run("docker", &["compose", "version"]).await.is_err() {
        rep.warnings.push("docker compose plugin missing — install Docker Compose v2".into());
    }

    rep
}
```

Добавить временную заглушку cloudflared (заменяется в Task 13), чтобы компилировалось:

```rust
async fn cloudflared_bootstrap(_sys: &dyn Sys, _dry: bool, rep: &mut SetupReport) {
    rep.warnings.push("--with-cloudflared not yet implemented".into());
}
```

- [ ] **Step 4: Запустить тесты**

Run: `cargo test -p pi agent::setup`
Expected: PASS (все 4 новых сценария + предыдущие).

- [ ] **Step 5: Commit**

```bash
git add crates/bin/src/agent/setup.rs
git commit -m "feat(agent-setup): idempotent bootstrap orchestration with /var/log/pi repair"
```

---

### Task 11: Команда `pi agent setup` (clap + root/SUDO_USER + печать)

**Files:**
- Modify: `crates/bin/src/agent/setup.rs`, `crates/bin/src/main.rs`
- Test: `crates/bin/src/main.rs`

**Interfaces:**
- Consumes: `setup`, `SetupOpts`, `HostSys`.
- Produces: `async fn run_cmd(user: Option<String>, with_cloudflared: bool, dry_run: bool) -> anyhow::Result<()>`; clap `AgentCmd::Setup`.

- [ ] **Step 1: Реализовать `run_cmd`**

В `setup.rs` добавить:

```rust
/// CLI entrypoint: resolve the login user (--user or $SUDO_USER), run setup,
/// print the report. Must run as root (under sudo) on the Pi.
pub async fn run_cmd(user: Option<String>, with_cloudflared: bool, dry_run: bool) -> anyhow::Result<()> {
    let login_user = user
        .or_else(|| std::env::var("SUDO_USER").ok())
        .filter(|u| !u.is_empty() && u != "root")
        .ok_or_else(|| anyhow::anyhow!(
            "cannot determine the SSH login user; run via `sudo pi agent setup` or pass --user <name>"
        ))?;
    let opts = SetupOpts { login_user, with_cloudflared, dry_run };
    let report = setup(&HostSys, &opts).await;
    report.print();
    if dry_run {
        println!("(dry run — no changes made)");
    }
    Ok(())
}
```

- [ ] **Step 2: Failing-тест clap для `agent setup`**

В `main.rs` `mod tests`:

```rust
#[test]
fn agent_setup_flags_parse() {
    let cli = Cli::try_parse_from(["pi", "agent", "setup", "--user", "piuser", "--dry-run"]).unwrap();
    match cli.cmd {
        Cmd::Agent { cmd: AgentCmd::Setup { user, with_cloudflared, dry_run } } => {
            assert_eq!(user.as_deref(), Some("piuser"));
            assert!(!with_cloudflared);
            assert!(dry_run);
        }
        _ => panic!("expected agent setup"),
    }
}
```

Run: `cargo test -p pi agent_setup_flags_parse`
Expected: FAIL — нет `AgentCmd::Setup`.

- [ ] **Step 3: Добавить вариант и диспатч**

В `main.rs` в `enum AgentCmd` добавить:

```rust
    /// Bootstrap the agent on this Pi (run with sudo; idempotent)
    Setup {
        /// SSH login user to add to the pi-agent group (default: $SUDO_USER)
        #[arg(long)]
        user: Option<String>,
        /// Also bootstrap cloudflared (linger + user unit)
        #[arg(long)]
        with_cloudflared: bool,
        /// Print the plan without changing anything
        #[arg(long)]
        dry_run: bool,
    },
```

В `match cli.cmd` добавить рукав:

```rust
        Cmd::Agent { cmd: AgentCmd::Setup { user, with_cloudflared, dry_run } } => {
            agent::setup::run_cmd(user, with_cloudflared, dry_run).await
        }
```

- [ ] **Step 4: Тест + сборка**

Run: `cargo test -p pi agent_setup_flags_parse && cargo build -p pi`
Expected: PASS + сборка.

- [ ] **Step 5: Commit**

```bash
git add crates/bin/src/agent/setup.rs crates/bin/src/main.rs
git commit -m "feat(agent-setup): wire `pi agent setup` with sudo-user resolution"
```

---

### Task 12: `pi agent uninstall`

**Files:**
- Create: `crates/bin/src/agent/uninstall.rs`
- Modify: `crates/bin/src/agent/mod.rs`, `crates/bin/src/main.rs`
- Test: `crates/bin/src/agent/uninstall.rs`, `crates/bin/src/main.rs`

**Interfaces:**
- Consumes: `Sys`, `HostSys`, `user_exists` (Task 8).
- Produces: `struct UninstallOpts { purge: bool, yes: bool }`; `struct UninstallReport { removed, kept: Vec<String> }`; `async fn uninstall(&dyn Sys, &UninstallOpts) -> UninstallReport`; `async fn run_cmd(purge: bool, yes: bool) -> anyhow::Result<()>`; clap `AgentCmd::Uninstall`.

- [ ] **Step 1: Failing-тест: по умолчанию данные сохраняются, с `--purge` удаляются**

Создать `crates/bin/src/agent/uninstall.rs`:

```rust
use std::path::Path;
use crate::agent::setup::{user_exists, HostSys, Sys, UNIT_PATH};

pub struct UninstallOpts {
    pub purge: bool,
    pub yes: bool,
}

#[derive(Default)]
pub struct UninstallReport {
    pub removed: Vec<String>,
    pub kept: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::setup::{fake::FakeSys, AGENT_TOML_PATH};

    fn installed_sys() -> FakeSys {
        let mut sys = FakeSys::default();
        sys.ok.insert(FakeSys::key("id", &["-u", "pi-agent"]), "999".into());
        for p in ["/var/lib/pi", "/etc/pi", "/var/log/pi", UNIT_PATH, AGENT_TOML_PATH] {
            sys.paths.insert(p.into());
        }
        sys
    }

    #[tokio::test]
    async fn default_keeps_data() {
        let sys = installed_sys();
        let report = uninstall(&sys, &UninstallOpts { purge: false, yes: true }).await;
        let calls = sys.calls();
        assert!(calls.iter().any(|c| c == "systemctl disable --now pi-agent"));
        assert!(calls.iter().any(|c| c == "userdel pi-agent"));
        assert!(!calls.iter().any(|c| c.contains("rm -rf /var/lib/pi")), "data preserved");
        assert!(report.kept.iter().any(|k| k.contains("/var/lib/pi")));
    }

    #[tokio::test]
    async fn purge_removes_data() {
        let sys = installed_sys();
        let report = uninstall(&sys, &UninstallOpts { purge: true, yes: true }).await;
        let calls = sys.calls();
        assert!(calls.iter().any(|c| c.contains("rm -rf /var/lib/pi")));
        assert!(report.removed.iter().any(|r| r.contains("/var/lib/pi")));
    }
}
```

- [ ] **Step 2: Запустить — fail (модуль не подключён)**

Run: `cargo test -p pi agent::uninstall`
Expected: FAIL — `agent::uninstall` не объявлен, нет `uninstall`.

- [ ] **Step 3: Подключить модуль + реализовать `uninstall`/`run_cmd`**

В `crates/bin/src/agent/mod.rs` добавить `pub mod uninstall;`.

В `uninstall.rs` (над `#[cfg(test)]`) добавить:

```rust
/// Remove the agent service/unit/user. Keeps data dirs unless `purge` (spec §5).
pub async fn uninstall(sys: &dyn Sys, opts: &UninstallOpts) -> UninstallReport {
    let mut rep = UninstallReport::default();

    let _ = sys.run("systemctl", &["disable", "--now", "pi-agent"]).await;
    if sys.exists(Path::new(UNIT_PATH)) {
        let _ = sys.run("rm", &["-f", UNIT_PATH, &format!("{UNIT_PATH}.bak")]).await;
        rep.removed.push(UNIT_PATH.into());
    }
    let _ = sys.run("systemctl", &["daemon-reload"]).await;
    if user_exists(sys, "pi-agent").await {
        let _ = sys.run("userdel", &["pi-agent"]).await;
        rep.removed.push("user pi-agent".into());
    }

    if opts.purge {
        for dir in ["/var/lib/pi", "/etc/pi", "/var/log/pi"] {
            if sys.exists(Path::new(dir)) {
                let _ = sys.run("rm", &["-rf", dir]).await;
                rep.removed.push(dir.into());
            }
        }
    } else {
        for dir in ["/var/lib/pi", "/etc/pi", "/var/log/pi"] {
            if sys.exists(Path::new(dir)) {
                rep.kept.push(dir.into());
            }
        }
    }
    rep
}

/// CLI entrypoint: confirm when purging, run uninstall, print the report.
pub async fn run_cmd(purge: bool, yes: bool) -> anyhow::Result<()> {
    if purge && !yes {
        anyhow::bail!(
            "--purge deletes /var/lib/pi (secrets, deploy keys, state) irreversibly. \
             Re-run with --purge --yes to confirm."
        );
    }
    let report = uninstall(&HostSys, &UninstallOpts { purge, yes }).await;
    for r in &report.removed { println!("removed: {r}"); }
    for k in &report.kept { println!("kept: {k}"); }
    if !report.kept.is_empty() {
        println!("note: data kept; re-run with `--purge` to delete it");
    }
    Ok(())
}
```

- [ ] **Step 4: Failing-тест clap + диспатч**

В `main.rs` `mod tests`:

```rust
#[test]
fn agent_uninstall_flags_parse() {
    let cli = Cli::try_parse_from(["pi", "agent", "uninstall", "--purge", "--yes"]).unwrap();
    match cli.cmd {
        Cmd::Agent { cmd: AgentCmd::Uninstall { purge, yes } } => {
            assert!(purge);
            assert!(yes);
        }
        _ => panic!("expected agent uninstall"),
    }
}
```

Run: `cargo test -p pi agent_uninstall_flags_parse`
Expected: FAIL — нет `AgentCmd::Uninstall`.

В `main.rs` `enum AgentCmd` добавить:

```rust
    /// Remove the agent (keeps data unless --purge)
    Uninstall {
        /// Also delete /var/lib/pi, /etc/pi, /var/log/pi (irreversible)
        #[arg(long)]
        purge: bool,
        #[arg(long)]
        yes: bool,
    },
```

В `match cli.cmd`:

```rust
        Cmd::Agent { cmd: AgentCmd::Uninstall { purge, yes } } => {
            agent::uninstall::run_cmd(purge, yes).await
        }
```

- [ ] **Step 5: Тесты + сборка**

Run: `cargo test -p pi agent::uninstall agent_uninstall_flags_parse && cargo build -p pi`
Expected: PASS + сборка.

- [ ] **Step 6: Commit**

```bash
git add crates/bin/src/agent/uninstall.rs crates/bin/src/agent/mod.rs crates/bin/src/main.rs
git commit -m "feat(agent): `pi agent uninstall` keeping data unless --purge"
```

---

### Task 13: cloudflared opt-in (`--with-cloudflared`)

**Files:**
- Modify: `crates/bin/src/agent/setup.rs`
- Test: `crates/bin/src/agent/setup.rs`

**Interfaces:**
- Consumes: `Sys`, `SetupReport`.
- Produces: настоящая `cloudflared_bootstrap` (заменяет заглушку из Task 10): включает linger, пишет user-unit cloudflared, печатает инструкцию по `cloudflared tunnel login`.

- [ ] **Step 1: Failing-тест: linger + инструкция, без флага не трогаем**

Добавить в `mod tests`:

```rust
#[tokio::test]
async fn with_cloudflared_enables_linger_and_instructs() {
    let sys = fresh_sys();
    let opts = SetupOpts { login_user: "piuser".into(), with_cloudflared: true, dry_run: false };
    let report = setup(&sys, &opts).await;
    let calls = sys.calls();
    assert!(calls.iter().any(|c| c == "loginctl enable-linger pi-agent"));
    assert!(report.created.iter().any(|c| c.contains("linger")));
    assert!(report.warnings.iter().any(|w| w.contains("cloudflared tunnel login")), "prints manual login step");
}

#[tokio::test]
async fn without_cloudflared_does_not_touch_linger() {
    let sys = fresh_sys();
    let opts = SetupOpts { login_user: "piuser".into(), with_cloudflared: false, dry_run: false };
    let _ = setup(&sys, &opts).await;
    assert!(!sys.calls().iter().any(|c| c.contains("enable-linger")));
}
```

- [ ] **Step 2: Запустить — fail (заглушка не вызывает linger)**

Run: `cargo test -p pi agent::setup with_cloudflared`
Expected: FAIL — заглушка только пишет warning.

- [ ] **Step 3: Заменить заглушку реальной реализацией**

В `setup.rs` заменить временную `cloudflared_bootstrap` на:

```rust
const CLOUDFLARED_UNIT_PATH: &str = "/var/lib/pi/.config/systemd/user/cloudflared.service";

const CLOUDFLARED_UNIT: &str = "\
[Unit]
Description=cloudflared tunnel (pi-agent)
After=network-online.target

[Service]
ExecStart=/usr/local/bin/cloudflared tunnel run
Restart=on-failure

[Install]
WantedBy=default.target
";

/// Opt-in cloudflared scaffolding: enable linger and write the user unit.
/// The interactive `cloudflared tunnel login` step is left to the operator.
async fn cloudflared_bootstrap(sys: &dyn Sys, dry: bool, rep: &mut SetupReport) {
    if !dry {
        let _ = sys.run("loginctl", &["enable-linger", "pi-agent"]).await;
    }
    rep.created.push("systemd linger for pi-agent".into());

    if sys.exists(Path::new(CLOUDFLARED_UNIT_PATH)) {
        rep.skipped.push(CLOUDFLARED_UNIT_PATH.into());
    } else {
        if !dry {
            let _ = sys.run("install", &["-d", "-o", "pi-agent", "-g", "pi-agent", "/var/lib/pi/.config/systemd/user"]).await;
            let _ = sys.write(Path::new(CLOUDFLARED_UNIT_PATH), CLOUDFLARED_UNIT);
        }
        rep.created.push(CLOUDFLARED_UNIT_PATH.into());
    }
    rep.warnings.push(
        "cloudflared: finish manually — run `cloudflared tunnel login`, create a tunnel, \
         write /var/lib/pi/cloudflared/config.yml, add [cloudflared] to /etc/pi/agent.toml, \
         then `systemctl --user enable --now cloudflared` as pi-agent".into(),
    );
}
```

- [ ] **Step 4: Запустить тесты**

Run: `cargo test -p pi agent::setup`
Expected: PASS (включая `fresh_install_*`, `dry_run_*` без linger).

Примечание: проверить, что тест `dry_run_makes_no_changes` (Task 10) использует `with_cloudflared: false`; иначе linger-вызов не появится, т.к. dry — да, инвариант сохраняется.

- [ ] **Step 5: Commit**

```bash
git add crates/bin/src/agent/setup.rs
git commit -m "feat(agent-setup): opt-in cloudflared linger + user unit scaffolding"
```

---

### Task 14: Документация + версия 0.5.0

**Files:**
- Modify: `README.md`, `docs/install-agent-v0.1.md`, `Cargo.toml`
- Test: ручная сверка + сборка

**Interfaces:**
- Consumes: всё выше.
- Produces: документированные команды; версия `0.5.0`.

- [ ] **Step 1: Поднять версию workspace**

В корневом `Cargo.toml` заменить:

```toml
version = "0.4.0"
```

на:

```toml
version = "0.5.0"
```

- [ ] **Step 2: README — статус и установка**

В `README.md` заменить строку статуса (строки 8-11) на блок, описывающий v0.5: один `sudo pi agent setup` на Пайке, `pi setup`/`pi init` на клиенте; ручная установка из исходников остаётся как fallback. В список «Supported features» добавить пункты `pi setup`, `pi init`, `pi agent setup`, `pi agent uninstall`. Привести порядок строк systemd-юнита в секции «Install `pi-agent`» к §9 (Environment-строки до `WorkingDirectory`, как в шаблоне).

Конкретно: добавить новую секцию после «Build And Install The Binary»:

````markdown
## Quick Setup (v0.5)

On the Pi, after the binary is installed (see above):

```bash
sudo pi agent setup
```

This is idempotent: it creates the `pi-agent` user, directories, the systemd
unit, and `/etc/pi/agent.toml` if missing, repairs `/var/log/pi`, and never
touches `secret.key` or `state.db`. Re-running it is safe. Use `--dry-run` to
preview, `--with-cloudflared` to scaffold cloudflared.

On the developer machine:

```bash
pi setup            # wizard: server profile + SSH key + config.toml
pi init             # wizard: generate pi.toml in the current project
```
````

- [ ] **Step 3: install-agent doc — отметить repair `/var/log/pi`**

В `docs/install-agent-v0.1.md` добавить в конец:

```markdown
## v0.5: setup automation

`pi agent setup` now creates `/var/log/pi` (owner `pi-agent`) automatically, so
the rolling agent logs activate without the manual `install -d` step above.
Re-run `sudo pi agent setup` on an existing install to repair a missing
`/var/log/pi`, then `sudo systemctl restart pi-agent`.
```

- [ ] **Step 4: Сборка и весь тест-набор**

Run: `cargo build -p pi && cargo test -p pi`
Expected: сборка ок; все тесты PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml README.md docs/install-agent-v0.1.md
git commit -m "docs: document v0.5 setup commands; bump version to 0.5.0"
```

---

### Task 15 (опционально): Ctrl+C cancel во время deploy/follow

Низкий приоритет (спека §11). Брать только если не раздувает объём. Если берётся —
вынести в отдельный план `2026-06-17-pi-deploy-cancel-on-ctrlc.md`, т.к. затрагивает
`cli/commands.rs` (deploy/SSE-follow) и не связан с установкой. **По умолчанию — не реализуем в этом плане.**

---

## Self-Review

**1. Spec coverage** (спека §1 «Входит»):
- `pi agent setup` (Adopt & preserve, §4) → Tasks 8–11, 13. ✓
- `pi agent uninstall [--purge]` (§5) → Task 12. ✓
- `pi setup` (профиль + SSH-ключ + config + тест, §6) → Tasks 1, 6, 7. ✓
- `pi init` (детект + генерация + `expose`, §7) → Tasks 3, 4, 5. ✓
- Неинтерактивный режим (флаги/`--yes`) → присутствует в Tasks 4, 5, 7, 11, 12. ✓
- `/var/log/pi` repair (§2.5) → Task 10 (`repairs_only_missing_var_log_pi_on_working_install`). ✓
- never-touch `secret.key`/`state.db` (§2) → Task 10 (`agent.toml`/unit untouched assert), Task 12 (data kept). ✓
- Канон byte-for-byte (§9) → Task 9 (`unit_template_matches_spec_byte_for_byte`). ✓
- cloudflared/linger opt-in (§4) → Task 13 (`without_cloudflared_does_not_touch_linger`). ✓
- Версия 0.5.0, доки (§12) → Task 14. ✓
- Ctrl+C cancel (§11, опц.) → Task 15 (явно опционально). ✓

**2. Placeholder scan:** запрещённых формулировок нет; каждый шаг с кодом содержит полный код. ✓

**3. Type consistency:**
- `Prompter` (text/confirm/select) одинаков в Tasks 2, 4, 7; `select` реально используется — выбор `expose` (Task 4) и SSH-ключа (Task 7), что подключает `keys::detect_ssh_keys` (Task 6) в `pi setup`. Нет «мёртвых» методов трейта. ✓
- `Sys`/`FakeSys` сигнатуры (run/exists/read/write) согласованы Tasks 8–13. ✓
- `SetupOpts { login_user, with_cloudflared, dry_run }` совпадает в Tasks 10, 11. ✓
- `ClientConfig::save_merged(name, profile, make_default)` и `ServerProfile { host, user, key }` совпадают Tasks 1, 7. ✓
- `cloudflared_bootstrap(&dyn Sys, bool, &mut SetupReport)` — заглушка (Task 10) и реализация (Task 13) одной сигнатуры. ✓
- `UNIT_PATH`/`AGENT_TOML_PATH`/`UNIT`/`AGENT_TOML` объявлены в Task 9, переиспользуются в Tasks 10, 12. ✓

**4. Аудит висячих кусков (dead code / unused):**
- Каждый метод трейта `Prompter` вызывается: `text`/`confirm` в Tasks 4–7, `select` — для `expose` (Task 4) и SSH-ключа (Task 7). ✓
- `keys::detect_ssh_keys` подключён в `pi setup` (Task 7 `run`); `pubkey_path`/`generate_key`/`push_pubkey` — там же. ✓
- Все поля структур (`InitFlags`/`InitFields`/`DetectedDefaults`/`SetupFlags`/`SetupOpts`/`SetupReport`/`UninstallOpts`/`UninstallReport`) читаются; все clap-аргументы прокинуты в диспатч; все варианты `WriteAction` конструируются и матчатся. ✓
- `cli::init::run(flags)` — без фантомного `write_extras`. ✓
- Импорты совпадают с использованием: `AGENT_TOML_PATH` в `uninstall.rs` — только в тест-модуле (в основном коде используется лишь `UNIT_PATH`); `ClientConfig` в `cli/setup.rs` импортируется в Step 3, где впервые нужен. `deny(warnings)` в крейте нет, но импорты всё равно чистые. ✓

Замечание для исполнителя: при добавлении вариантов `Cmd::Init`/`Cmd::Setup` обычными (не `#[command(flatten)]`) структурами — существующие тесты `deploy_*`/`server_flag_*` не затрагиваются.
