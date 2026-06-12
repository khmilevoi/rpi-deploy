use std::path::Path;

use pi_domain::entities::{HealthcheckConfig, ProjectConfig, StageTimeoutOverrides};
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

pub(crate) fn parse_duration_secs(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (digits, mult) = if let Some(d) = s.strip_suffix('m') {
        (d, 60)
    } else if let Some(d) = s.strip_suffix('s') {
        (d, 1)
    } else {
        (s, 1)
    };
    digits
        .trim()
        .parse::<u64>()
        .map(|n| n * mult)
        .map_err(|_| format!("invalid duration '{s}' (expected like \"60s\" or \"2m\")"))
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
        if let Some(expect) = &parsed.healthcheck.expect {
            validate_expect(expect).map_err(|e| anyhow::anyhow!("pi.toml [healthcheck]: {e}"))?;
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
            timeouts: StageTimeoutOverrides::default(),
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
    fn parse_duration_secs_supports_s_m_and_bare_numbers() {
        assert_eq!(parse_duration_secs("60s").unwrap(), 60);
        assert_eq!(parse_duration_secs("2m").unwrap(), 120);
        assert_eq!(parse_duration_secs("90").unwrap(), 90);
        assert!(parse_duration_secs("soon").is_err());
    }
}
