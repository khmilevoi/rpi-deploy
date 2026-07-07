# Secret Files (`rpi secrets`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `rpi secrets send` отправляет env-файл и секретные файлы из `[secrets]` в rpi.toml одним зашифрованным бандлом на агент; деплой материализует их в чекауте; `rpi env send`/`rpi env ls` удалены.

**Architecture:** Единая сущность `SecretsBundle { vars, files }` заменяет `EnvBundle` (через временный алиас, чтобы каждая задача компилировалась). Хранение — один age-блоб `<project>.secrets.age` (JSON, файлы в base64) с fallback-чтением старого `<project>.env.age`. Один эндпоинт `PUT/GET /v1/projects/{name}/secrets`. Запись в workdir — через `FsSecretsWriter` с канонизацией родителя против симлинк-эскейпа.

**Tech Stack:** Rust workspace (pi-domain / pi-application / pi-infrastructure / pi), axum 0.8, age 0.11, serde_json, base64 0.22 (новая зависимость), mockall, tempfile.

**Спека:** `docs/superpowers/specs/2026-07-07-secret-files-design.md` — источник требований.

## Global Constraints

- Лимиты: **1 MiB на файл** (`MAX_SECRET_FILE_BYTES = 1024 * 1024`), **8 MiB на бандл** (`MAX_SECRETS_BUNDLE_BYTES = 8 * 1024 * 1024`), axum body limit **12 MiB**. Проверяются и в CLI, и на агенте.
- Пути секретных файлов: только относительные, только прямые слэши, без `..`/`.`/пустых компонентов/`\`/`:`/NUL. Валидация — `pi_infrastructure::secretpath::validate_rel_path`, вызывается в rpitoml-парсере, в агент-хендлере и в `FsSecretsWriter`.
- Права: файлы 0600 (через существующий `fsutil::write_private_atomic`), создаваемые каталоги 0700.
- Обратной совместимости нет: роуты `/v1/projects/{name}/env`, команды `rpi env send|ls`, секция `[env]` — удаляются. `[env]` в rpi.toml — жёсткая ошибка с подсказкой. Fallback только один: агент читает старый `<project>.env.age` при load.
- `pub type EnvBundle = SecretsBundle;` — временный алиас в pi-domain, живёт с Задачи 1 до Задачи 9 (удаление). Новый код пишет `SecretsBundle`.
- Все тесты — in-file `#[cfg(test)] mod tests`, паттерны репо (mockall-моки доменных трейтов, `CollectSink`, tower `oneshot`, tempdir).
- Все shell-команды запускать с префиксом `rtk` (см. CLAUDE.md): `rtk cargo test -p pi-domain`, `rtk git add ...`.
- Сообщения логов/CLI — на английском, в стиле существующих (`saved N key(s) ...`).

---

### Task 1: Домен — `SecretsBundle`

**Files:**
- Modify: `crates/domain/src/entities.rs` (строки 1–28 — `EnvBundle`, тесты 377–408)
- Modify: `crates/bin/src/agent/http.rs:471` (литерал `EnvBundle { vars: req.vars }`)

**Interfaces:**
- Produces: `pi_domain::entities::SecretsBundle { pub vars: BTreeMap<String,String>, pub files: BTreeMap<String,Vec<u8>> }` с методами `is_empty()`, `keys() -> Vec<String>`, `file_paths() -> Vec<String>`; временный алиас `pub type EnvBundle = SecretsBundle;`. Все последующие задачи опираются на эти имена.

- [ ] **Step 1: Написать падающие тесты**

В `crates/domain/src/entities.rs` заменить два теста `env_bundle_*` на:

```rust
#[test]
fn secrets_bundle_default_is_empty_and_keys_are_sorted() {
    let mut bundle = SecretsBundle::default();
    assert!(bundle.is_empty());
    bundle.vars.insert("Z_KEY".into(), "1".into());
    bundle.vars.insert("A_KEY".into(), "2".into());
    assert!(!bundle.is_empty());
    assert_eq!(bundle.keys(), vec!["A_KEY".to_string(), "Z_KEY".to_string()]);
}

#[test]
fn secrets_bundle_with_only_files_is_not_empty() {
    let mut bundle = SecretsBundle::default();
    bundle.files.insert("certs/server.pem".into(), b"PEM".to_vec());
    assert!(!bundle.is_empty());
    assert_eq!(bundle.file_paths(), vec!["certs/server.pem".to_string()]);
    assert!(bundle.keys().is_empty());
}

#[test]
fn secrets_bundle_debug_shows_names_without_values_or_contents() {
    let mut bundle = SecretsBundle::default();
    bundle.vars.insert("API_TOKEN".into(), "raw-token-value".into());
    bundle
        .files
        .insert("certs/server.pem".into(), b"secret-file-body".to_vec());

    let debug = format!("{bundle:?}");

    assert!(debug.contains("SecretsBundle"));
    assert!(debug.contains("API_TOKEN"));
    assert!(debug.contains("certs/server.pem"));
    assert!(!debug.contains("raw-token-value"));
    assert!(!debug.contains("secret-file-body"));
}
```

- [ ] **Step 2: Запустить — убедиться, что падают**

Run: `rtk cargo test -p pi-domain`
Expected: FAIL — `SecretsBundle` not found.

- [ ] **Step 3: Реализация**

Заменить `EnvBundle` (entities.rs:4–28) на:

```rust
/// Project secrets: env vars + secret files (secrets spec 2026-07-07).
/// Values and file contents never leave the agent unmasked.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct SecretsBundle {
    pub vars: BTreeMap<String, String>,
    /// Relative path (forward slashes) -> raw file bytes.
    pub files: BTreeMap<String, Vec<u8>>,
}

impl SecretsBundle {
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty() && self.files.is_empty()
    }

    /// Env key names only (sorted, BTreeMap order) — `rpi secrets ls`.
    pub fn keys(&self) -> Vec<String> {
        self.vars.keys().cloned().collect()
    }

    /// Secret file paths only (sorted) — `rpi secrets ls`.
    pub fn file_paths(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }
}

impl std::fmt::Debug for SecretsBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretsBundle")
            .field("keys", &self.keys())
            .field("files", &self.file_paths())
            .finish()
    }
}

/// Temporary alias while call sites migrate; removed in the cleanup task.
pub type EnvBundle = SecretsBundle;
```

В `crates/bin/src/agent/http.rs:471` литерал перестанет компилироваться (нет поля `files`). Заменить:

```rust
let bundle = EnvBundle {
    vars: req.vars,
    files: std::collections::BTreeMap::new(),
};
```

- [ ] **Step 4: Прогнать тесты**

Run: `rtk cargo test --workspace`
Expected: PASS (весь workspace компилируется через алиас).

- [ ] **Step 5: Commit**

```bash
rtk git add crates/domain/src/entities.rs crates/bin/src/agent/http.rs
rtk git commit -m "feat(domain): SecretsBundle with secret files, EnvBundle alias"
```

---

### Task 2: Инфраструктура — валидация путей `secretpath`

**Files:**
- Create: `crates/infrastructure/src/secretpath.rs`
- Modify: `crates/infrastructure/src/lib.rs` (добавить `pub mod secretpath;`)

**Interfaces:**
- Produces: `pi_infrastructure::secretpath::validate_rel_path(path: &str) -> Result<(), String>`. Используется в Задачах 4 (writer), 6 (агент-хендлер), 7 (rpitoml).

- [ ] **Step 1: Написать падающие тесты**

Создать `crates/infrastructure/src/secretpath.rs` сразу с тестами (реализация — заглушка на следующем шаге):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_and_nested_forward_slash_paths() {
        for p in ["certs/server.pem", ".env.production", "a/b/c.txt", "config.json"] {
            assert!(validate_rel_path(p).is_ok(), "{p}");
        }
    }

    #[test]
    fn rejects_escapes_absolutes_and_platform_specifics() {
        for p in [
            "",                    // empty
            "/etc/passwd",         // absolute
            "../outside",          // parent escape
            "a/../b",              // nested escape
            "./a",                 // current-dir component
            "a//b",                // empty component
            "a/",                  // trailing slash
            r"certs\server.pem",   // backslash (Windows separator)
            "C:/x",                // drive letter
            "a\0b",                // NUL
        ] {
            assert!(validate_rel_path(p).is_err(), "{p:?} must be rejected");
        }
    }
}
```

- [ ] **Step 2: Реализация**

Над тестами в том же файле:

```rust
//! Validation of secret-file relative paths (secrets spec §3, §7). Shared by
//! the rpi.toml parser (CLI), the agent PUT handler and the workdir writer,
//! so anything accepted client-side is accepted server-side and vice versa.

/// Forward-slash relative path: no `..`/`.`, no empty components, no
/// backslashes, drive letters or NUL. Errors name the violated rule.
pub fn validate_rel_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("path is empty".into());
    }
    if path.contains('\0') {
        return Err("path contains NUL".into());
    }
    if path.contains('\\') {
        return Err("use forward slashes, not backslashes".into());
    }
    if path.contains(':') {
        return Err("drive letters / colons are not allowed".into());
    }
    if path.starts_with('/') {
        return Err("path must be relative".into());
    }
    for component in path.split('/') {
        match component {
            "" => return Err("empty path component (double or trailing slash)".into()),
            "." | ".." => return Err("'.' and '..' components are not allowed".into()),
            _ => {}
        }
    }
    Ok(())
}
```

В `crates/infrastructure/src/lib.rs` добавить `pub mod secretpath;` (по алфавиту рядом с `pub mod secrets;`).

- [ ] **Step 3: Прогнать тесты**

Run: `rtk cargo test -p pi-infrastructure secretpath`
Expected: PASS (оба теста).

- [ ] **Step 4: Commit**

```bash
rtk git add crates/infrastructure/src/secretpath.rs crates/infrastructure/src/lib.rs
rtk git commit -m "feat(infra): secret file path validation"
```

---

### Task 3: Инфраструктура — новый формат хранения + legacy fallback

**Files:**
- Modify: `Cargo.toml` (workspace: добавить `base64 = "0.22"`)
- Modify: `crates/infrastructure/Cargo.toml` (добавить `serde` и `base64`)
- Modify: `crates/infrastructure/src/secrets.rs`

**Interfaces:**
- Consumes: `SecretsBundle` (Task 1).
- Produces: `EncryptedFileStore` с прежней сигнатурой `SecretStore` (`save`/`load`/`remove`), но: пишет `<data_dir>/secrets/<project>.secrets.age` (JSON `{vars, files: base64}`), удаляет legacy `<project>.env.age` при save; load читает `.secrets.age`, иначе legacy `.env.age` (только vars), иначе пустой бандл; remove удаляет оба.

- [ ] **Step 1: Зависимости**

В корневом `Cargo.toml` в `[workspace.dependencies]` после `age = "0.11"` добавить:

```toml
base64 = "0.22"
```

В `crates/infrastructure/Cargo.toml` в `[dependencies]` добавить:

```toml
serde = { workspace = true }
base64 = { workspace = true }
```

- [ ] **Step 2: Написать падающие тесты**

В `crates/infrastructure/src/secrets.rs` обновить хелпер и добавить тесты (существующие тесты, проверяющие путь `rateme.env.age`, поменять на `rateme.secrets.age`):

```rust
fn bundle() -> SecretsBundle {
    let mut b = SecretsBundle::default();
    b.vars
        .insert("DB_PASSWORD".into(), "super-secret-value".into());
    b.vars.insert("PORT".into(), "3000".into());
    b.files
        .insert("certs/server.pem".into(), vec![0u8, 159, 146, 150]); // non-UTF8 binary
    b
}

#[tokio::test]
async fn save_load_roundtrips_vars_and_binary_files() {
    let dir = tempfile::tempdir().unwrap();
    let store = EncryptedFileStore::open(dir.path()).unwrap();
    store.save("rateme", &bundle()).await.unwrap();
    assert_eq!(store.load("rateme").await.unwrap(), bundle());
}

#[tokio::test]
async fn load_falls_back_to_legacy_env_age_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let store = EncryptedFileStore::open(dir.path()).unwrap();
    // simulate a pre-secrets agent: dotenv text encrypted at <p>.env.age
    let legacy = age::encrypt(&store.identity.to_public(), b"DB_PASSWORD=old-secret\n").unwrap();
    std::fs::write(dir.path().join("secrets").join("rateme.env.age"), legacy).unwrap();

    let loaded = store.load("rateme").await.unwrap();
    assert_eq!(loaded.vars["DB_PASSWORD"], "old-secret");
    assert!(loaded.files.is_empty());
}

#[tokio::test]
async fn save_removes_legacy_env_age_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = EncryptedFileStore::open(dir.path()).unwrap();
    let legacy_path = dir.path().join("secrets").join("rateme.env.age");
    let legacy = age::encrypt(&store.identity.to_public(), b"A=1\n").unwrap();
    std::fs::write(&legacy_path, legacy).unwrap();

    store.save("rateme", &bundle()).await.unwrap();

    assert!(!legacy_path.exists(), "legacy bundle must be removed");
    assert!(dir.path().join("secrets").join("rateme.secrets.age").exists());
    assert_eq!(store.load("rateme").await.unwrap(), bundle());
}

#[tokio::test]
async fn remove_deletes_both_formats() {
    let dir = tempfile::tempdir().unwrap();
    let store = EncryptedFileStore::open(dir.path()).unwrap();
    store.save("rateme", &bundle()).await.unwrap();
    let legacy = age::encrypt(&store.identity.to_public(), b"A=1\n").unwrap();
    std::fs::write(dir.path().join("secrets").join("rateme.env.age"), legacy).unwrap();

    store.remove("rateme").await.unwrap();

    assert!(std::fs::read_dir(dir.path().join("secrets")).unwrap().next().is_none());
}
```

В тесте `bundle_on_disk_is_not_plaintext` дополнительно проверить, что на диске нет и содержимого файла: needle `b"certs/server.pem"` тоже не должен встречаться (JSON зашифрован целиком). Путь в тестах 0600 — `secrets/rateme.secrets.age`.

- [ ] **Step 3: Запустить — падают**

Run: `rtk cargo test -p pi-infrastructure secrets`
Expected: FAIL (нет поля files в roundtrip, нет fallback).

- [ ] **Step 4: Реализация**

В `crates/infrastructure/src/secrets.rs`:

```rust
use std::collections::BTreeMap;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// On-disk plaintext (before age encryption): JSON with base64 file bodies.
#[derive(Serialize, Deserialize)]
struct StoredBundle {
    vars: BTreeMap<String, String>,
    #[serde(default)]
    files: BTreeMap<String, String>,
}
```

`bundle_path` переименовать логикой в две функции:

```rust
fn bundle_path(&self, project: &str) -> Result<PathBuf, DomainError> {
    let project = validated_project(project)?;
    Ok(self.dir.join(format!("{project}.secrets.age")))
}

fn legacy_path(&self, project: &str) -> Result<PathBuf, DomainError> {
    let project = validated_project(project)?;
    Ok(self.dir.join(format!("{project}.env.age")))
}
```

`save`:

```rust
async fn save(&self, project: &str, bundle: &SecretsBundle) -> Result<(), DomainError> {
    let stored = StoredBundle {
        vars: bundle.vars.clone(),
        files: bundle
            .files
            .iter()
            .map(|(p, b)| (p.clone(), base64::engine::general_purpose::STANDARD.encode(b)))
            .collect(),
    };
    let plaintext = serde_json::to_vec(&stored).map_err(secrets_err)?;
    let ciphertext =
        age::encrypt(&self.identity.to_public(), &plaintext).map_err(secrets_err)?;
    let path = self.bundle_path(project)?;
    let legacy = self.legacy_path(project)?;
    tokio::task::spawn_blocking(move || {
        fsutil::write_private_atomic(&path, &ciphertext).map_err(secrets_err)?;
        match fs::remove_file(&legacy) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(secrets_err(e)),
        }
    })
    .await
    .map_err(|e| secrets_err(format!("join error: {e}")))?
}
```

`load` (fallback-цепочка; импорт `SecretsBundle` вместо `EnvBundle`):

```rust
async fn load(&self, project: &str) -> Result<SecretsBundle, DomainError> {
    match tokio::fs::read(self.bundle_path(project)?).await {
        Ok(ciphertext) => {
            let plaintext = age::decrypt(&self.identity, &ciphertext).map_err(secrets_err)?;
            let stored: StoredBundle =
                serde_json::from_slice(&plaintext).map_err(secrets_err)?;
            let mut files = BTreeMap::new();
            for (path, b64) in stored.files {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&b64)
                    .map_err(secrets_err)?;
                files.insert(path, bytes);
            }
            Ok(SecretsBundle { vars: stored.vars, files })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // pre-secrets agents stored dotenv text at <project>.env.age
            match tokio::fs::read(self.legacy_path(project)?).await {
                Ok(ciphertext) => {
                    let plaintext =
                        age::decrypt(&self.identity, &ciphertext).map_err(secrets_err)?;
                    let text = String::from_utf8(plaintext).map_err(secrets_err)?;
                    dotenv::parse(&text).map_err(secrets_err)
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    Ok(SecretsBundle::default())
                }
                Err(e) => Err(secrets_err(e)),
            }
        }
        Err(e) => Err(secrets_err(e)),
    }
}
```

`remove` — удалить оба пути (каждый с ignore NotFound). Doc-комментарий структуры обновить: `age-encrypted bundles at <data_dir>/secrets/<project>.secrets.age (legacy <project>.env.age is read as fallback and dropped on the next save)`.

Примечание: `dotenv::parse` возвращает `Result<EnvBundle, String>`, где `EnvBundle = SecretsBundle` — files пустые, это и нужно.

- [ ] **Step 5: Прогнать тесты**

Run: `rtk cargo test -p pi-infrastructure secrets`
Expected: PASS все, включая 0600 и path-traversal.

- [ ] **Step 6: Commit**

```bash
rtk git add Cargo.toml crates/infrastructure/Cargo.toml crates/infrastructure/src/secrets.rs
rtk git commit -m "feat(infra): unified secrets bundle storage with legacy env.age fallback"
```

---

### Task 4: `SecretsWriter` — переименование трейта и запись файлов в workdir

**Files:**
- Modify: `crates/domain/src/contracts.rs:160-166` (трейт `EnvFileWriter` → `SecretsWriter`)
- Create: `crates/infrastructure/src/secretsfile.rs` (перенос+расширение `envfile.rs`)
- Delete: `crates/infrastructure/src/envfile.rs`
- Modify: `crates/infrastructure/src/lib.rs` (`pub mod envfile;` → `pub mod secretsfile;`)
- Modify (механически, только имена): `crates/application/src/env.rs`, `crates/application/src/deploy.rs`, `crates/bin/src/agent/state.rs`, `crates/bin/src/agent/http.rs` (тестовый `state_with`)

**Interfaces:**
- Consumes: `SecretsBundle`, `secretpath::validate_rel_path`, `fsutil::write_private_atomic`.
- Produces: трейт `pi_domain::contracts::SecretsWriter { async fn write(&self, workdir: &Path, bundle: &SecretsBundle) -> Result<(), DomainError> }` (+ `MockSecretsWriter` из automock); `pi_infrastructure::secretsfile::FsSecretsWriter::new() -> Arc<FsSecretsWriter>`.

- [ ] **Step 1: Переименовать трейт**

В `crates/domain/src/contracts.rs` заменить блок `EnvFileWriter` на:

```rust
/// Writes the decrypted bundle into the project workdir: `.env` from vars
/// plus each secret file at its relative path (secrets spec §7).
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait SecretsWriter: Send + Sync {
    /// Fails with NotFound when the workdir does not exist (never deployed).
    async fn write(&self, workdir: &Path, bundle: &SecretsBundle) -> Result<(), DomainError>;
}
```

В импортах contracts.rs заменить `EnvBundle` на `SecretsBundle` (строка 8–12). Сигнатуру `SecretStore` тоже перевести на `SecretsBundle` (тип тот же — алиас, меняются только упоминания в сигнатурах/доках).

Механическая замена по четырём файлам (использовать точные строки):
- `EnvFileWriter` → `SecretsWriter`; `MockEnvFileWriter` → `MockSecretsWriter` (`crates/application/src/env.rs`, `crates/application/src/deploy.rs` — импорты, поля `env_files: Arc<dyn ...>`, моки в тестах).
- `FsEnvFileWriter` → `FsSecretsWriter`; `pi_infrastructure::envfile::` → `pi_infrastructure::secretsfile::` (`crates/bin/src/agent/state.rs:20,73`, `crates/bin/src/agent/http.rs` в тестовом `state_with`).

- [ ] **Step 2: Написать падающие тесты writer'а**

Создать `crates/infrastructure/src/secretsfile.rs`. Перенести 4 существующих теста из `envfile.rs` (заменив `FsEnvFileWriter` → `FsSecretsWriter`, `EnvBundle` → `SecretsBundle`) и добавить:

```rust
fn bundle_with_file() -> SecretsBundle {
    let mut b = SecretsBundle::default();
    b.vars.insert("A".into(), "1".into());
    b.files
        .insert("certs/server.pem".into(), vec![0u8, 159, 146, 150]);
    b
}

#[tokio::test]
async fn writes_secret_files_at_relative_paths_creating_dirs() {
    let dir = tempfile::tempdir().unwrap();
    FsSecretsWriter::new()
        .write(dir.path(), &bundle_with_file())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("certs").join("server.pem")).unwrap(),
        vec![0u8, 159, 146, 150]
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".env")).unwrap(),
        "A=1\n"
    );
}

#[tokio::test]
async fn empty_vars_removes_stale_env_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".env"), "OLD=1\n").unwrap();
    let mut b = SecretsBundle::default();
    b.files.insert("secret.txt".into(), b"x".to_vec());
    FsSecretsWriter::new().write(dir.path(), &b).await.unwrap();
    assert!(!dir.path().join(".env").exists(), "stale .env must not survive");
    assert_eq!(std::fs::read(dir.path().join("secret.txt")).unwrap(), b"x");
}

#[tokio::test]
async fn rejects_traversal_paths() {
    let dir = tempfile::tempdir().unwrap();
    let mut b = SecretsBundle::default();
    b.files.insert("../escape.txt".into(), b"x".to_vec());
    let err = FsSecretsWriter::new().write(dir.path(), &b).await.unwrap_err();
    assert!(matches!(err, DomainError::Invalid(_)), "got: {err}");
    assert!(!dir.path().parent().unwrap().join("escape.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_directory_cannot_redirect_writes_outside_workdir() {
    let dir = tempfile::tempdir().unwrap();
    let workdir = dir.path().join("wd");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, workdir.join("link")).unwrap();

    let mut b = SecretsBundle::default();
    b.files.insert("link/leak.txt".into(), b"secret".to_vec());
    let err = FsSecretsWriter::new().write(&workdir, &b).await.unwrap_err();

    assert!(matches!(err, DomainError::Invalid(_)), "got: {err}");
    assert!(!outside.join("leak.txt").exists(), "write escaped the workdir");
}

#[cfg(unix)]
#[tokio::test]
async fn secret_files_and_created_dirs_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    FsSecretsWriter::new()
        .write(dir.path(), &bundle_with_file())
        .await
        .unwrap();
    let file_mode = std::fs::metadata(dir.path().join("certs/server.pem"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(file_mode & 0o777, 0o600);
    let dir_mode = std::fs::metadata(dir.path().join("certs"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(dir_mode & 0o777, 0o700);
}
```

- [ ] **Step 3: Запустить — падают**

Run: `rtk cargo test -p pi-infrastructure secretsfile`
Expected: FAIL — модуля/типа нет.

- [ ] **Step 4: Реализация `FsSecretsWriter`**

`crates/infrastructure/src/secretsfile.rs` (до тестов):

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::SecretsWriter;
use pi_domain::entities::SecretsBundle;
use pi_domain::error::DomainError;

use crate::dotenv;
use crate::fsutil;
use crate::secretpath;

/// Writes the decrypted bundle into `<workdir>`: `.env` (0600, atomic) plus
/// every secret file at its relative path (dirs 0700, files 0600). The parent
/// of each target is canonicalized and must stay inside the canonicalized
/// workdir, so a symlink committed to the repo cannot redirect writes outside
/// (secrets spec §7). Files stay in place: compose re-reads them on `up`.
pub struct FsSecretsWriter;

impl FsSecretsWriter {
    pub fn new() -> Arc<FsSecretsWriter> {
        Arc::new(FsSecretsWriter)
    }
}

fn storage_err(context: String, e: impl std::fmt::Display) -> DomainError {
    DomainError::Storage(format!("{context}: {e}"))
}

fn create_private_dirs(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn write_files_blocking(workdir: PathBuf, files: Vec<(String, Vec<u8>)>) -> Result<(), DomainError> {
    let root = std::fs::canonicalize(&workdir)
        .map_err(|e| storage_err("canonicalize workdir".into(), e))?;
    for (rel, bytes) in files {
        secretpath::validate_rel_path(&rel)
            .map_err(|e| DomainError::Invalid(format!("secret file '{rel}': {e}")))?;
        let target = root.join(&rel);
        let parent = target
            .parent()
            .ok_or_else(|| DomainError::Invalid(format!("secret file '{rel}': no parent")))?;
        create_private_dirs(parent)
            .map_err(|e| storage_err(format!("create dirs for '{rel}'"), e))?;
        let canon_parent = std::fs::canonicalize(parent)
            .map_err(|e| storage_err(format!("canonicalize parent of '{rel}'"), e))?;
        if !canon_parent.starts_with(&root) {
            return Err(DomainError::Invalid(format!(
                "secret file '{rel}' escapes the workdir (symlinked directory?)"
            )));
        }
        let name = target
            .file_name()
            .ok_or_else(|| DomainError::Invalid(format!("secret file '{rel}': empty name")))?;
        fsutil::write_private_atomic(&canon_parent.join(name), &bytes)
            .map_err(|e| storage_err(format!("write secret file '{rel}'"), e))?;
    }
    Ok(())
}

#[async_trait]
impl SecretsWriter for FsSecretsWriter {
    async fn write(&self, workdir: &Path, bundle: &SecretsBundle) -> Result<(), DomainError> {
        if !workdir.is_dir() {
            return Err(DomainError::NotFound(format!(
                "workdir {} does not exist; deploy the project first",
                workdir.display()
            )));
        }
        let env_path = workdir.join(".env");
        if bundle.vars.is_empty() {
            // whole-bundle replace: a stale .env must not survive a resend
            match tokio::fs::remove_file(&env_path).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(storage_err("remove stale .env".into(), e)),
            }
        } else {
            let contents = dotenv::serialize(bundle);
            tokio::task::spawn_blocking(move || {
                fsutil::write_private_atomic(&env_path, contents.as_bytes())
            })
            .await
            .map_err(|e| storage_err("write .env".into(), format!("join error: {e}")))?
            .map_err(|e| storage_err("write .env".into(), e))?;
        }
        if bundle.files.is_empty() {
            return Ok(());
        }
        let root = workdir.to_path_buf();
        let files: Vec<(String, Vec<u8>)> =
            bundle.files.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        tokio::task::spawn_blocking(move || write_files_blocking(root, files))
            .await
            .map_err(|e| storage_err("write secret files".into(), format!("join error: {e}")))?
    }
}
```

Удалить `crates/infrastructure/src/envfile.rs`, обновить `lib.rs`.

- [ ] **Step 5: Прогнать тесты**

Run: `rtk cargo test --workspace`
Expected: PASS — infra-тесты нового writer'а + прежние application/http тесты с переименованными моками.

- [ ] **Step 6: Commit**

```bash
rtk git add -A
rtk git commit -m "feat(infra): SecretsWriter materializes secret files with symlink guard"
```

---

### Task 5: Application — `SendSecrets`/`ListSecrets` + стадия деплоя

**Files:**
- Rename: `crates/application/src/env.rs` → `crates/application/src/secrets.rs`
- Modify: `crates/application/src/lib.rs` (`pub mod env;` → `pub mod secrets;`)
- Modify: `crates/application/src/deploy.rs` (стадия secrets: строки 230–236, поле `env_files` → `secrets_writer`)
- Modify: `crates/bin/src/agent/state.rs`, `crates/bin/src/agent/http.rs` (имена use-case'ов и полей AppState; роуты пока старые)

**Interfaces:**
- Consumes: `SecretsWriter`, `SecretStore`, `SecretsBundle`.
- Produces (для Задачи 6 и 8):
  - `pi_application::secrets::SecretsSaved { pub keys: usize, pub files: usize, pub applied: bool }`
  - `pi_application::secrets::SendSecrets::new(secrets, projects, source, writer, overrides, runtime) -> Arc<SendSecrets>`; `execute(&self, project: &str, bundle: SecretsBundle, apply: bool, log: Arc<dyn LogSink>) -> Result<SecretsSaved, DomainError>`
  - `pi_application::secrets::StoredSecrets { pub keys: Vec<String>, pub files: Vec<String> }`
  - `pi_application::secrets::ListSecrets::new(secrets) -> Arc<ListSecrets>`; `execute(&self, project: &str) -> Result<StoredSecrets, DomainError>`
  - Поля `AppState`: `send_secrets: Arc<SendSecrets>`, `list_secrets: Arc<ListSecrets>` (вместо `send_env`/`env_keys`).

- [ ] **Step 1: Переименовать и написать падающие тесты**

`git mv crates/application/src/env.rs crates/application/src/secrets.rs`, обновить `lib.rs`. В файле: `EnvSaved` → `SecretsSaved` (+ поле `files`), `SendEnv` → `SendSecrets` (поле `env_files` → `writer`), `ListEnvKeys` → `ListSecrets`. `execute`:

```rust
if bundle.is_empty() {
    return Err(DomainError::Invalid("secrets bundle is empty".into()));
}
self.secrets.save(project, &bundle).await?;
let keys = bundle.vars.len();
let files = bundle.files.len();
if !apply {
    return Ok(SecretsSaved { keys, files, applied: false });
}
// ... остальное без изменений, self.writer.write(&workdir, &bundle)
Ok(SecretsSaved { keys, files, applied: true })
```

`ListSecrets::execute`:

```rust
pub async fn execute(&self, project: &str) -> Result<StoredSecrets, DomainError> {
    let bundle = self.secrets.load(project).await?;
    Ok(StoredSecrets {
        keys: bundle.keys(),
        files: bundle.file_paths(),
    })
}
```

Тесты в secrets.rs: обновить `bundle()` — добавить `b.files.insert("certs/server.pem".into(), b"PEM-BODY".to_vec());`; ожидания `SecretsSaved { keys: 2, files: 1, applied: ... }`; `withf` для `writer.expect_write` — `b.vars.len() == 2 && b.files.len() == 1`. Новый тест:

```rust
#[tokio::test]
async fn list_secrets_returns_key_names_and_file_paths_only() {
    let mut secrets = MockSecretStore::new();
    secrets
        .expect_load()
        .withf(|p| p == "rateme")
        .returning(|_| Ok(bundle()));
    let stored = ListSecrets::new(Arc::new(secrets))
        .execute("rateme")
        .await
        .unwrap();
    assert_eq!(stored.keys, vec!["DB_PASSWORD".to_string(), "PORT".to_string()]);
    assert_eq!(stored.files, vec!["certs/server.pem".to_string()]);
}
```

Бандл только из файлов не пустой — тест:

```rust
#[tokio::test]
async fn files_only_bundle_is_saved() {
    let mut m = mocks();
    m.secrets.expect_save().times(1).returning(|_, _| Ok(()));
    let mut b = SecretsBundle::default();
    b.files.insert("id_rsa".into(), b"key".to_vec());
    let saved = build(m)
        .execute("rateme", b, false, CollectSink::new())
        .await
        .unwrap();
    assert_eq!(saved, SecretsSaved { keys: 0, files: 1, applied: false });
}
```

- [ ] **Step 2: Стадия деплоя**

В `crates/application/src/deploy.rs` (строки 230–236) заменить:

```rust
// secrets spec §7: decrypt -> arm masking -> inject .env + secret files
let bundle = self.secrets.load(&config.name).await?;
if !bundle.is_empty() {
    masker.arm(&bundle);
    self.secrets_writer.write(&fetched.workdir, &bundle).await?;
    log.line(&format!(
        "secrets injected ({} keys, {} files)",
        bundle.vars.len(),
        bundle.files.len()
    ));
}
```

Поле структуры `env_files` → `secrets_writer` (объявление, конструктор `new`, все использования). В тестах deploy.rs, где мок писателя настраивается, — обновить имя переменной моков; в тесте happy-path добавить проверку, что при бандле с файлом лог содержит `secrets injected (1 keys, 1 files)` (настроить `m.secrets.expect_load()` вернуть бандл с одним var и одним файлом в одном из существующих тестов с env, например переиспользовать тот, что проверяет `.env injected` — замени ожидание строки).

- [ ] **Step 3: Обновить agent state/http (имена)**

`crates/bin/src/agent/state.rs`: `use pi_application::secrets::{ListSecrets, SendSecrets};`, поля `send_secrets`, `list_secrets`, конструирование `SendSecrets::new(...)`, `ListSecrets::new(secrets)`.
`crates/bin/src/agent/http.rs`: в хендлерах `state.send_secrets.execute(...)` (ответ: `EnvSendResponse { saved_keys: saved.keys, applied: saved.applied }` — поле `saved.files` подключим в Задаче 6), `let stored = state.list_secrets.execute(&name).await...; Ok(Json(EnvKeysResponse { keys: stored.keys }))`. В тестовом `state_with` — те же имена.

- [ ] **Step 4: Прогнать тесты**

Run: `rtk cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add -A
rtk git commit -m "feat(app): SendSecrets/ListSecrets use-cases, deploy injects secret files"
```

---

### Task 6: Агент HTTP — `/v1/projects/{name}/secrets`

**Files:**
- Modify: `crates/bin/src/proto.rs` (новые DTO + константы лимитов)
- Modify: `crates/bin/src/agent/http.rs` (роуты, хендлеры, body limit, тесты)
- Modify: `crates/bin/Cargo.toml` (добавить `base64 = { workspace = true }`)

**Interfaces:**
- Consumes: `SendSecrets`/`ListSecrets` (Task 5), `secretpath` (Task 2).
- Produces (для Задачи 8):
  - `crate::proto::MAX_SECRET_FILE_BYTES: usize = 1024 * 1024`; `MAX_SECRETS_BUNDLE_BYTES: usize = 8 * 1024 * 1024`
  - `SecretsSendRequest { vars: BTreeMap<String,String>, files: BTreeMap<String,String>, apply: bool }` (files: путь → base64)
  - `SecretsSendResponse { saved_keys: usize, saved_files: usize, applied: bool }`
  - `SecretsListResponse { keys: Vec<String>, files: Vec<String> }`
  - Роуты: `PUT/GET /v1/projects/{name}/secrets`; старые `/env` удалены.

- [ ] **Step 1: Написать падающие http-тесты**

В `crates/bin/src/agent/http.rs` заменить четыре `env_*` теста на:

```rust
#[tokio::test]
async fn secrets_send_then_ls_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(state_with(dir.path(), Arc::new(ok_source()), Arc::new(ok_runtime())));

    // "PEM" -> base64 "UEVN"
    let body = serde_json::json!({
        "vars": { "DB_PASSWORD": "hunter2-long" },
        "files": { "certs/server.pem": "UEVN" },
        "apply": false
    });
    let (status, json) = request(app.clone(), put_json("/v1/projects/rateme/secrets", &body)).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["saved_keys"], 1);
    assert_eq!(json["saved_files"], 1);
    assert_eq!(json["applied"], false);

    let (status, json) = request(app.clone(), get_req("/v1/projects/rateme/secrets")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["keys"], serde_json::json!(["DB_PASSWORD"]));
    assert_eq!(json["files"], serde_json::json!(["certs/server.pem"]));
}

#[tokio::test]
async fn secrets_send_rejects_bad_paths_base64_and_oversize() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(state_with(dir.path(), Arc::new(ok_source()), Arc::new(ok_runtime())));

    for bad in [
        serde_json::json!({ "vars": {}, "files": { "../escape": "UEVN" } }),
        serde_json::json!({ "vars": {}, "files": { "/abs/path": "UEVN" } }),
        serde_json::json!({ "vars": {}, "files": { "certs\\win.pem": "UEVN" } }),
        serde_json::json!({ "vars": {}, "files": { "ok.pem": "not-base64!!!" } }),
        serde_json::json!({ "vars": { "BAD-DASH": "x" }, "files": {} }),
        serde_json::json!({ "vars": { "OK": "line1\nline2" }, "files": {} }),
    ] {
        let (status, json) =
            request(app.clone(), put_json("/v1/projects/rateme/secrets", &bad)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} -> {json}");
    }

    use base64::Engine as _;
    let big = base64::engine::general_purpose::STANDARD
        .encode(vec![0u8; crate::proto::MAX_SECRET_FILE_BYTES + 1]);
    let body = serde_json::json!({ "vars": {}, "files": { "big.bin": big } });
    let (status, json) = request(app.clone(), put_json("/v1/projects/rateme/secrets", &body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
}

#[tokio::test]
async fn secrets_apply_for_unknown_project_is_404() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(state_with(dir.path(), Arc::new(ok_source()), Arc::new(ok_runtime())));
    let body = serde_json::json!({ "vars": { "A_KEY": "value-long-enough" }, "files": {}, "apply": true });
    let (status, _) = request(app, put_json("/v1/projects/ghost/secrets", &body)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn secrets_ls_for_unknown_project_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(state_with(dir.path(), Arc::new(ok_source()), Arc::new(ok_runtime())));
    let (status, json) = request(app, get_req("/v1/projects/ghost/secrets")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["keys"], serde_json::json!([]));
    assert_eq!(json["files"], serde_json::json!([]));
}

#[tokio::test]
async fn legacy_env_routes_are_gone() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(state_with(dir.path(), Arc::new(ok_source()), Arc::new(ok_runtime())));
    let body = serde_json::json!({ "vars": { "A": "1" } });
    let (status, _) = request(app.clone(), put_json("/v1/projects/rateme/env", &body)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = request(app, get_req("/v1/projects/rateme/env")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
```

- [ ] **Step 2: Запустить — падают**

Run: `rtk cargo test -p pi secrets_`
Expected: FAIL (роута/DTO нет).

- [ ] **Step 3: Реализация**

`crates/bin/Cargo.toml` — добавить `base64 = { workspace = true }` в `[dependencies]`.

`crates/bin/src/proto.rs` — вместо `EnvSendRequest`/`EnvSendResponse`/`EnvKeysResponse` пока ДОБАВИТЬ (старые остаются до Задачи 8, ими ещё пользуется api.rs):

```rust
/// Secrets bundle limits, enforced by the CLI before upload and re-checked
/// by the agent (secrets spec §2.7). Decoded byte sizes.
pub const MAX_SECRET_FILE_BYTES: usize = 1024 * 1024;
pub const MAX_SECRETS_BUNDLE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretsSendRequest {
    pub vars: BTreeMap<String, String>,
    /// Relative path (forward slashes) -> base64-encoded contents.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub apply: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretsSendResponse {
    pub saved_keys: usize,
    pub saved_files: usize,
    pub applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretsListResponse {
    pub keys: Vec<String>,
    pub files: Vec<String>,
}
```

`crates/bin/src/agent/http.rs`:

1. Роут (вместо `/env`):

```rust
.route(
    "/v1/projects/{name}/secrets",
    put(send_secrets_handler).get(list_secrets_handler),
)
```

2. Body limit — на `Router` перед `.with_state(state)`:

```rust
// base64 inflates the 8 MiB bundle limit by ~4/3; leave headroom
.layer(axum::extract::DefaultBodyLimit::max(12 * 1024 * 1024))
```

3. Хендлеры (вместо `send_env_handler`/`env_keys_handler`; импорты: `SecretsBundle` вместо `EnvBundle`, новые DTO, `base64::Engine as _`):

```rust
async fn send_secrets_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<SecretsSendRequest>,
) -> Result<Json<SecretsSendResponse>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    for (key, value) in &req.vars {
        if !pi_infrastructure::dotenv::is_valid_key(key) {
            return Err(ApiError(DomainError::Invalid(format!(
                "invalid env key '{key}'"
            ))));
        }
        if value.contains('\n') {
            return Err(ApiError(DomainError::Invalid(format!(
                "value of '{key}' contains a newline (multi-line values are unsupported)"
            ))));
        }
    }
    let mut files = std::collections::BTreeMap::new();
    let mut total: usize = 0;
    for (path, b64) in &req.files {
        pi_infrastructure::secretpath::validate_rel_path(path)
            .map_err(|e| ApiError(DomainError::Invalid(format!("secret file '{path}': {e}"))))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|_| {
                ApiError(DomainError::Invalid(format!(
                    "secret file '{path}': contents are not valid base64"
                )))
            })?;
        if bytes.len() > crate::proto::MAX_SECRET_FILE_BYTES {
            return Err(ApiError(DomainError::Invalid(format!(
                "secret file '{path}' is {} bytes; max is 1 MiB",
                bytes.len()
            ))));
        }
        total += bytes.len();
        if total > crate::proto::MAX_SECRETS_BUNDLE_BYTES {
            return Err(ApiError(DomainError::Invalid(
                "secret files exceed 8 MiB total".into(),
            )));
        }
        files.insert(path.clone(), bytes);
    }
    let bundle = SecretsBundle { vars: req.vars, files };
    let saved = state
        .send_secrets
        .execute(&name, bundle, req.apply, Arc::new(TracingSink))
        .await
        .map_err(ApiError)?;
    Ok(Json(SecretsSendResponse {
        saved_keys: saved.keys,
        saved_files: saved.files,
        applied: saved.applied,
    }))
}

async fn list_secrets_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<SecretsListResponse>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    let stored = state.list_secrets.execute(&name).await.map_err(ApiError)?;
    Ok(Json(SecretsListResponse {
        keys: stored.keys,
        files: stored.files,
    }))
}
```

Комментарий у `TracingSink` обновить: `rpi secrets send --apply`.

- [ ] **Step 4: Прогнать тесты**

Run: `rtk cargo test -p pi`
Expected: PASS (в т.ч. `legacy_env_routes_are_gone`). Старые `EnvSend*` DTO остаются в proto.rs без предупреждений — их до Задачи 8 продолжает использовать api.rs.

- [ ] **Step 5: Commit**

```bash
rtk git add -A
rtk git commit -m "feat(agent): unified PUT/GET /v1/projects/{name}/secrets endpoint"
```

---

### Task 7: rpi.toml — секция `[secrets]`, запрет `[env]`, `rpi init`

**Files:**
- Modify: `crates/bin/src/cli/rpitoml.rs`
- Modify: `crates/bin/src/cli/init.rs` (рендер + подсказка + тесты)
- Modify: `crates/bin/src/cli/commands.rs:91` (временный мост в `env_send`, полностью переписывается в Задаче 8)

**Interfaces:**
- Produces (для Задачи 8): `RpiToml.secrets: SecretsSection { pub env: Option<String>, pub files: Vec<String> }` — `env: None` означает «дефолтный `.env`, отсутствие на диске допустимо»; `Some(path)` — файл обязателен.

- [ ] **Step 1: Написать падающие тесты**

В `crates/bin/src/cli/rpitoml.rs`: в `SAMPLE` заменить блок

```
[env]
file = ".env"
```

на

```
[secrets]
env = ".env"
files = ["certs/server.pem"]
```

Тест `env_and_healthcheck_sections_are_parsed_with_defaults` → проверять `parsed.secrets.env.as_deref() == Some(".env")` и `parsed.secrets.files == vec!["certs/server.pem"]`. Тест `missing_env_and_healthcheck_sections_fall_back_to_defaults` → убрать `[secrets]`-блок из SAMPLE строкой-заменой и проверять `parsed.secrets.env.is_none()` и `parsed.secrets.files.is_empty()`. Новые тесты:

```rust
#[test]
fn legacy_env_section_is_a_hard_error_with_migration_hint() {
    let toml = SAMPLE.replace(
        "[secrets]\nenv = \".env\"\nfiles = [\"certs/server.pem\"]\n",
        "[env]\nfile = \".env\"\n",
    );
    let err = RpiToml::parse(&toml).unwrap_err().to_string();
    assert!(err.contains("[env] was replaced by [secrets]"), "got: {err}");
}

#[test]
fn secrets_files_paths_are_validated() {
    for bad in ["../escape", "/abs", r"win\path", "a//b"] {
        let toml = SAMPLE.replace(
            "files = [\"certs/server.pem\"]",
            &format!("files = [\"{}\"]", bad.replace('\\', "\\\\")),
        );
        let err = RpiToml::parse(&toml).unwrap_err().to_string();
        assert!(err.contains("[secrets].files"), "{bad}: {err}");
    }
}

#[test]
fn duplicate_secrets_files_are_rejected() {
    let toml = SAMPLE.replace(
        "files = [\"certs/server.pem\"]",
        "files = [\"a.pem\", \"a.pem\"]",
    );
    let err = RpiToml::parse(&toml).unwrap_err().to_string();
    assert!(err.contains("duplicate"), "got: {err}");
}
```

- [ ] **Step 2: Запустить — падают**

Run: `rtk cargo test -p pi rpitoml`
Expected: FAIL.

- [ ] **Step 3: Реализация**

В `RpiToml` заменить поле `env` на:

```rust
#[serde(default)]
pub secrets: SecretsSection,
/// Legacy [env] table: rejected in parse() with a migration hint. Detected
/// via Option<toml::Value> because serde tolerates unknown sections.
#[serde(default, rename = "env")]
legacy_env: Option<toml::Value>,
```

`EnvSection`/`default_env_file` удалить, добавить:

```rust
/// [secrets] in rpi.toml (secrets spec §3): what `rpi secrets send` reads.
#[derive(Debug, Default, Deserialize)]
pub struct SecretsSection {
    /// Local env file. None -> default ".env" (missing file is fine then);
    /// Some(path) -> the file must exist.
    pub env: Option<String>,
    /// Secret files, relative forward-slash paths (recreated verbatim on the Pi).
    #[serde(default)]
    pub files: Vec<String>,
}
```

В `RpiToml::parse` после проверки `schema`:

```rust
if parsed.legacy_env.is_some() {
    anyhow::bail!(
        "rpi.toml: [env] was replaced by [secrets]; move `file = \"...\"` to:\n[secrets]\nenv = \"...\""
    );
}
let mut seen = std::collections::BTreeSet::new();
for path in &parsed.secrets.files {
    pi_infrastructure::secretpath::validate_rel_path(path)
        .map_err(|e| anyhow::anyhow!("rpi.toml [secrets].files: '{path}': {e}"))?;
    if !seen.insert(path.as_str()) {
        anyhow::bail!("rpi.toml [secrets].files: duplicate path '{path}'");
    }
}
```

Временный мост в `crates/bin/src/cli/commands.rs` (функция `env_send`, строка 91), чтобы задача компилировалась:

```rust
let env_name = rpitoml.secrets.env.clone().unwrap_or_else(|| ".env".to_string());
let env_file = Path::new(&env_name).to_path_buf();
```

В `crates/bin/src/cli/init.rs`:
- рендер (строки 41–44):

```rust
if let Some(env) = &f.env_file {
    let _ = writeln!(s, "\n[secrets]");
    let _ = writeln!(s, "env = {}", toml_str(env));
}
```

- подсказка в `run` (строка 164): `println!("next: `rpi secrets send` (if you use secrets), then `rpi deploy`");`
- тест `render_escapes_backslashes_and_quotes_and_round_trips`: `parsed.env.file` → `parsed.secrets.env.as_deref() == Some("C:\\app\\.env")`; литерал-проверка `file = 'C:\\app\\.env'` → `env = 'C:\\app\\.env'`.

- [ ] **Step 4: Прогнать тесты**

Run: `rtk cargo test -p pi`
Expected: PASS (rpitoml + init + остальное).

- [ ] **Step 5: Commit**

```bash
rtk git add -A
rtk git commit -m "feat(cli): [secrets] section in rpi.toml, [env] is a hard error"
```

---

### Task 8: CLI — `rpi secrets send` / `rpi secrets ls`

**Files:**
- Modify: `crates/bin/src/cli/api.rs` (методы + детект старого агента; удалить `send_env`/`env_keys`)
- Modify: `crates/bin/src/cli/commands.rs` (заменить `env_send`/`env_ls` на `secrets_send`/`secrets_ls` + `collect_secrets`)
- Modify: `crates/bin/src/main.rs` (`Cmd::Env`→`Cmd::Secrets`, `EnvCmd`→`SecretsCmd`, диспатч, clap-тесты)
- Modify: `crates/bin/src/proto.rs` (удалить `EnvSendRequest`/`EnvSendResponse`/`EnvKeysResponse`)

**Interfaces:**
- Consumes: `SecretsSendRequest/Response`, `SecretsListResponse`, `MAX_*`-константы (Task 6), `SecretsSection` (Task 7).
- Produces: `cli::commands::secrets_send(apply: bool, connect: ConnectOpts)`, `cli::commands::secrets_ls(connect: ConnectOpts)`; `ApiClient::send_secrets(project, vars, files, apply)`, `ApiClient::list_secrets(project)`.

- [ ] **Step 1: Написать падающие тесты**

В `crates/bin/src/cli/commands.rs` добавить тесты для `collect_secrets` (в существующий `#[cfg(test)] mod tests`):

```rust
use crate::cli::rpitoml::SecretsSection;

fn section(env: Option<&str>, files: &[&str]) -> SecretsSection {
    SecretsSection {
        env: env.map(str::to_string),
        files: files.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn collect_reads_env_and_files_as_base64() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".env"), "A=1\n").unwrap();
    std::fs::create_dir_all(dir.path().join("certs")).unwrap();
    std::fs::write(dir.path().join("certs/server.pem"), b"PEM").unwrap();

    let (vars, files) =
        collect_secrets(dir.path(), &section(None, &["certs/server.pem"])).unwrap();
    assert_eq!(vars["A"], "1");
    assert_eq!(files["certs/server.pem"], "UEVN"); // base64("PEM")
}

#[test]
fn explicit_env_file_must_exist_but_default_is_optional() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), b"x").unwrap();

    let err = collect_secrets(dir.path(), &section(Some(".env.prod"), &[])).unwrap_err();
    assert!(err.to_string().contains(".env.prod"), "got: {err}");

    let (vars, files) = collect_secrets(dir.path(), &section(None, &["f.txt"])).unwrap();
    assert!(vars.is_empty(), "missing default .env is fine");
    assert_eq!(files.len(), 1);
}

#[test]
fn all_missing_files_are_reported_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let err = collect_secrets(dir.path(), &section(None, &["a.pem", "b.pem"])).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("a.pem") && msg.contains("b.pem"), "got: {msg}");
}

#[test]
fn oversized_file_is_rejected_locally() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("big.bin"),
        vec![0u8; crate::proto::MAX_SECRET_FILE_BYTES + 1],
    )
    .unwrap();
    let err = collect_secrets(dir.path(), &section(None, &["big.bin"])).unwrap_err();
    assert!(err.to_string().contains("1 MiB"), "got: {err}");
}
```

В `crates/bin/src/main.rs` тесты:

```rust
#[test]
fn secrets_commands_parse_and_env_is_gone() {
    let cli = Cli::try_parse_from(["pi", "secrets", "send", "--apply"]).unwrap();
    match cli.cmd {
        Cmd::Secrets { cmd: SecretsCmd::Send { apply, .. } } => assert!(apply),
        _ => panic!("expected secrets send"),
    }
    assert!(Cli::try_parse_from(["pi", "secrets", "ls"]).is_ok());
    assert!(Cli::try_parse_from(["pi", "env", "send"]).is_err(), "env is removed");
}
```

- [ ] **Step 2: Запустить — падают**

Run: `rtk cargo test -p pi`
Expected: FAIL (нет `collect_secrets`, `SecretsCmd`).

- [ ] **Step 3: Реализация**

`crates/bin/src/cli/api.rs` — удалить `send_env`/`env_keys`, импорт `EnvKeysResponse, EnvSendRequest, EnvSendResponse` заменить на `SecretsListResponse, SecretsSendRequest, SecretsSendResponse`; добавить:

```rust
/// Old agents have no /secrets route. axum's bare 404 carries no {"error"}
/// JSON body (every rpi-agent error does), so an error-less 404 means
/// "route not found" -> the agent predates the secrets API.
async fn extract_secrets_error(resp: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        let bytes = resp.bytes().await.unwrap_or_default();
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            if let Some(msg) = v["error"].as_str() {
                anyhow::bail!("{msg}");
            }
        }
        anyhow::bail!("agent does not support the secrets API; update the agent on the Pi");
    }
    extract_error(resp).await
}

pub async fn send_secrets(
    &self,
    project: &str,
    vars: BTreeMap<String, String>,
    files: BTreeMap<String, String>,
    apply: bool,
) -> anyhow::Result<SecretsSendResponse> {
    let req = SecretsSendRequest { vars, files, apply };
    let resp = self
        .http
        .put(format!("{}/v1/projects/{project}/secrets", self.base))
        .json(&req)
        .send()
        .await?;
    Ok(extract_secrets_error(resp).await?.json().await?)
}

pub async fn list_secrets(&self, project: &str) -> anyhow::Result<SecretsListResponse> {
    let resp = self
        .http
        .get(format!("{}/v1/projects/{project}/secrets", self.base))
        .send()
        .await?;
    Ok(extract_secrets_error(resp).await?.json().await?)
}
```

`crates/bin/src/cli/commands.rs` — удалить `env_send`/`env_ls`, добавить (импорт `use crate::cli::rpitoml::SecretsSection;` и `use base64::Engine as _;`):

```rust
pub async fn secrets_send(apply: bool, connect: ConnectOpts) -> anyhow::Result<()> {
    let rpitoml = RpiToml::load(Path::new("rpi.toml"))?;
    let project_name = rpitoml.project.name.clone();
    let (vars, files) = collect_secrets(Path::new("."), &rpitoml.secrets)?;
    if vars.is_empty() && files.is_empty() {
        anyhow::bail!(
            "no secrets to send: env file has no variables and [secrets].files is empty"
        );
    }

    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let (n, m) = (vars.len(), files.len());
    let resp = api.send_secrets(&project_name, vars, files, apply).await?;
    eprintln!("saved {n} key(s) and {m} file(s) for project '{project_name}'");
    if resp.applied {
        eprintln!("secrets applied to running containers");
    }
    Ok(())
}

/// Assemble the outgoing bundle per secrets spec §3: an explicitly configured
/// env file must exist, the default ".env" may be absent; all missing
/// [secrets].files are reported in one error; limits match the agent's.
fn collect_secrets(
    root: &Path,
    section: &SecretsSection,
) -> anyhow::Result<(BTreeMap<String, String>, BTreeMap<String, String>)> {
    let vars = match &section.env {
        Some(name) => {
            let path = root.join(name);
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
            parse_env_file(&raw)?
        }
        None => match std::fs::read_to_string(root.join(".env")) {
            Ok(raw) => parse_env_file(&raw)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(anyhow::anyhow!("cannot read .env: {e}")),
        },
    };

    let mut files = BTreeMap::new();
    let mut missing: Vec<&str> = Vec::new();
    let mut total: usize = 0;
    for rel in &section.files {
        let path = root.join(rel);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                missing.push(rel);
                continue;
            }
            Err(e) => return Err(anyhow::anyhow!("cannot read {}: {e}", path.display())),
        };
        if bytes.len() > crate::proto::MAX_SECRET_FILE_BYTES {
            anyhow::bail!("secret file '{rel}' is {} bytes; max is 1 MiB", bytes.len());
        }
        total += bytes.len();
        if total > crate::proto::MAX_SECRETS_BUNDLE_BYTES {
            anyhow::bail!("secret files exceed 8 MiB total");
        }
        files.insert(
            rel.clone(),
            base64::engine::general_purpose::STANDARD.encode(&bytes),
        );
    }
    if !missing.is_empty() {
        anyhow::bail!(
            "secret file(s) not found: {} (paths are relative to the project root)",
            missing.join(", ")
        );
    }
    Ok((vars, files))
}

pub async fn secrets_ls(connect: ConnectOpts) -> anyhow::Result<()> {
    let rpitoml = RpiToml::load(Path::new("rpi.toml"))?;
    let project_name = rpitoml.project.name.clone();

    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let resp = api.list_secrets(&project_name).await?;
    if resp.keys.is_empty() && resp.files.is_empty() {
        println!("no secrets stored for project '{project_name}'");
        return Ok(());
    }
    if !resp.keys.is_empty() {
        println!("env keys:");
        for key in &resp.keys {
            println!("  {key}");
        }
    }
    if !resp.files.is_empty() {
        println!("files:");
        for file in &resp.files {
            println!("  {file}");
        }
    }
    Ok(())
}
```

Примечание: `SecretsSection.files: Vec<String>` — `missing.join(", ")` требует `Vec<&str>`, как в коде выше.

`crates/bin/src/main.rs` — заменить `Cmd::Env`/`EnvCmd`:

```rust
/// Manage project secrets (env vars + secret files from [secrets] in rpi.toml)
Secrets {
    #[command(subcommand)]
    cmd: SecretsCmd,
},
```

```rust
#[derive(Subcommand)]
enum SecretsCmd {
    /// Send the env file and [secrets].files to the agent (encrypted at rest)
    Send {
        /// Also apply the new secrets to running containers
        #[arg(long)]
        apply: bool,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// List stored env keys and file paths (values are never transmitted)
    Ls {
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
}
```

Диспатч:

```rust
Cmd::Secrets {
    cmd: SecretsCmd::Send { apply, connect },
} => cli::commands::secrets_send(apply, connect).await,
Cmd::Secrets {
    cmd: SecretsCmd::Ls { connect },
} => cli::commands::secrets_ls(connect).await,
```

`crates/bin/src/proto.rs` — удалить `EnvSendRequest`, `EnvSendResponse`, `EnvKeysResponse`.

- [ ] **Step 4: Прогнать тесты**

Run: `rtk cargo test -p pi`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add -A
rtk git commit -m "feat(cli): rpi secrets send/ls replace rpi env send/ls"
```

---

### Task 9: Cleanup — убрать алиас `EnvBundle`, финальная проверка

**Files:**
- Modify: `crates/domain/src/entities.rs` (удалить `pub type EnvBundle = SecretsBundle;`)
- Modify: все оставшиеся упоминания `EnvBundle` (найти греп-ом; ожидаемо: `crates/application/src/mask.rs`, `crates/application/src/logs.rs`, `crates/application/src/deploy.rs` (тесты), `crates/application/src/secrets.rs` (тесты), `crates/infrastructure/src/dotenv.rs`, `crates/domain/src/contracts.rs` (док-комменты), `crates/bin/src/agent/http.rs`)

**Interfaces:**
- Produces: единственное имя `SecretsBundle` во всём workspace.

- [ ] **Step 1: Найти все упоминания**

Run: `rtk grep EnvBundle`
Заменить каждое на `SecretsBundle` (импорты, типы, док-комменты; `dotenv.rs` — параметры/возвраты `parse`/`serialize`; `mask.rs` — параметр `arm`).

- [ ] **Step 2: Удалить алиас**

Убрать строку `pub type EnvBundle = SecretsBundle;` из `crates/domain/src/entities.rs`.

- [ ] **Step 3: Полный прогон**

Run: `rtk cargo test --workspace && rtk cargo clippy --workspace -- -D warnings`
Expected: PASS, ноль предупреждений clippy. Повторный `rtk grep EnvBundle` — пусто.

- [ ] **Step 4: Commit**

```bash
rtk git add -A
rtk git commit -m "refactor: drop temporary EnvBundle alias"
```

---

### Task 10: Документация и миграция

**Files:**
- Modify: `README.md`
- Create: `docs/migration-env-to-secrets.md`

- [ ] **Step 1: README**

Найти все упоминания: `rtk grep -n "env send\|env ls\|\[env\]" README.md`. Заменить команды `rpi env send [--apply]` → `rpi secrets send [--apply]`, `rpi env ls` → `rpi secrets ls`, тексты «send env bundle» → «send secrets (env + files)». Пример rpi.toml в README: блок

```toml
[env]
file = ".env"
```

заменить на

```toml
[secrets]
env = ".env"                     # optional, default ".env"
files = [                        # optional; recreated at the same paths on the Pi
  "certs/server.pem",
]
```

Рядом добавить одно предложение: secret files are sent encrypted, stored age-encrypted on the agent and written into the checkout (0600) on every deploy; paths are relative, forward slashes, `..` is rejected.

- [ ] **Step 2: Миграционная заметка**

Создать `docs/migration-env-to-secrets.md`:

```markdown
# Миграция: `rpi env` → `rpi secrets`

Команды `rpi env send` / `rpi env ls` и секция `[env]` в rpi.toml заменены единым
механизмом секретов: `rpi secrets send` / `rpi secrets ls` и секцией `[secrets]`
(env-файл + произвольные секретные файлы). Хранение на агенте — один
зашифрованный бандл `<project>.secrets.age`.

## Шаги

1. Обновить агент на плате (новый бинарь `rpi`, затем `sudo rpi agent setup` при
   необходимости и `systemctl restart rpi-agent`).
2. Обновить CLI на машине разработчика (тот же релиз, что и агент: старый CLI и
   новый агент несовместимы для секретов, и наоборот).
3. В каждом rpi.toml заменить:

   ```toml
   [env]
   file = ".env"
   ```

   на

   ```toml
   [secrets]
   env = ".env"
   ```

   `[env]` теперь вызывает ошибку парсинга с этой же подсказкой.
4. Пересылать секреты не обязательно: агент читает старый `<project>.env.age`
   как fallback и переводит его в новый формат при первом `rpi secrets send`.
5. Секретные файлы (сертификаты, ключи) добавляются так:

   ```toml
   [secrets]
   env = ".env"
   files = ["certs/server.pem"]
   ```

   после чего `rpi secrets send` — файлы будут созданы по тем же путям в
   чекауте проекта при каждом деплое.
```

- [ ] **Step 3: Commit**

```bash
rtk git add README.md docs/migration-env-to-secrets.md
rtk git commit -m "docs: rpi secrets migration and README update"
```

- [ ] **Step 4: Напоминание вне репозитория**

Сообщить пользователю (не коммитится): скиллы `~/.claude/skills/rpi-cli` и `~/.claude/skills/rpi-toml` описывают `rpi env send` и `[env]` — их нужно обновить под `rpi secrets` / `[secrets]` после мержа.

---

## Верификация фичи целиком (после Task 10)

1. `rtk cargo test --workspace` — зелёный.
2. Локальный агент: `cargo run -p pi -- agent run --config dev/agent.toml`, в другом терминале `$env:PI_AGENT_URL = "http://127.0.0.1:7700"`; в тестовом проекте с `rpi.toml` (`[secrets] files = ["certs/test.pem"]`, файл создать) выполнить `rpi secrets send` → `saved N key(s) and 1 file(s)`; `rpi secrets ls` → ключи + `certs/test.pem`; `rpi env send` → ошибка clap.
3. Проверить на агенте: `<data_dir>/secrets/<project>.secrets.age` существует, старый `.env.age` удалён (если был).
