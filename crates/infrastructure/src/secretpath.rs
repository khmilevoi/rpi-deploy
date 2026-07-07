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
