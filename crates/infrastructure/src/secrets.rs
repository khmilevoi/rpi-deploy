use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use age::secrecy::ExposeSecret;
use async_trait::async_trait;
use pi_domain::contracts::SecretStore;
use pi_domain::entities::EnvBundle;
use pi_domain::error::DomainError;

use crate::dotenv;

fn secrets_err(msg: impl std::fmt::Display) -> DomainError {
    DomainError::Secrets(msg.to_string())
}

/// age-encrypted bundles at <data_dir>/secrets/<project>.env.age; the agent
/// key is generated on first start at <data_dir>/secret.key, 0600 (§10, §17).
pub struct EncryptedFileStore {
    dir: PathBuf,
    identity: age::x25519::Identity,
}

impl EncryptedFileStore {
    pub fn open(data_dir: &Path) -> Result<Arc<EncryptedFileStore>, DomainError> {
        std::fs::create_dir_all(data_dir).map_err(secrets_err)?;
        let key_path = data_dir.join("secret.key");
        let identity = open_or_create_identity(&key_path)?;
        let dir = data_dir.join("secrets");
        std::fs::create_dir_all(&dir).map_err(secrets_err)?;
        Ok(Arc::new(EncryptedFileStore { dir, identity }))
    }

    fn bundle_path(&self, project: &str) -> Result<PathBuf, DomainError> {
        let project = validated_project(project)?;
        Ok(self.dir.join(format!("{project}.env.age")))
    }
}

fn validated_project(project: &str) -> Result<&str, DomainError> {
    if project.is_empty()
        || project == "."
        || project.contains("..")
        || project.contains('/')
        || project.contains('\\')
    {
        return Err(secrets_err(format!("invalid project name: {project:?}")));
    }
    Ok(project)
}

fn read_identity(path: &Path) -> Result<age::x25519::Identity, DomainError> {
    fs::read_to_string(path)
        .map_err(secrets_err)?
        .trim()
        .parse::<age::x25519::Identity>()
        .map_err(secrets_err)
}

fn open_or_create_identity(path: &Path) -> Result<age::x25519::Identity, DomainError> {
    let identity = age::x25519::Identity::generate();
    let contents = identity.to_string();
    if write_private_exclusive(path, contents.expose_secret().as_bytes())? {
        Ok(identity)
    } else {
        read_identity(path)
    }
}

fn create_private_new(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn write_temp_private(dir: &Path, prefix: &str, contents: &[u8]) -> Result<PathBuf, DomainError> {
    loop {
        let temp_path = dir.join(format!(".{prefix}.{}.tmp", uuid::Uuid::new_v4()));
        match create_private_new(&temp_path) {
            Ok(mut file) => {
                let write_result = file.write_all(contents).and_then(|_| file.sync_all());
                if let Err(e) = write_result {
                    let _ = fs::remove_file(&temp_path);
                    return Err(secrets_err(e));
                }
                return Ok(temp_path);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(secrets_err(e)),
        }
    }
}

fn write_private_exclusive(path: &Path, contents: &[u8]) -> Result<bool, DomainError> {
    let dir = path
        .parent()
        .ok_or_else(|| secrets_err(format!("missing parent directory for {}", path.display())))?;
    let prefix = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("secret");
    let temp_path = write_temp_private(dir, prefix, contents)?;
    let link_result = fs::hard_link(&temp_path, path);
    let _ = fs::remove_file(&temp_path);
    match link_result {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(secrets_err(e)),
    }
}

fn write_private_atomic(path: &Path, contents: &[u8]) -> Result<(), DomainError> {
    let dir = path
        .parent()
        .ok_or_else(|| secrets_err(format!("missing parent directory for {}", path.display())))?;
    let prefix = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("bundle");
    let temp_path = write_temp_private(dir, prefix, contents)?;
    if let Err(e) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(secrets_err(e));
    }
    Ok(())
}

#[async_trait]
impl SecretStore for EncryptedFileStore {
    async fn save(&self, project: &str, bundle: &EnvBundle) -> Result<(), DomainError> {
        let plaintext = dotenv::serialize(bundle);
        let ciphertext =
            age::encrypt(&self.identity.to_public(), plaintext.as_bytes()).map_err(secrets_err)?;
        let path = self.bundle_path(project)?;
        tokio::task::spawn_blocking(move || write_private_atomic(&path, &ciphertext))
            .await
            .map_err(|e| secrets_err(format!("join error: {e}")))?
    }

    async fn load(&self, project: &str) -> Result<EnvBundle, DomainError> {
        let ciphertext = match tokio::fs::read(self.bundle_path(project)?).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(EnvBundle::default());
            }
            Err(e) => return Err(secrets_err(e)),
        };
        let plaintext = age::decrypt(&self.identity, &ciphertext).map_err(secrets_err)?;
        let text = String::from_utf8(plaintext).map_err(secrets_err)?;
        dotenv::parse(&text).map_err(secrets_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle() -> EnvBundle {
        let mut b = EnvBundle::default();
        b.vars
            .insert("DB_PASSWORD".into(), "super-secret-value".into());
        b.vars.insert("PORT".into(), "3000".into());
        b
    }

    #[tokio::test]
    async fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::open(dir.path()).unwrap();
        store.save("rateme", &bundle()).await.unwrap();
        assert_eq!(store.load("rateme").await.unwrap(), bundle());
    }

    #[tokio::test]
    async fn load_missing_project_returns_empty_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::open(dir.path()).unwrap();
        assert!(store.load("nope").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn reopened_store_reuses_key_and_decrypts_old_bundles() {
        let dir = tempfile::tempdir().unwrap();
        EncryptedFileStore::open(dir.path())
            .unwrap()
            .save("rateme", &bundle())
            .await
            .unwrap();
        let reopened = EncryptedFileStore::open(dir.path()).unwrap();
        assert_eq!(reopened.load("rateme").await.unwrap(), bundle());
    }

    #[tokio::test]
    async fn bundle_on_disk_is_not_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::open(dir.path()).unwrap();
        store.save("rateme", &bundle()).await.unwrap();
        let raw = std::fs::read(dir.path().join("secrets").join("rateme.env.age")).unwrap();
        let needle = b"super-secret-value";
        assert!(!raw.windows(needle.len()).any(|w| w == needle));
    }

    #[tokio::test]
    async fn invalid_project_names_are_rejected_without_escaping_secrets_dir() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::open(dir.path()).unwrap();

        for project in ["", "..", "../escape", "nested/project", r"nested\project"] {
            let result = store.save(project, &bundle()).await;
            assert!(
                matches!(result, Err(DomainError::Secrets(_))),
                "{project:?}"
            );
        }

        assert!(!dir.path().join("escape.env.age").exists());
        assert!(!dir.path().join("nested").exists());
        assert!(std::fs::read_dir(dir.path().join("secrets"))
            .unwrap()
            .next()
            .is_none());
    }

    #[tokio::test]
    async fn overwriting_bundle_preserves_encryption_and_loads_latest_values() {
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::open(dir.path()).unwrap();
        let mut updated = bundle();
        updated.vars.insert("DB_PASSWORD".into(), "rotated".into());

        store.save("rateme", &bundle()).await.unwrap();
        store.save("rateme", &updated).await.unwrap();

        assert_eq!(store.load("rateme").await.unwrap(), updated);
        let raw = std::fs::read(dir.path().join("secrets").join("rateme.env.age")).unwrap();
        for plaintext in [b"super-secret-value".as_slice(), b"rotated".as_slice()] {
            assert!(!raw.windows(plaintext.len()).any(|w| w == plaintext));
        }
    }

    #[test]
    fn opening_with_existing_key_reuses_persisted_identity() {
        let dir = tempfile::tempdir().unwrap();
        let identity = age::x25519::Identity::generate();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(
            dir.path().join("secret.key"),
            identity.to_string().expose_secret().as_bytes(),
        )
        .unwrap();

        let store = EncryptedFileStore::open(dir.path()).unwrap();

        assert_eq!(
            store.identity.to_public().to_string(),
            identity.to_public().to_string()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn key_and_bundle_files_are_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::open(dir.path()).unwrap();
        store.save("rateme", &bundle()).await.unwrap();
        for file in ["secret.key", "secrets/rateme.env.age"] {
            let mode = std::fs::metadata(dir.path().join(file))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{file}");
        }
    }
}
