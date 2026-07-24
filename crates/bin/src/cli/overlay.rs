use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::cli::rpitoml::CommandValue;

#[allow(dead_code)]
pub const RESERVED_ENV_NAMES: &[&str] = &["show", "ls", "destroy", "reset-data"];

#[allow(dead_code)]
pub fn validate_env_name(name: &str) -> anyhow::Result<()> {
    let mut chars = name.chars();
    let ok = matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-'));
    if !ok {
        anyhow::bail!("environment name '{name}' must match ^[a-z][a-z0-9-]*$");
    }
    if RESERVED_ENV_NAMES.contains(&name) {
        anyhow::bail!(
            "environment name '{name}' is reserved (reserved: {})",
            RESERVED_ENV_NAMES.join(", ")
        );
    }
    Ok(())
}

#[allow(dead_code)]
const MAX_SLUG_LEN: usize = 30;

fn is_valid_var_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some('A'..='Z'))
        && chars.all(|c| matches!(c, 'A'..='Z' | '0'..='9' | '_'))
}

#[allow(dead_code)]
pub fn parse_vars(pairs: &[String]) -> anyhow::Result<BTreeMap<String, String>> {
    let mut vars = BTreeMap::new();
    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            anyhow::bail!("--vars expects KEY=VALUE, got '{pair}'");
        };
        if !is_valid_var_name(key) {
            anyhow::bail!("--vars: variable name '{key}' must match ^[A-Z][A-Z0-9_]*$");
        }
        if key.starts_with("RPI_") {
            anyhow::bail!(
                "--vars: the RPI_ prefix is reserved for rpi-provided variables ('{key}')"
            );
        }
        if key != "BRANCH_NAME" {
            anyhow::bail!(
                "--vars: unknown variable '{key}' (this version supports only BRANCH_NAME)"
            );
        }
        if vars.insert(key.to_string(), value.to_string()).is_some() {
            anyhow::bail!("--vars: duplicate variable '{key}'");
        }
    }
    Ok(vars)
}

#[allow(dead_code)]
pub fn derive_slug(branch: &str) -> anyhow::Result<String> {
    let mut slug = String::new();
    for c in branch.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.truncate(MAX_SLUG_LEN);
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        anyhow::bail!(
            "cannot derive RPI_ENV_SLUG from BRANCH_NAME '{branch}' (no [a-z0-9] characters)"
        );
    }
    Ok(slug)
}

#[allow(dead_code)]
pub fn derive_key(base: &str, env: &str, slug: Option<&str>) -> String {
    match slug {
        Some(slug) => format!("{base}--{env}--{slug}"),
        None => format!("{base}--{env}"),
    }
}

/// Overlay file `rpi.<env>.toml`: every field optional; unknown fields are
/// errors (stricter than the base file); `schema`/`[project]` forbidden.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpiTomlOverlay {
    /// Forbidden — schema version is a property of the base file.
    schema: Option<toml::Value>,
    /// Forbidden — the project key is CLI-derived.
    project: Option<toml::Value>,
    pub source: Option<OverlaySource>,
    pub build: Option<OverlayBuild>,
    pub ingress: Option<OverlayIngress>,
    pub timeouts: Option<OverlayTimeouts>,
    pub healthcheck: Option<OverlayHealthcheck>,
    pub secrets: Option<OverlaySecrets>,
    pub commands: Option<BTreeMap<String, CommandValue>>,
    pub environment: Option<EnvironmentSection>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlaySource {
    pub repo: Option<String>,
    pub branch: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayBuild {
    pub compose: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayIngress {
    pub hostname: Option<String>,
    pub service: Option<String>,
    pub port: Option<u16>,
    pub expose: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayTimeouts {
    pub fetch: Option<String>,
    pub build: Option<String>,
    pub up: Option<String>,
    pub command: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayHealthcheck {
    pub path: Option<String>,
    pub expect: Option<String>,
    pub timeout: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlaySecrets {
    pub env: Option<String>,
    pub files: Option<Vec<String>>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentSection {
    pub ttl: Option<String>,
    pub on_create: Option<String>,
}

impl RpiTomlOverlay {
    pub fn parse(text: &str, file: &str) -> anyhow::Result<RpiTomlOverlay> {
        let parsed: RpiTomlOverlay =
            toml::from_str(text).map_err(|e| anyhow::anyhow!("{file}: {e}"))?;
        if parsed.schema.is_some() {
            anyhow::bail!("{file}: `schema` is not allowed in an overlay (set it in rpi.toml)");
        }
        if parsed.project.is_some() {
            anyhow::bail!("{file}: [project] is not allowed in an overlay (the project key is derived by the CLI)");
        }
        Ok(parsed)
    }

    #[allow(dead_code)]
    pub fn load(path: &Path) -> anyhow::Result<RpiTomlOverlay> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
        RpiTomlOverlay::parse(&text, &path.display().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_overlay() {
        let o = RpiTomlOverlay::parse(
            "[source]\nbranch = \"develop\"\n\n[environment]\nttl = \"7d\"\non_create = \"seed\"\n",
            "rpi.test.toml",
        )
        .unwrap();
        assert_eq!(
            o.source.as_ref().unwrap().branch.as_deref(),
            Some("develop")
        );
        let env = o.environment.as_ref().unwrap();
        assert_eq!(env.ttl.as_deref(), Some("7d"));
        assert_eq!(env.on_create.as_deref(), Some("seed"));
    }

    #[test]
    fn rejects_schema_and_project_in_overlay() {
        let err = RpiTomlOverlay::parse("schema = 1\n", "rpi.test.toml")
            .unwrap_err()
            .to_string();
        assert!(err.contains("schema"), "got: {err}");
        let err = RpiTomlOverlay::parse("[project]\nname = \"x\"\n", "rpi.test.toml")
            .unwrap_err()
            .to_string();
        assert!(err.contains("[project]"), "got: {err}");
    }

    #[test]
    fn rejects_unknown_fields() {
        let err = RpiTomlOverlay::parse("[sourc]\nbranch = \"x\"\n", "rpi.test.toml")
            .unwrap_err()
            .to_string();
        assert!(err.contains("sourc"), "got: {err}");
        let err = RpiTomlOverlay::parse("[ingress]\nhost = \"x\"\n", "rpi.test.toml")
            .unwrap_err()
            .to_string();
        assert!(err.contains("host"), "got: {err}");
    }

    #[test]
    fn env_name_charset_and_reserved() {
        assert!(validate_env_name("test").is_ok());
        assert!(validate_env_name("branch-preview2").is_ok());
        for bad in ["Test", "1x", "-x", "x_y", ""] {
            assert!(validate_env_name(bad).is_err(), "{bad} must be rejected");
        }
        for reserved in ["show", "ls", "destroy", "reset-data"] {
            let err = validate_env_name(reserved).unwrap_err().to_string();
            assert!(err.contains("reserved"), "{reserved}: {err}");
        }
    }

    #[test]
    fn parse_vars_accepts_branch_name_only() {
        let vars = parse_vars(&["BRANCH_NAME=feature/login".into()]).unwrap();
        assert_eq!(vars["BRANCH_NAME"], "feature/login");
        assert!(parse_vars(&[]).unwrap().is_empty());
        for (bad, needle) in [
            ("BRANCH_NAME", "KEY=VALUE"),      // no '='
            ("branch=x", "^[A-Z][A-Z0-9_]*$"), // lowercase name
            ("RPI_ENV_SLUG=x", "reserved"),    // RPI_ namespace
            ("FOO=x", "BRANCH_NAME"),          // unknown var in v1
        ] {
            let err = parse_vars(&[bad.to_string()]).unwrap_err().to_string();
            assert!(err.contains(needle), "{bad}: {err}");
        }
        let err = parse_vars(&["BRANCH_NAME=a".into(), "BRANCH_NAME=b".into()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("duplicate"), "got: {err}");
    }

    #[test]
    fn slug_normalizes_truncates_and_rejects_empty() {
        assert_eq!(derive_slug("feature/login").unwrap(), "feature-login");
        assert_eq!(derive_slug("Feature//Big__X").unwrap(), "feature-big-x");
        assert_eq!(derive_slug("-x-").unwrap(), "x");
        let long = derive_slug("abcdefghij-abcdefghij-abcdefghij-tail").unwrap();
        assert!(long.len() <= 30, "got len {}: {long}", long.len());
        assert!(!long.ends_with('-'));
        assert!(derive_slug("///").is_err());
    }

    #[test]
    fn key_derivation() {
        assert_eq!(derive_key("myapp", "test", None), "myapp--test");
        assert_eq!(
            derive_key("myapp", "branch", Some("feature-login")),
            "myapp--branch--feature-login"
        );
    }
}
