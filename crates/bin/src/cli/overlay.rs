use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::cli::rpitoml::{CommandValue, RpiToml};

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

/// Substitute ${VAR} in one allowed field. Marks `used` when a reference was found.
fn substitute(
    field: &str,
    value: &str,
    vars: &BTreeMap<String, String>,
    used: &mut bool,
) -> anyhow::Result<String> {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            anyhow::bail!("{field}: unclosed ${{...}} in '{value}'");
        };
        let name = &after[..end];
        *used = true;
        let Some(v) = vars.get(name) else {
            anyhow::bail!(
                "{field}: unknown variable '{name}' (available: {})",
                vars.keys().cloned().collect::<Vec<_>>().join(", ")
            );
        };
        out.push_str(v);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Error when a forbidden field contains a ${...} reference.
fn forbid(field: &str, value: Option<&str>) -> anyhow::Result<()> {
    if value.is_some_and(|v| v.contains("${")) {
        anyhow::bail!(
            "{field}: ${{...}} interpolation is only allowed in source.branch and ingress.hostname"
        );
    }
    Ok(())
}

fn command_strings(value: &CommandValue) -> Vec<&str> {
    use crate::cli::rpitoml::CommandRun;
    match value {
        CommandValue::Line(line) => vec![line.as_str()],
        CommandValue::Argv(items) => items.iter().map(String::as_str).collect(),
        CommandValue::Table { run, service } => {
            let mut out = match run {
                CommandRun::Line(line) => vec![line.as_str()],
                CommandRun::Argv(items) => items.iter().map(String::as_str).collect(),
            };
            if let Some(s) = service {
                out.push(s.as_str());
            }
            out
        }
    }
}

#[allow(dead_code)]
pub fn interpolate(
    overlay: &mut RpiTomlOverlay,
    user_vars: &BTreeMap<String, String>,
) -> anyhow::Result<bool> {
    // Forbid ${...} everywhere except the two allowed fields.
    if let Some(s) = &overlay.source {
        forbid("source.repo", s.repo.as_deref())?;
    }
    if let Some(b) = &overlay.build {
        forbid("build.compose", b.compose.as_deref())?;
    }
    if let Some(i) = &overlay.ingress {
        forbid("ingress.service", i.service.as_deref())?;
        forbid("ingress.expose", i.expose.as_deref())?;
    }
    if let Some(t) = &overlay.timeouts {
        for (f, v) in [
            ("timeouts.fetch", &t.fetch),
            ("timeouts.build", &t.build),
            ("timeouts.up", &t.up),
            ("timeouts.command", &t.command),
        ] {
            forbid(f, v.as_deref())?;
        }
    }
    if let Some(h) = &overlay.healthcheck {
        for (f, v) in [
            ("healthcheck.path", &h.path),
            ("healthcheck.expect", &h.expect),
            ("healthcheck.timeout", &h.timeout),
        ] {
            forbid(f, v.as_deref())?;
        }
    }
    if let Some(s) = &overlay.secrets {
        forbid("secrets.env", s.env.as_deref())?;
        for f in s.files.iter().flatten() {
            forbid("secrets.files", Some(f))?;
        }
    }
    for (name, value) in overlay.commands.iter().flatten() {
        for s in command_strings(value) {
            forbid(&format!("commands.{name}"), Some(s))?;
        }
    }
    if let Some(e) = &overlay.environment {
        forbid("environment.ttl", e.ttl.as_deref())?;
        forbid("environment.on_create", e.on_create.as_deref())?;
    }

    // Check if RPI_ENV_SLUG is actually referenced before deriving it.
    let needs_slug = [
        overlay.source.as_ref().and_then(|s| s.branch.as_deref()),
        overlay.ingress.as_ref().and_then(|i| i.hostname.as_deref()),
    ]
    .into_iter()
    .flatten()
    .any(|v| v.contains("${RPI_ENV_SLUG}"));
    let mut vars = user_vars.clone();
    if needs_slug {
        let Some(branch) = user_vars.get("BRANCH_NAME") else {
            anyhow::bail!(
                "${{RPI_ENV_SLUG}} requires --vars BRANCH_NAME=<branch> (the slug is derived from it)"
            );
        };
        vars.insert("RPI_ENV_SLUG".to_string(), derive_slug(branch)?);
    }

    let mut used = false;
    if let Some(s) = &mut overlay.source {
        if let Some(branch) = &s.branch {
            s.branch = Some(substitute("source.branch", branch, &vars, &mut used)?);
        }
    }
    if let Some(i) = &mut overlay.ingress {
        if let Some(hostname) = &i.hostname {
            i.hostname = Some(substitute("ingress.hostname", hostname, &vars, &mut used)?);
        }
    }
    Ok(used)
}

/// `""` resets an optional field to `None`; any other value replaces it.
fn reset_or(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// Typed schema-aware merge (spec: scalars replace, tables field-wise,
/// arrays and [commands] wholesale, "" resets optionals).
#[allow(dead_code)]
pub fn apply_overlay(base: &mut RpiToml, overlay: RpiTomlOverlay) {
    if let Some(s) = overlay.source {
        if let Some(repo) = s.repo {
            base.source.repo = repo;
        }
        if let Some(branch) = s.branch {
            base.source.branch = branch;
        }
    }
    if let Some(b) = overlay.build {
        if let Some(compose) = b.compose {
            base.build.compose = compose;
        }
    }
    if let Some(i) = overlay.ingress {
        if let Some(hostname) = i.hostname {
            base.ingress.hostname = reset_or(hostname);
        }
        if let Some(service) = i.service {
            base.ingress.service = service;
        }
        if let Some(port) = i.port {
            base.ingress.port = port;
        }
        if let Some(expose) = i.expose {
            base.ingress.expose = reset_or(expose);
        }
    }
    if let Some(t) = overlay.timeouts {
        if let Some(v) = t.fetch {
            base.timeouts.fetch = reset_or(v);
        }
        if let Some(v) = t.build {
            base.timeouts.build = reset_or(v);
        }
        if let Some(v) = t.up {
            base.timeouts.up = reset_or(v);
        }
        if let Some(v) = t.command {
            base.timeouts.command = reset_or(v);
        }
    }
    if let Some(h) = overlay.healthcheck {
        if let Some(v) = h.path {
            base.healthcheck.path = reset_or(v);
        }
        if let Some(v) = h.expect {
            base.healthcheck.expect = reset_or(v);
        }
        if let Some(v) = h.timeout {
            base.healthcheck.timeout = reset_or(v);
        }
    }
    if let Some(s) = overlay.secrets {
        if let Some(env) = s.env {
            base.secrets.env = reset_or(env);
        }
        if let Some(files) = s.files {
            base.secrets.files = files;
        }
    }
    if let Some(commands) = overlay.commands {
        base.commands = Some(commands);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"
schema = 1

[project]
name = "myapp"

[source]
repo = "git@github.com:acme/myapp.git"
branch = "main"

[ingress]
hostname = "app.example.com"
service = "web"
port = 3000

[healthcheck]
path = "/health"

[secrets]
env = ".env"

[commands]
seed = "node seed.js"
"#;

    #[test]
    fn merge_replaces_scalars_field_wise() {
        let mut base = crate::cli::rpitoml::RpiToml::parse(BASE).unwrap();
        let o = overlay("[source]\nbranch = \"develop\"\n\n[ingress]\nhostname = \"test.example.com\"\n\n[secrets]\nenv = \".env.test\"\n");
        apply_overlay(&mut base, o);
        assert_eq!(base.source.branch, "develop");
        assert_eq!(
            base.source.repo, "git@github.com:acme/myapp.git",
            "untouched"
        );
        assert_eq!(base.ingress.hostname.as_deref(), Some("test.example.com"));
        assert_eq!(base.ingress.service, "web", "untouched");
        assert_eq!(base.secrets.env.as_deref(), Some(".env.test"));
    }

    #[test]
    fn empty_string_resets_optional_fields() {
        let mut base = crate::cli::rpitoml::RpiToml::parse(BASE).unwrap();
        let o = overlay(
            "[ingress]\nhostname = \"\"\n\n[secrets]\nenv = \"\"\n\n[healthcheck]\npath = \"\"\n",
        );
        apply_overlay(&mut base, o);
        assert_eq!(base.ingress.hostname, None);
        assert_eq!(base.secrets.env, None);
        assert_eq!(base.healthcheck.path, None);
    }

    #[test]
    fn commands_table_replaces_wholesale() {
        let mut base = crate::cli::rpitoml::RpiToml::parse(BASE).unwrap();
        let o = overlay("[commands]\nmigrate = \"npx prisma migrate deploy\"\n");
        apply_overlay(&mut base, o);
        let commands = base.commands.as_ref().unwrap();
        assert!(commands.contains_key("migrate"));
        assert!(
            !commands.contains_key("seed"),
            "base commands must be replaced, not merged"
        );
    }

    #[test]
    fn secrets_files_replace_wholesale() {
        let mut base = crate::cli::rpitoml::RpiToml::parse(&BASE.replace(
            "env = \".env\"",
            "env = \".env\"\nfiles = [\"a.pem\", \"b.pem\"]",
        ))
        .unwrap();
        let o = overlay("[secrets]\nfiles = [\"c.pem\"]\n");
        apply_overlay(&mut base, o);
        assert_eq!(base.secrets.files, vec!["c.pem".to_string()]);
    }

    #[test]
    fn merged_result_passes_common_validation() {
        let mut base = crate::cli::rpitoml::RpiToml::parse(BASE).unwrap();
        let o = overlay("[healthcheck]\ntimeout = \"soon\"\n");
        apply_overlay(&mut base, o);
        assert!(
            base.validate_common().is_err(),
            "bad merged duration must fail"
        );
    }

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

    fn overlay(text: &str) -> RpiTomlOverlay {
        RpiTomlOverlay::parse(text, "rpi.branch.toml").unwrap()
    }

    fn branch_vars() -> BTreeMap<String, String> {
        parse_vars(&["BRANCH_NAME=feature/login".into()]).unwrap()
    }

    #[test]
    fn interpolates_branch_and_hostname() {
        let mut o = overlay(
            "[source]\nbranch = \"${BRANCH_NAME}\"\n\n[ingress]\nhostname = \"${RPI_ENV_SLUG}.preview.example.com\"\n",
        );
        let parameterized = interpolate(&mut o, &branch_vars()).unwrap();
        assert!(parameterized);
        assert_eq!(
            o.source.as_ref().unwrap().branch.as_deref(),
            Some("feature/login")
        );
        assert_eq!(
            o.ingress.as_ref().unwrap().hostname.as_deref(),
            Some("feature-login.preview.example.com")
        );
    }

    #[test]
    fn static_overlay_is_not_parameterized() {
        let mut o = overlay("[source]\nbranch = \"develop\"\n");
        assert!(!interpolate(&mut o, &BTreeMap::new()).unwrap());
    }

    #[test]
    fn unknown_var_and_unclosed_brace_are_errors() {
        let mut o = overlay("[source]\nbranch = \"${NOPE}\"\n");
        let err = interpolate(&mut o, &branch_vars()).unwrap_err().to_string();
        assert!(err.contains("NOPE"), "got: {err}");
        let mut o = overlay("[source]\nbranch = \"${BRANCH_NAME\"\n");
        assert!(interpolate(&mut o, &branch_vars()).is_err());
    }

    #[test]
    fn interpolation_outside_allowed_fields_is_rejected() {
        for text in [
            "[secrets]\nenv = \".env.${BRANCH_NAME}\"\n",
            "[build]\ncompose = \"${BRANCH_NAME}.yml\"\n",
            "[ingress]\nservice = \"${BRANCH_NAME}\"\n",
            "[commands]\nseed = \"run ${BRANCH_NAME}\"\n",
            "[environment]\non_create = \"${BRANCH_NAME}\"\n",
        ] {
            let mut o = overlay(text);
            let err = interpolate(&mut o, &branch_vars()).unwrap_err().to_string();
            assert!(
                err.contains("source.branch") && err.contains("ingress.hostname"),
                "{text}: {err}"
            );
        }
    }

    #[test]
    fn missing_branch_name_for_parameterized_overlay_is_an_error() {
        let mut o = overlay("[source]\nbranch = \"${BRANCH_NAME}\"\n");
        let err = interpolate(&mut o, &BTreeMap::new())
            .unwrap_err()
            .to_string();
        assert!(err.contains("BRANCH_NAME"), "got: {err}");
    }

    #[test]
    fn static_overlay_ignores_underivable_branch_name_for_slug() {
        // BRANCH_NAME that cannot produce a slug must not break an overlay
        // that never references ${RPI_ENV_SLUG}; parse_vars accepts the value,
        // interpolate must not call derive_slug.
        let mut o = overlay("[source]\nbranch = \"${BRANCH_NAME}\"\n");
        let vars = parse_vars(&["BRANCH_NAME=___".into()]).unwrap();
        assert!(interpolate(&mut o, &vars).unwrap());
        assert_eq!(o.source.as_ref().unwrap().branch.as_deref(), Some("___"));

        let mut o = overlay("[ingress]\nhostname = \"${RPI_ENV_SLUG}.preview.example.com\"\n");
        let vars = parse_vars(&["BRANCH_NAME=___".into()]).unwrap();
        let err = interpolate(&mut o, &vars).unwrap_err().to_string();
        assert!(err.contains("RPI_ENV_SLUG"), "got: {err}");
    }

    #[test]
    fn multiple_references_in_one_string_are_substituted() {
        let mut o =
            overlay("[ingress]\nhostname = \"${RPI_ENV_SLUG}.${BRANCH_NAME}.example.com\"\n");
        let vars = parse_vars(&["BRANCH_NAME=login".into()]).unwrap();
        assert!(interpolate(&mut o, &vars).unwrap());
        assert_eq!(
            o.ingress.as_ref().unwrap().hostname.as_deref(),
            Some("login.login.example.com")
        );
    }

    #[test]
    fn interpolation_in_argv_and_table_commands_is_rejected() {
        for text in [
            "[commands]\nseed = [\"run\", \"${BRANCH_NAME}\"]\n",
            "[commands.seed]\nrun = \"x\"\nservice = \"${BRANCH_NAME}\"\n",
        ] {
            let mut o = overlay(text);
            let vars = parse_vars(&["BRANCH_NAME=x".into()]).unwrap();
            assert!(
                interpolate(&mut o, &vars).is_err(),
                "{text} must be rejected"
            );
        }
    }
}
