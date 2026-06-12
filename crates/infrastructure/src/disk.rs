use std::path::{Path, PathBuf};
use std::sync::Arc;

use pi_domain::contracts::DiskProbe;
use pi_domain::error::DomainError;
use sysinfo::Disks;

/// Used-space probe for the filesystem holding the agent data dir (§8.1).
pub struct SysinfoDiskProbe {
    path: PathBuf,
}

impl SysinfoDiskProbe {
    pub fn new(path: &Path) -> Arc<SysinfoDiskProbe> {
        Arc::new(SysinfoDiskProbe {
            path: path.to_path_buf(),
        })
    }
}

impl DiskProbe for SysinfoDiskProbe {
    fn used_percent(&self) -> Result<u8, DomainError> {
        let path = normalize_for_mount(
            std::fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone()),
        );
        let disks = Disks::new_with_refreshed_list();
        let disk = disks
            .list()
            .iter()
            .filter(|d| path.starts_with(d.mount_point()))
            .max_by_key(|d| d.mount_point().as_os_str().len())
            .ok_or_else(|| DomainError::Storage(format!("no disk found for {}", path.display())))?;
        let total = disk.total_space();
        if total == 0 {
            return Err(DomainError::Storage("disk reports zero total space".into()));
        }
        let used = total.saturating_sub(disk.available_space());
        Ok(((used * 100) / total) as u8)
    }
}

fn normalize_for_mount(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = path.display().to_string();
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn used_percent_of_current_dir_is_a_sane_percentage() {
        let probe = SysinfoDiskProbe::new(Path::new("."));
        let used = probe.used_percent().unwrap();
        assert!(used <= 100, "got {used}");
    }
}
