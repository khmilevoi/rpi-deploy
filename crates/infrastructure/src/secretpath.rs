//! Validation of secret-file relative paths (secrets spec §3, §7). Shared by
//! the rpi.toml parser (CLI), the agent PUT handler and the workdir writer,
//! so anything accepted client-side is accepted server-side and vice versa.

use std::path::{Path, PathBuf};

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

/// Resolves `root.join(rel)` to its canonical real path and confirms the
/// result is still inside the canonical `root`. This is the read-side
/// counterpart to the write-side symlink guards in `secretsfile.rs`:
/// `validate_rel_path` only inspects the path *string* (rejects `..`,
/// absolute paths, etc.) and cannot see that a path component is, on disk,
/// a symlink pointing anywhere on the filesystem — `canonicalize` resolves
/// every symlink along the way, so any escape (at any component, including
/// the leaf) shows up as the result no longer being prefixed by `root`.
///
/// Callers should still call `validate_rel_path` first for a fast,
/// friendly error message at config-parse time; this function is the
/// defense-in-depth check performed right before a file is actually
/// opened for reading.
///
/// A missing path (any component) surfaces as `io::ErrorKind::NotFound`,
/// same as `fs::canonicalize`, so callers can keep treating "missing" and
/// "escapes the root" as distinct outcomes.
pub fn resolve_within_root(root: &Path, rel: &str) -> std::io::Result<PathBuf> {
    let root_real = std::fs::canonicalize(root)?;
    let real = std::fs::canonicalize(root_real.join(rel))?;
    if !real.starts_with(&root_real) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("'{rel}' escapes the project root (symlink?)"),
        ));
    }
    Ok(real)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_and_nested_forward_slash_paths() {
        for p in [
            "certs/server.pem",
            ".env.production",
            "a/b/c.txt",
            "config.json",
        ] {
            assert!(validate_rel_path(p).is_ok(), "{p}");
        }
    }

    #[test]
    fn rejects_escapes_absolutes_and_platform_specifics() {
        for p in [
            "",                  // empty
            "/etc/passwd",       // absolute
            "../outside",        // parent escape
            "a/../b",            // nested escape
            "./a",               // current-dir component
            "a//b",              // empty component
            "a/",                // trailing slash
            r"certs\server.pem", // backslash (Windows separator)
            "C:/x",              // drive letter
            "a\0b",              // NUL
        ] {
            assert!(validate_rel_path(p).is_err(), "{p:?} must be rejected");
        }
    }

    #[test]
    fn resolve_within_root_accepts_plain_nested_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("certs")).unwrap();
        std::fs::write(dir.path().join("certs/server.pem"), b"PEM").unwrap();

        let resolved = resolve_within_root(dir.path(), "certs/server.pem").unwrap();
        assert_eq!(std::fs::read(&resolved).unwrap(), b"PEM");
    }

    #[test]
    fn resolve_within_root_missing_file_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_within_root(dir.path(), "nope.txt").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_within_root_rejects_symlinked_file_escaping_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"top-secret").unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("leak.txt")).unwrap();

        let err = resolve_within_root(&root, "leak.txt").unwrap_err();
        assert_ne!(
            err.kind(),
            std::io::ErrorKind::NotFound,
            "an escape must not be reported as a plain missing file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_within_root_rejects_symlinked_directory_escaping_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("a.pem"), b"top-secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("certs")).unwrap();

        let err = resolve_within_root(&root, "certs/a.pem").unwrap_err();
        assert_ne!(
            err.kind(),
            std::io::ErrorKind::NotFound,
            "an escape must not be reported as a plain missing file"
        );
    }
}
