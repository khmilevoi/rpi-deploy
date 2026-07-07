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

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle() -> SecretsBundle {
        let mut b = SecretsBundle::default();
        b.vars.insert("A".into(), "1".into());
        b
    }

    fn bundle_with_file() -> SecretsBundle {
        let mut b = SecretsBundle::default();
        b.vars.insert("A".into(), "1".into());
        b.files
            .insert("certs/server.pem".into(), vec![0u8, 159, 146, 150]);
        b
    }

    #[tokio::test]
    async fn writes_env_file_into_existing_workdir() {
        let dir = tempfile::tempdir().unwrap();
        FsSecretsWriter::new()
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
        let writer = FsSecretsWriter::new();
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
        FsSecretsWriter::new()
            .write(dir.path(), &bundle())
            .await
            .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[tokio::test]
    async fn missing_workdir_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = FsSecretsWriter::new()
            .write(&dir.path().join("never-deployed"), &bundle())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::NotFound(_)), "got: {err}");
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
}
