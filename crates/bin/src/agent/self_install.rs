use std::fs;
use std::io;
use std::path::Path;

/// Canonical agent binary path — must match ExecStart in setup::UNIT.
pub const AGENT_BIN_PATH: &str = "/usr/local/bin/rpi";

#[derive(Debug, PartialEq, Eq)]
pub enum SelfInstallAction {
    /// The running binary already is the canonical file — nothing to do.
    AlreadyCanonical,
    /// The canonical binary is byte-identical — nothing to do.
    UpToDate,
    /// The canonical binary was (or, in dry-run, would be) written.
    Installed,
}

/// Copy `current` (the running binary) over `target` when they differ.
/// Atomic: write `<target dir>/.rpi.tmp`, chmod 0755, rename over target.
pub fn ensure_installed(
    current: &Path,
    target: &Path,
    dry_run: bool,
) -> Result<SelfInstallAction, String> {
    if is_same_file(current, target) {
        return Ok(SelfInstallAction::AlreadyCanonical);
    }
    let cur_bytes = fs::read(current).map_err(|e| format!("read {}: {e}", current.display()))?;
    match fs::read(target) {
        Ok(t) if t == cur_bytes => return Ok(SelfInstallAction::UpToDate),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("read {}: {e}", target.display())),
    }
    if dry_run {
        return Ok(SelfInstallAction::Installed);
    }
    let dir = target
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", target.display()))?;
    let tmp = dir.join(".rpi.tmp");
    fs::write(&tmp, &cur_bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod {}: {e}", tmp.display()))?;
    }
    fs::rename(&tmp, target)
        .map_err(|e| format!("rename {} -> {}: {e}", tmp.display(), target.display()))?;
    Ok(SelfInstallAction::Installed)
}

/// True when both paths resolve to the same existing file.
fn is_same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_dirs() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("node_modules-rpi");
        let target = dir.path().join("usr-local-bin").join("rpi");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&current, b"binary-v2").unwrap();
        (dir, current, target)
    }

    #[test]
    fn installs_when_target_missing() {
        let (_d, current, target) = setup_dirs();
        let action = ensure_installed(&current, &target, false).unwrap();
        assert_eq!(action, SelfInstallAction::Installed);
        assert_eq!(fs::read(&target).unwrap(), b"binary-v2");
    }

    #[test]
    fn replaces_when_target_differs() {
        let (_d, current, target) = setup_dirs();
        fs::write(&target, b"binary-v1").unwrap();
        let action = ensure_installed(&current, &target, false).unwrap();
        assert_eq!(action, SelfInstallAction::Installed);
        assert_eq!(fs::read(&target).unwrap(), b"binary-v2");
    }

    #[test]
    fn up_to_date_when_identical() {
        let (_d, current, target) = setup_dirs();
        fs::write(&target, b"binary-v2").unwrap();
        let action = ensure_installed(&current, &target, false).unwrap();
        assert_eq!(action, SelfInstallAction::UpToDate);
    }

    #[test]
    fn already_canonical_when_current_is_target() {
        let (_d, _current, target) = setup_dirs();
        fs::write(&target, b"binary-v2").unwrap();
        let action = ensure_installed(&target, &target, false).unwrap();
        assert_eq!(action, SelfInstallAction::AlreadyCanonical);
        assert_eq!(fs::read(&target).unwrap(), b"binary-v2", "file untouched");
    }

    #[test]
    fn dry_run_reports_but_does_not_write() {
        let (_d, current, target) = setup_dirs();
        let action = ensure_installed(&current, &target, true).unwrap();
        assert_eq!(action, SelfInstallAction::Installed);
        assert!(!target.exists(), "dry run must not create the target");
    }

    #[test]
    fn no_tmp_file_left_behind() {
        let (_d, current, target) = setup_dirs();
        ensure_installed(&current, &target, false).unwrap();
        assert!(!target.parent().unwrap().join(".rpi.tmp").exists());
    }

    #[cfg(unix)]
    #[test]
    fn installed_binary_is_executable() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, current, target) = setup_dirs();
        ensure_installed(&current, &target, false).unwrap();
        let mode = fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }
}
