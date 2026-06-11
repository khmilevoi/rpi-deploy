use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::EnvFileWriter;
use pi_domain::entities::EnvBundle;
use pi_domain::error::DomainError;

use crate::dotenv;
use crate::fsutil;

/// Writes the decrypted bundle as `<workdir>/.env`, 0600 (§10). The file
/// stays in place: compose re-reads it on every `up`/`restart`.
pub struct FsEnvFileWriter;

impl FsEnvFileWriter {
    pub fn new() -> Arc<FsEnvFileWriter> {
        Arc::new(FsEnvFileWriter)
    }
}

#[async_trait]
impl EnvFileWriter for FsEnvFileWriter {
    async fn write(&self, workdir: &Path, bundle: &EnvBundle) -> Result<(), DomainError> {
        if !workdir.is_dir() {
            return Err(DomainError::NotFound(format!(
                "workdir {} does not exist; deploy the project first",
                workdir.display()
            )));
        }
        let path = workdir.join(".env");
        let contents = dotenv::serialize(bundle);
        // Atomic replace: the file is born 0600 (no window with default
        // umask perms) and compose never sees a partial write.
        tokio::task::spawn_blocking(move || {
            fsutil::write_private_atomic(&path, contents.as_bytes())
        })
        .await
        .map_err(|e| DomainError::Storage(format!("write .env: join error: {e}")))?
        .map_err(|e| DomainError::Storage(format!("write .env: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle() -> EnvBundle {
        let mut b = EnvBundle::default();
        b.vars.insert("A".into(), "1".into());
        b
    }

    #[tokio::test]
    async fn writes_env_file_into_existing_workdir() {
        let dir = tempfile::tempdir().unwrap();
        FsEnvFileWriter::new()
            .write(dir.path(), &bundle())
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".env")).unwrap(),
            "A=1\n"
        );
    }

    #[tokio::test]
    async fn rewriting_env_file_replaces_contents() {
        let dir = tempfile::tempdir().unwrap();
        let writer = FsEnvFileWriter::new();
        writer.write(dir.path(), &bundle()).await.unwrap();
        let mut updated = bundle();
        updated.vars.insert("B".into(), "2".into());
        writer.write(dir.path(), &updated).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".env")).unwrap(),
            "A=1\nB=2\n"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn env_file_is_0600_even_when_replacing_a_wider_one() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "OLD=1\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        FsEnvFileWriter::new()
            .write(dir.path(), &bundle())
            .await
            .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[tokio::test]
    async fn missing_workdir_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = FsEnvFileWriter::new()
            .write(&dir.path().join("never-deployed"), &bundle())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::NotFound(_)), "got: {err}");
    }
}
