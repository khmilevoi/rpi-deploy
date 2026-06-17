use std::path::Path;

use crate::duration::parse_duration_secs;
use pi_domain::entities::{ExposeMode, HealthcheckConfig, ProjectConfig, StageTimeoutOverrides};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct PiToml {
    pub schema: u32,
    pub project: ProjectSection,
    pub source: SourceSection,
    #[serde(default)]
    pub build: BuildSection,
    pub ingress: IngressSection,
    #[serde(default)]
    pub timeouts: TimeoutsSection,
    #[serde(default)]
    pub healthcheck: HealthcheckSection,
    #[serde(default)]
    pub env: EnvSection,
}

#[derive(Debug, Deserialize)]
pub struct ProjectSection {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct SourceSection {
    pub repo: String,
    #[serde(default = "default_branch")]
    pub branch: String,
}

fn default_branch() -> String {
    "main".into()
}

#[derive(Debug, Deserialize)]
pub struct BuildSection {
    #[serde(default = "default_compose")]
    pub compose: String,
}

impl Default for BuildSection {
    fn default() -> BuildSection {
        BuildSection {
            compose: default_compose(),
        }
    }
}

fn default_compose() -> String {
    "docker-compose.yml".into()
}

#[derive(Debug, Deserialize)]
pub struct IngressSection {
    pub hostname: Option<String>,
    pub service: String,
    pub port: u16,
    #[serde(default)]
    pub expose: Option<String>,
}

/// [timeouts] in pi.toml — per-project stage overrides (§12, §8.1).
#[derive(Debug, Default, Deserialize)]
pub struct TimeoutsSection {
    pub fetch: Option<String>,
    pub build: Option<String>,
    pub up: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct HealthcheckSection {
    pub path: Option<String>,
    /// "2xx" | "3xx" | exact code like "204".
    pub expect: Option<String>,
    /// "60s" | "2m" | bare seconds. Default 60s (§22).
    pub timeout: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EnvSection {
    /// Which local file `pi env send` reads (§12).
    #[serde(default = "default_env_file")]
    pub file: String,
}

impl Default for EnvSection {
    fn default() -> EnvSection {
        EnvSection {
            file: default_env_file(),
        }
    }
}

fn default_env_file() -> String {
    ".env".into()
}

fn validate_expect(expect: &str) -> Result<(), String> {
    let ok = matches!(expect, "2xx" | "3xx")
        || (expect.len() == 3 && expect.chars().all(|c| c.is_ascii_digit()));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "invalid [healthcheck].expect '{expect}' (use \"2xx\", \"3xx\" or a code like \"204\")"
        ))
    }
}

impl PiToml {
    pub fn parse(text: &str) -> anyhow::Result<PiToml> {
        let parsed: PiToml = toml::from_str(text)?;
        if parsed.schema != 1 {
            anyhow::bail!(
                "unsupported pi.toml schema {} (this pi supports schema 1)",
                parsed.schema
            );
        }
        if let Some(timeout) = &parsed.healthcheck.timeout {
            parse_duration_secs(timeout)
                .map_err(|e| anyhow::anyhow!("pi.toml [healthcheck]: {e}"))?;
        }
        for (field, value) in [
            ("fetch", &parsed.timeouts.fetch),
            ("build", &parsed.timeouts.build),
            ("up", &parsed.timeouts.up),
        ] {
            if let Some(timeout) = value {
                parse_duration_secs(timeout)
                    .map_err(|e| anyhow::anyhow!("pi.toml [timeouts].{field}: {e}"))?;
            }
        }
        if let Some(expect) = &parsed.healthcheck.expect {
            validate_expect(expect).map_err(|e| anyhow::anyhow!("pi.toml [healthcheck]: {e}"))?;
        }
        if let Some(expose) = &parsed.ingress.expose {
            if ExposeMode::parse(expose).is_none() {
                anyhow::bail!(
                    "invalid pi.toml [ingress].expose '{expose}' (use \"private\" or \"lan\")"
                );
            }
        }
        Ok(parsed)
    }

    pub fn load(path: &Path) -> anyhow::Result<PiToml> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            anyhow::anyhow!(
                "cannot read {}: {e} (run from the project root, see §12)",
                path.display()
            )
        })?;
        PiToml::parse(&text)
    }

    pub fn to_project_config(&self) -> ProjectConfig {
        ProjectConfig {
            name: self.project.name.clone(),
            repo: self.source.repo.clone(),
            branch: self.source.branch.clone(),
            compose_path: self.build.compose.clone(),
            service: self.ingress.service.clone(),
            container_port: self.ingress.port,
            hostname: self.ingress.hostname.clone(),
            expose: self
                .ingress
                .expose
                .as_deref()
                .and_then(ExposeMode::parse)
                .unwrap_or_default(),
            healthcheck: HealthcheckConfig {
                path: self.healthcheck.path.clone(),
                expect: self.healthcheck.expect.clone(),
                timeout_secs: self
                    .healthcheck
                    .timeout
                    .as_deref()
                    .and_then(|t| parse_duration_secs(t).ok())
                    .unwrap_or(60),
            },
            timeouts: StageTimeoutOverrides {
                fetch_secs: self
                    .timeouts
                    .fetch
                    .as_deref()
                    .and_then(|t| parse_duration_secs(t).ok()),
                build_secs: self
                    .timeouts
                    .build
                    .as_deref()
                    .and_then(|t| parse_duration_secs(t).ok()),
                up_secs: self
                    .timeouts
                    .up
                    .as_deref()
                    .and_then(|t| parse_duration_secs(t).ok()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
schema = 1

[project]
name = "rateme"

[source]
repo = "git@github.com:isskelo/rateme.git"
branch = "main"

[build]
compose = "docker-compose.yml"

[ingress]
hostname = "rateme.isskelo.com"
service = "web"
port = 3000

[healthcheck]
path = "/"

[env]
file = ".env"
"#;

    #[test]
    fn parses_spec_sample_and_tolerates_future_sections() {
        let parsed = PiToml::parse(SAMPLE).unwrap();
        let config = parsed.to_project_config();
        assert_eq!(config.name, "rateme");
        assert_eq!(config.repo, "git@github.com:isskelo/rateme.git");
        assert_eq!(config.branch, "main");
        assert_eq!(config.compose_path, "docker-compose.yml");
        assert_eq!(config.service, "web");
        assert_eq!(config.container_port, 3000);
        assert_eq!(config.hostname.as_deref(), Some("rateme.isskelo.com"));
    }

    #[test]
    fn rejects_unknown_schema_version() {
        let toml = SAMPLE.replace("schema = 1", "schema = 2");
        let err = PiToml::parse(&toml).unwrap_err().to_string();
        assert!(err.contains("schema"), "got: {err}");
    }

    #[test]
    fn env_and_healthcheck_sections_are_parsed_with_defaults() {
        let parsed = PiToml::parse(SAMPLE).unwrap();
        assert_eq!(parsed.env.file, ".env");
        let config = parsed.to_project_config();
        assert_eq!(config.healthcheck.path.as_deref(), Some("/"));
        assert_eq!(config.healthcheck.expect, None);
        assert_eq!(config.healthcheck.timeout_secs, 60, "default budget");
    }

    #[test]
    fn missing_env_and_healthcheck_sections_fall_back_to_defaults() {
        let toml = SAMPLE
            .replace("[healthcheck]\npath = \"/\"\n", "")
            .replace("[env]\nfile = \".env\"\n", "");
        let parsed = PiToml::parse(&toml).unwrap();
        assert_eq!(parsed.env.file, ".env");
        let config = parsed.to_project_config();
        assert_eq!(config.healthcheck.path, None, "no path -> TCP probe");
        assert_eq!(config.healthcheck.timeout_secs, 60);
    }

    #[test]
    fn healthcheck_timeout_and_expect_are_validated() {
        let toml = SAMPLE.replace(
            "path = \"/\"",
            "path = \"/\"\ntimeout = \"2m\"\nexpect = \"204\"",
        );
        let config = PiToml::parse(&toml).unwrap().to_project_config();
        assert_eq!(config.healthcheck.timeout_secs, 120);
        assert_eq!(config.healthcheck.expect.as_deref(), Some("204"));

        let bad = SAMPLE.replace("path = \"/\"", "path = \"/\"\ntimeout = \"soon\"");
        assert!(PiToml::parse(&bad).is_err());
        let bad = SAMPLE.replace("path = \"/\"", "path = \"/\"\nexpect = \"ok\"");
        assert!(PiToml::parse(&bad).is_err());
    }

    #[test]
    fn timeouts_section_maps_to_overrides_and_is_validated() {
        let toml = SAMPLE.replace(
            "[healthcheck]",
            "[timeouts]\nfetch = \"3m\"\nup = \"120s\"\n\n[healthcheck]",
        );
        let config = PiToml::parse(&toml).unwrap().to_project_config();
        assert_eq!(config.timeouts.fetch_secs, Some(180));
        assert_eq!(config.timeouts.build_secs, None, "not set -> agent default");
        assert_eq!(config.timeouts.up_secs, Some(120));

        let bad = SAMPLE.replace(
            "[healthcheck]",
            "[timeouts]\nbuild = \"soon\"\n\n[healthcheck]",
        );
        assert!(PiToml::parse(&bad).is_err());
    }

    #[test]
    fn missing_timeouts_section_means_no_overrides() {
        let config = PiToml::parse(SAMPLE).unwrap().to_project_config();
        assert_eq!(config.timeouts, Default::default());
    }

    #[test]
    fn expose_defaults_private_and_parses_lan() {
        let default_cfg = PiToml::parse(SAMPLE).unwrap().to_project_config();
        assert_eq!(
            default_cfg.expose,
            pi_domain::entities::ExposeMode::Private
        );

        let lan = SAMPLE.replace("port = 3000", "port = 3000\nexpose = \"lan\"");
        let lan_cfg = PiToml::parse(&lan).unwrap().to_project_config();
        assert_eq!(lan_cfg.expose, pi_domain::entities::ExposeMode::Lan);
    }

    #[test]
    fn invalid_expose_is_rejected() {
        let bad = SAMPLE.replace("port = 3000", "port = 3000\nexpose = \"public\"");
        let err = PiToml::parse(&bad).unwrap_err().to_string();
        assert!(err.contains("expose"), "got: {err}");
    }
}
