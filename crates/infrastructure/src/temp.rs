use std::path::{Path, PathBuf};
use std::sync::Arc;

use pi_domain::contracts::TempProbe;

/// Reads CPU temperature from `<root>/sys/class/thermal/thermal_zone*/`.
/// `root` is injected (mirrors `SysinfoDiskProbe::new`) so tests can point at
/// a temp dir with fake zones; production passes `/`.
pub struct ThermalZoneTempProbe {
    root: PathBuf,
}

impl ThermalZoneTempProbe {
    pub fn new(root: &Path) -> Arc<ThermalZoneTempProbe> {
        Arc::new(ThermalZoneTempProbe {
            root: root.to_path_buf(),
        })
    }

    fn read_millideg(dir: &Path) -> Option<f64> {
        let raw = std::fs::read_to_string(dir.join("temp")).ok()?;
        let milli: f64 = raw.trim().parse().ok()?;
        Some(milli / 1000.0)
    }
}

impl TempProbe for ThermalZoneTempProbe {
    fn cpu_celsius(&self) -> Option<f64> {
        let base = self.root.join("sys/class/thermal");
        let mut zone0: Option<PathBuf> = None;
        for entry in std::fs::read_dir(&base).ok()?.flatten() {
            let dir = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("thermal_zone") {
                continue;
            }
            if name == "thermal_zone0" {
                zone0 = Some(dir.clone());
            }
            let zone_type = std::fs::read_to_string(dir.join("type")).unwrap_or_default();
            if zone_type.to_lowercase().contains("cpu") {
                return Self::read_millideg(&dir);
            }
        }
        zone0.and_then(|d| Self::read_millideg(&d))
    }
}

#[cfg(test)]
mod tests {
    use super::{TempProbe, ThermalZoneTempProbe};
    use std::fs;

    fn write_zone(root: &std::path::Path, idx: usize, zone_type: &str, millideg: &str) {
        let dir = root
            .join("sys/class/thermal")
            .join(format!("thermal_zone{idx}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("type"), zone_type).unwrap();
        fs::write(dir.join("temp"), millideg).unwrap();
    }

    #[test]
    fn prefers_the_cpu_zone_and_parses_millidegrees() {
        let root = tempfile::tempdir().unwrap();
        write_zone(root.path(), 0, "gpu-thermal\n", "40000\n");
        write_zone(root.path(), 1, "cpu-thermal\n", "48250\n");
        let probe = ThermalZoneTempProbe::new(root.path());
        assert_eq!(probe.cpu_celsius(), Some(48.25));
    }

    #[test]
    fn falls_back_to_zone0_when_no_cpu_zone() {
        let root = tempfile::tempdir().unwrap();
        write_zone(root.path(), 0, "soc\n", "55000\n");
        let probe = ThermalZoneTempProbe::new(root.path());
        assert_eq!(probe.cpu_celsius(), Some(55.0));
    }

    #[test]
    fn none_when_tree_absent() {
        let root = tempfile::tempdir().unwrap();
        let probe = ThermalZoneTempProbe::new(root.path());
        assert_eq!(probe.cpu_celsius(), None);
    }
}
