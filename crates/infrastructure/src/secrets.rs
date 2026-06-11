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
        let key_path = data_dir.join("secret.key");
        let identity = if key_path.exists() {
            std::fs::read_to_string(&key_path)
                .map_err(secrets_err)?
                .trim()
                .parse::<age::x25519::Identity>()
                .map_err(secrets_err)?
        } else {
            std::fs::create_dir_all(data_dir).map_err(secrets_err)?;
            let identity = age::x25519::Identity::generate();
            write_private(&key_path, identity.to_string().expose_secret().as_bytes())?;
            identity
        };
        let dir = data_dir.join("secrets");
        std::fs::create_dir_all(&dir).map_err(secrets_err)?;
        Ok(Arc::new(EncryptedFileStore { dir, identity }))
    }

    fn bundle_path(&self, project: &str) -> PathBuf {
        self.dir.join(format!("{project}.env.age"))
    }
}

fn write_private(path: &Path, contents: &[u8]) -> Result<(), DomainError> {
    std::fs::write(path, contents).map_err(secrets_err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(secrets_err)?;
    }
    Ok(())
}

#[async_trait]
impl SecretStore for EncryptedFileStore {
    async fn save(&self, project: &str, bundle: &EnvBundle) -> Result<(), DomainError> {
        let plaintext = dotenv::serialize(bundle);
        let ciphertext =
            age::encrypt(&self.identity.to_public(), plaintext.as_bytes()).map_err(secrets_err)?;
        let path = self.bundle_path(project);
        tokio::task::spawn_blocking(move || write_private(&path, &ciphertext))
            .await
            .map_err(|e| secrets_err(format!("join error: {e}")))?
    }

    async fn load(&self, project: &str) -> Result<EnvBundle, DomainError> {
        let ciphertext = match tokio::fs::read(self.bundle_path(project)).await {
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
        b.vars.insert("DB_PASSWORD".into(), "super-secret-value".into());
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

    #[cfg(unix)]
    #[tokio::test]
    async fn key_and_bundle_files_are_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = EncryptedFileStore::open(dir.path()).unwrap();
        store.save("rateme", &bundle()).await.unwrap();
        for file in ["secret.key", "secrets/rateme.env.age"] {
            let mode = std::fs::metadata(dir.path().join(file)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{file}");
        }
    }
}
