use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct ClientConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default)]
    pub servers: HashMap<String, ServerProfile>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerProfile {
    pub host: String,
    pub user: String,
    #[serde(skip_serializing_if = "Option::is_none")]
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

    /// Insert or replace a profile; set it as `default` only when asked and no
    /// default exists yet (Adopt & preserve).
    pub fn upsert(&mut self, name: &str, profile: ServerProfile, make_default: bool) {
        self.servers.insert(name.to_string(), profile);
        if make_default && self.default.is_none() {
            self.default = Some(name.to_string());
        }
    }

    /// Load the existing config (or empty), upsert the profile, write it back.
    pub fn save_merged(
        name: &str,
        profile: ServerProfile,
        make_default: bool,
    ) -> anyhow::Result<PathBuf> {
        let path = ClientConfig::path()?;
        let mut cfg = match std::fs::read_to_string(&path) {
            Ok(text) => ClientConfig::parse(&text)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ClientConfig {
                default: None,
                servers: HashMap::new(),
            },
            Err(e) => return Err(anyhow::anyhow!("cannot read {}: {e}", path.display())),
        };
        cfg.upsert(name, profile, make_default);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, toml::to_string(&cfg)?)?;
        Ok(path)
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

    #[test]
    fn save_merged_preserves_other_profiles_and_default() {
        let existing = r#"
default = "home"

[servers.home]
host = "pihost.local"
user = "piuser"
key = "~/.ssh/pi"
"#;
        let mut cfg = ClientConfig::parse(existing).unwrap();
        cfg.upsert(
            "work",
            ServerProfile {
                host: "10.0.0.2".into(),
                user: "deploy".into(),
                key: None,
            },
            false,
        );
        let rendered = toml::to_string(&cfg).unwrap();
        let reparsed = ClientConfig::parse(&rendered).unwrap();
        assert_eq!(
            reparsed.default.as_deref(),
            Some("home"),
            "default preserved"
        );
        assert_eq!(reparsed.servers.len(), 2);
        assert_eq!(reparsed.servers["home"].host, "pihost.local");
        assert_eq!(reparsed.servers["work"].host, "10.0.0.2");
        assert_eq!(reparsed.servers["work"].key, None);
    }

    #[test]
    fn upsert_sets_default_only_when_requested_and_absent() {
        let mut cfg = ClientConfig {
            default: None,
            servers: Default::default(),
        };
        cfg.upsert(
            "home",
            ServerProfile {
                host: "h".into(),
                user: "u".into(),
                key: None,
            },
            true,
        );
        assert_eq!(cfg.default.as_deref(), Some("home"));
        cfg.upsert(
            "work",
            ServerProfile {
                host: "h2".into(),
                user: "u".into(),
                key: None,
            },
            true,
        );
        assert_eq!(
            cfg.default.as_deref(),
            Some("home"),
            "existing default not overwritten"
        );
    }
}
