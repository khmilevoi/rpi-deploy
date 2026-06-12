use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ClientConfig {
    pub default: Option<String>,
    #[serde(default)]
    pub servers: HashMap<String, ServerProfile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerProfile {
    pub host: String,
    pub user: String,
    pub key: Option<String>,
}

/// Connection selection shared by all remote commands (§16): a profile from
/// the client config (--server / PI_SERVER / default), or a direct
/// --host/--user/--key triple for CI that bypasses the config file entirely.
#[derive(Debug, clap::Args)]
pub struct ConnectOpts {
    /// Server profile from ~/.config/pi/config.toml
    #[arg(long, conflicts_with = "host")]
    pub server: Option<String>,
    /// Direct SSH host (CI mode; the client config file is not read)
    #[arg(long, requires = "user")]
    pub host: Option<String>,
    /// SSH login user for --host
    #[arg(long, requires = "host")]
    pub user: Option<String>,
    /// SSH private key path for --host
    #[arg(long, requires = "host")]
    pub key: Option<String>,
}

impl ConnectOpts {
    pub fn resolve(&self) -> anyhow::Result<ServerProfile> {
        if let (Some(host), Some(user)) = (&self.host, &self.user) {
            return Ok(ServerProfile {
                host: host.clone(),
                user: user.clone(),
                key: self.key.clone(),
            });
        }
        ClientConfig::load()?.select(self.server.as_deref())
    }
}

impl ClientConfig {
    pub fn parse(text: &str) -> anyhow::Result<ClientConfig> {
        Ok(toml::from_str(text)?)
    }

    pub fn path() -> anyhow::Result<PathBuf> {
        Ok(dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("cannot resolve user config dir"))?
            .join("pi")
            .join("config.toml"))
    }

    pub fn load() -> anyhow::Result<ClientConfig> {
        let path = ClientConfig::path()?;
        let text = std::fs::read_to_string(&path).map_err(|e| {
            anyhow::anyhow!(
                "cannot read {}: {e}\ncreate it first (see README.md, section 'Configure A Client Profile')",
                path.display()
            )
        })?;
        ClientConfig::parse(&text)
    }

    pub fn select(&self, flag: Option<&str>) -> anyhow::Result<ServerProfile> {
        let name = flag
            .map(str::to_string)
            .or_else(|| std::env::var("PI_SERVER").ok())
            .or_else(|| self.default.clone())
            .or_else(|| {
                (self.servers.len() == 1)
                    .then(|| self.servers.keys().next().cloned())
                    .flatten()
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no server selected: pass --server, set PI_SERVER, or set `default` in config"
                )
            })?;
        self.servers
            .get(&name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("server profile '{name}' not found in client config"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
default = "home"

[servers.home]
host = "192.168.1.50"
user = "pi"
key = "~/.ssh/id_ed25519"

[servers.work]
host = "10.0.0.2"
user = "deploy"
"#;

    #[test]
    fn select_prefers_flag_over_default() {
        let config = ClientConfig::parse(SAMPLE).unwrap();
        assert_eq!(config.select(Some("work")).unwrap().host, "10.0.0.2");
        assert_eq!(config.select(None).unwrap().host, "192.168.1.50");
    }

    #[test]
    fn select_unknown_profile_is_error() {
        let config = ClientConfig::parse(SAMPLE).unwrap();
        assert!(config.select(Some("nope")).is_err());
    }

    #[test]
    fn connect_opts_with_host_bypass_the_config_file() {
        let opts = ConnectOpts {
            server: None,
            host: Some("203.0.113.7".into()),
            user: Some("pi".into()),
            key: Some("./deploy_key".into()),
        };
        let profile = opts.resolve().unwrap();
        assert_eq!(profile.host, "203.0.113.7");
        assert_eq!(profile.user, "pi");
        assert_eq!(profile.key.as_deref(), Some("./deploy_key"));
    }
}
