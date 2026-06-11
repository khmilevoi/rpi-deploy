use std::path::{Path, PathBuf};

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

impl AgentConfig {
    pub fn parse(text: &str) -> anyhow::Result<AgentConfig> {
        Ok(toml::from_str(text)?)
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
}
