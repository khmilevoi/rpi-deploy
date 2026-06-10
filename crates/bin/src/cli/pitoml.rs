use std::path::Path;

use pi_domain::entities::ProjectConfig;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct PiToml {
    pub schema: u32,
    pub project: ProjectSection,
    pub source: SourceSection,
    #[serde(default)]
    pub build: BuildSection,
    pub ingress: IngressSection,
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
        BuildSection { compose: default_compose() }
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

impl PiToml {
    pub fn parse(text: &str) -> anyhow::Result<PiToml> {
        let parsed: PiToml = toml::from_str(text)?;
        if parsed.schema != 1 {
            anyhow::bail!("unsupported pi.toml schema {} (this pi supports schema 1)", parsed.schema);
        }
        Ok(parsed)
    }

    pub fn load(path: &Path) -> anyhow::Result<PiToml> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e} (run from the project root, see §12)", path.display()))?;
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
}
