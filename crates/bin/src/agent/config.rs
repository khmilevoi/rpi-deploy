use std::path::{Path, PathBuf};

use crate::duration::parse_duration_secs;
use pi_domain::entities::StageTimeouts;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    #[serde(default = "default_socket")]
    #[cfg_attr(not(unix), allow(dead_code))]
    pub socket: PathBuf,
    pub tcp: Option<String>,
    #[serde(default = "default_port_min")]
    pub port_min: u16,
    #[serde(default = "default_port_max")]
    pub port_max: u16,
    pub cloudflared: Option<CloudflaredSection>,
    #[serde(default = "default_build_concurrency")]
    pub build_concurrency: usize,
    #[serde(default = "default_history_keep")]
    pub history_keep: usize,
    #[serde(default)]
    pub timeouts: TimeoutsSection,
    #[serde(default)]
    pub gc: GcSection,
}

/// [timeouts] in agent.toml — agent-wide stage timeout defaults (§8.1).
#[derive(Debug, Default, Deserialize)]
pub struct TimeoutsSection {
    pub fetch: Option<String>,
    pub build: Option<String>,
    pub up: Option<String>,
}

/// [gc] in agent.toml (§8.1).
#[derive(Debug, Deserialize)]
pub struct GcSection {
    #[serde(default = "default_disk_threshold")]
    pub disk_threshold_percent: u8,
}

impl Default for GcSection {
    fn default() -> GcSection {
        GcSection {
            disk_threshold_percent: default_disk_threshold(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CloudflaredSection {
    /// Path to the locally-managed cloudflared config.yml (§11).
    pub config: PathBuf,
    /// Tunnel name for `cloudflared tunnel route dns`.
    pub tunnel: String,
    /// Command applying the config; no sudo needed under linger (§11).
    #[serde(default = "default_restart")]
    pub restart: Vec<String>,
}

fn default_restart() -> Vec<String> {
    ["systemctl", "--user", "restart", "cloudflared"]
        .map(String::from)
        .to_vec()
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("/var/lib/pi")
}
fn default_socket() -> PathBuf {
    PathBuf::from("/run/pi/agent.sock")
}
fn default_port_min() -> u16 {
    8000
}
fn default_port_max() -> u16 {
    8999
}
fn default_build_concurrency() -> usize {
    1
}
fn default_history_keep() -> usize {
    50
}
fn default_disk_threshold() -> u8 {
    85
}

impl AgentConfig {
    pub fn parse(text: &str) -> anyhow::Result<AgentConfig> {
        let config: AgentConfig = toml::from_str(text)?;
        config.stage_timeouts()?;
        Ok(config)
    }

    /// Stage timeout defaults: spec values overridden by [timeouts] (§8.1).
    pub fn stage_timeouts(&self) -> anyhow::Result<StageTimeouts> {
        let mut t = StageTimeouts::default();
        let parse = |field: &str, value: &Option<String>| -> anyhow::Result<Option<u64>> {
            match value {
                Some(s) => parse_duration_secs(s)
                    .map(Some)
                    .map_err(|e| anyhow::anyhow!("agent.toml [timeouts].{field}: {e}")),
                None => Ok(None),
            }
        };
        if let Some(secs) = parse("fetch", &self.timeouts.fetch)? {
            t.fetch_secs = secs;
        }
        if let Some(secs) = parse("build", &self.timeouts.build)? {
            t.build_secs = secs;
        }
        if let Some(secs) = parse("up", &self.timeouts.up)? {
            t.up_secs = secs;
        }
        Ok(t)
    }

    pub fn load(path: Option<&Path>) -> anyhow::Result<AgentConfig> {
        let path = path
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/etc/pi/agent.toml"));
        if path.exists() {
            AgentConfig::parse(&std::fs::read_to_string(&path)?)
        } else {
            AgentConfig::parse("")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_gives_spec_defaults() {
        let config = AgentConfig::parse("").unwrap();
        assert_eq!(config.data_dir, std::path::PathBuf::from("/var/lib/pi"));
        assert_eq!(
            config.socket,
            std::path::PathBuf::from("/run/pi/agent.sock")
        );
        assert!(config.tcp.is_none());
        assert_eq!((config.port_min, config.port_max), (8000, 8999));
    }

    #[test]
    fn tcp_override_for_dev() {
        let config =
            AgentConfig::parse("tcp = \"127.0.0.1:7700\"\ndata_dir = \".dev-data\"").unwrap();
        assert_eq!(config.tcp.as_deref(), Some("127.0.0.1:7700"));
    }

    #[test]
    fn cloudflared_section_parses_with_default_restart() {
        let config = AgentConfig::parse(
            "[cloudflared]\nconfig = \"/var/lib/pi/cloudflared/config.yml\"\ntunnel = \"home\"",
        )
        .unwrap();
        let cf = config.cloudflared.unwrap();
        assert_eq!(cf.tunnel, "home");
        assert_eq!(
            cf.restart,
            vec!["systemctl", "--user", "restart", "cloudflared"]
        );
    }

    #[test]
    fn cloudflared_section_is_optional() {
        assert!(AgentConfig::parse("").unwrap().cloudflared.is_none());
    }

    #[test]
    fn v03_defaults_for_resilience_options() {
        let config = AgentConfig::parse("").unwrap();
        assert_eq!(config.build_concurrency, 1, "build semaphore size (§8.1)");
        assert_eq!(
            config.history_keep, 50,
            "deployments kept per project (§18)"
        );
        assert_eq!(config.gc.disk_threshold_percent, 85, "§8.1");
        let t = config.stage_timeouts().unwrap();
        assert_eq!((t.fetch_secs, t.build_secs, t.up_secs), (120, 1800, 300));
    }

    #[test]
    fn timeouts_section_overrides_defaults_and_is_validated() {
        let config =
            AgentConfig::parse("[timeouts]\nfetch = \"5m\"\nbuild = \"45m\"\nup = \"90s\"")
                .unwrap();
        let t = config.stage_timeouts().unwrap();
        assert_eq!((t.fetch_secs, t.build_secs, t.up_secs), (300, 2700, 90));
        assert!(
            AgentConfig::parse("[timeouts]\nbuild = \"soon\"").is_err(),
            "bad duration must fail at load"
        );
    }

    #[test]
    fn gc_and_concurrency_sections_parse() {
        let config = AgentConfig::parse(
            "build_concurrency = 2\nhistory_keep = 10\n[gc]\ndisk_threshold_percent = 90",
        )
        .unwrap();
        assert_eq!(config.build_concurrency, 2);
        assert_eq!(config.history_keep, 10);
        assert_eq!(config.gc.disk_threshold_percent, 90);
    }
}
