//! Atomic, owner-only file writes shared by adapters that persist sensitive
//! data (secret key/bundles, workdir `.env`): files are born `0600` (§10,
//! §17) and replaced via temp + rename so readers never observe a partial
//! write or a permission window.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

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

fn write_temp_private(dir: &Path, prefix: &str, contents: &[u8]) -> std::io::Result<PathBuf> {
    loop {
        let temp_path = dir.join(format!(".{prefix}.{}.tmp", uuid::Uuid::new_v4()));
        match create_private_new(&temp_path) {
            Ok(mut file) => {
                let write_result = file.write_all(contents).and_then(|_| file.sync_all());
                if let Err(e) = write_result {
                    let _ = fs::remove_file(&temp_path);
                    return Err(e);
                }
                return Ok(temp_path);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

fn parent_dir<'p>(path: &'p Path) -> std::io::Result<&'p Path> {
    path.parent().ok_or_else(|| {
        std::io::Error::other(format!("missing parent directory for {}", path.display()))
    })
}

fn temp_prefix(path: &Path, fallback: &'static str) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(fallback)
        .to_string()
}

/// Creates `path` with `0600` only when it does not exist yet; returns
/// Ok(false) when another writer got there first (the existing file wins).
pub(crate) fn write_private_exclusive(path: &Path, contents: &[u8]) -> std::io::Result<bool> {
    let dir = parent_dir(path)?;
    let prefix = temp_prefix(path, "secret");
    let temp_path = write_temp_private(dir, &prefix, contents)?;
    let link_result = fs::hard_link(&temp_path, path);
    let _ = fs::remove_file(&temp_path);
    match link_result {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

/// Replaces `path` with fresh `0600` contents atomically (temp + rename).
pub(crate) fn write_private_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let dir = parent_dir(path)?;
    let prefix = temp_prefix(path, "private");
    let temp_path = write_temp_private(dir, &prefix, contents)?;
    if let Err(e) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(e);
    }
    Ok(())
}
