# Environment Overlays Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deploy test and per-branch preview environments from the same repo via `rpi.<env>.toml` overlay files, with isolated keys/secrets, an `on_create` hook, `rpi env` commands, and an agent-side TTL reaper.

**Architecture:** "Thin agent" (spec `docs/superpowers/specs/2026-07-24-environment-overlays-design.md`): all resolution (overlay parse, `${VAR}` interpolation, typed merge, key derivation) happens in the CLI, which sends an ordinary `ProjectConfig` whose `name` is the derived key plus an optional `environment` block in `DeployRequest`. The agent adds registry metadata, kind guards, the `on_create` hook, `/v1/environments` endpoints, Cloudflare DNS deletion, and a background reaper.

**Tech Stack:** Rust workspace (`pi` bin crate → binary `rpi`; `pi-domain`, `pi-application`, `pi-infrastructure`), clap, axum, rusqlite + rusqlite_migration, tokio, serde/toml, mockall. E2E: Node harness `tests/e2e/run.mjs` with shell scenarios.

## Global Constraints

- Before EVERY commit run: `rtk cargo fmt --all` then `rtk cargo clippy --all-targets --locked -- -D warnings` then `rtk cargo test --locked`. All three must pass.
- Always prefix commands with `rtk` (repo rule), including git.
- Env overlay file: `./rpi.<env>.toml`; `<env>` matches `^[a-z][a-z0-9-]*$`; reserved names: `show`, `ls`, `destroy`, `reset-data`.
- Key derivation: `<base>--<env>` (named) / `<base>--<env>--<RPI_ENV_SLUG>` (parameterized). `--` is forbidden in a base `[project].name` (breaking change with clear error).
- Slug: lowercase; non-`[a-z0-9]` → `-`; collapse runs; trim edge `-`; truncate to 30 chars then trim trailing `-`; empty → error.
- Variables: user-supplied `BRANCH_NAME` only (v1); derived `RPI_ENV_SLUG`; `RPI_` prefix reserved (user vars must not use it); var name syntax `^[A-Z][A-Z0-9_]*$`.
- Interpolation allowed ONLY in overlay `source.branch` and `ingress.hostname`.
- Merge: scalar in overlay replaces base; tables merge field-wise; arrays and the whole `[commands]` table replace wholesale; empty string resets an optional field to absent.
- `[environment]` (`ttl`, `on_create`) valid only in overlays; `schema` and `[project]` forbidden in overlays.
- TTL: sliding from last successful deploy; no TTL → no expiry; reaper touches only environment entries with a ttl.
- New compat feature: `Feature::Environments`, capability `"environments"`, policy `Required`, since `"0.24.0"` (workspace version is 0.23.0).
- Commit messages end with `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

## Phase 1 — CLI resolution

### Task 1: Overlay schema + env-name validation (`overlay.rs`)

**Files:**
- Create: `crates/bin/src/cli/overlay.rs`
- Modify: `crates/bin/src/cli/mod.rs` (add `pub mod overlay;`)
- Modify: `crates/bin/src/cli/rpitoml.rs` (make `pub struct` fields available — they already are; no change needed here yet)

**Interfaces:**
- Produces: `RpiTomlOverlay` (all-optional mirror of `RpiToml` + `[environment]`), `RpiTomlOverlay::parse(text: &str, file: &str) -> anyhow::Result<RpiTomlOverlay>`, `RpiTomlOverlay::load(path: &Path) -> anyhow::Result<RpiTomlOverlay>`, `validate_env_name(name: &str) -> anyhow::Result<()>`, `pub const RESERVED_ENV_NAMES: &[&str]`.
- Consumes: `crate::cli::rpitoml::CommandValue` (reused for `[commands]` values).

- [ ] **Step 1: Write failing tests** — in `crates/bin/src/cli/overlay.rs` bottom `#[cfg(test)] mod tests`:

```rust
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
        assert_eq!(o.source.as_ref().unwrap().branch.as_deref(), Some("develop"));
        let env = o.environment.as_ref().unwrap();
        assert_eq!(env.ttl.as_deref(), Some("7d"));
        assert_eq!(env.on_create.as_deref(), Some("seed"));
    }

    #[test]
    fn rejects_schema_and_project_in_overlay() {
        let err = RpiTomlOverlay::parse("schema = 1\n", "rpi.test.toml").unwrap_err().to_string();
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
}
```

- [ ] **Step 2: Run to verify failure** — `rtk cargo test --locked -p pi overlay` → FAIL (module missing).

- [ ] **Step 3: Implement** — `crates/bin/src/cli/overlay.rs`:

```rust
use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::cli::rpitoml::CommandValue;

pub const RESERVED_ENV_NAMES: &[&str] = &["show", "ls", "destroy", "reset-data"];

pub fn validate_env_name(name: &str) -> anyhow::Result<()> {
    let mut chars = name.chars();
    let ok = matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-'));
    if !ok {
        anyhow::bail!("environment name '{name}' must match ^[a-z][a-z0-9-]*$");
    }
    if RESERVED_ENV_NAMES.contains(&name) {
        anyhow::bail!("environment name '{name}' is reserved (reserved: {})", RESERVED_ENV_NAMES.join(", "));
    }
    Ok(())
}

/// Overlay file `rpi.<env>.toml`: every field optional; unknown fields are
/// errors (stricter than the base file); `schema`/`[project]` forbidden.
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlaySource {
    pub repo: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayBuild {
    pub compose: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayIngress {
    pub hostname: Option<String>,
    pub service: Option<String>,
    pub port: Option<u16>,
    pub expose: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayTimeouts {
    pub fetch: Option<String>,
    pub build: Option<String>,
    pub up: Option<String>,
    pub command: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayHealthcheck {
    pub path: Option<String>,
    pub expect: Option<String>,
    pub timeout: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlaySecrets {
    pub env: Option<String>,
    pub files: Option<Vec<String>>,
}

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

    pub fn load(path: &Path) -> anyhow::Result<RpiTomlOverlay> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
        RpiTomlOverlay::parse(&text, &path.display().to_string())
    }
}
```

Add `pub mod overlay;` to `crates/bin/src/cli/mod.rs`.

- [ ] **Step 4: Run tests** — `rtk cargo test --locked -p pi overlay` → PASS.
- [ ] **Step 5: Commit** — `rtk git add -A && rtk git commit -m "feat(cli): overlay file schema for environment overlays"` (+ trailer).

### Task 2: Vars parsing, slug, key derivation

**Files:**
- Modify: `crates/bin/src/cli/overlay.rs`

**Interfaces:**
- Produces: `parse_vars(pairs: &[String]) -> anyhow::Result<BTreeMap<String, String>>`, `derive_slug(branch: &str) -> anyhow::Result<String>`, `derive_key(base: &str, env: &str, slug: Option<&str>) -> String`.

- [ ] **Step 1: Failing tests** (append to `overlay.rs` tests):

```rust
#[test]
fn parse_vars_accepts_branch_name_only() {
    let vars = parse_vars(&["BRANCH_NAME=feature/login".into()]).unwrap();
    assert_eq!(vars["BRANCH_NAME"], "feature/login");
    assert!(parse_vars(&[]).unwrap().is_empty());
    for (bad, needle) in [
        ("BRANCH_NAME", "KEY=VALUE"),            // no '='
        ("branch=x", "^[A-Z][A-Z0-9_]*$"),       // lowercase name
        ("RPI_ENV_SLUG=x", "reserved"),          // RPI_ namespace
        ("FOO=x", "BRANCH_NAME"),                // unknown var in v1
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
```

- [ ] **Step 2: Run to verify failure** — `rtk cargo test --locked -p pi overlay` → FAIL.
- [ ] **Step 3: Implement** (append to `overlay.rs`):

```rust
const MAX_SLUG_LEN: usize = 30;

fn is_valid_var_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some('A'..='Z'))
        && chars.all(|c| matches!(c, 'A'..='Z' | '0'..='9' | '_'))
}

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
            anyhow::bail!("--vars: the RPI_ prefix is reserved for rpi-provided variables ('{key}')");
        }
        if key != "BRANCH_NAME" {
            anyhow::bail!("--vars: unknown variable '{key}' (this version supports only BRANCH_NAME)");
        }
        if vars.insert(key.to_string(), value.to_string()).is_some() {
            anyhow::bail!("--vars: duplicate variable '{key}'");
        }
    }
    Ok(vars)
}

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
        anyhow::bail!("cannot derive RPI_ENV_SLUG from BRANCH_NAME '{branch}' (no [a-z0-9] characters)");
    }
    Ok(slug)
}

pub fn derive_key(base: &str, env: &str, slug: Option<&str>) -> String {
    match slug {
        Some(slug) => format!("{base}--{env}--{slug}"),
        None => format!("{base}--{env}"),
    }
}
```

- [ ] **Step 4: Run tests** → PASS. **Step 5: Commit** `feat(cli): vars, slug and key derivation for environments`.

### Task 3: `${VAR}` interpolation with two-field allow-list

**Files:**
- Modify: `crates/bin/src/cli/overlay.rs`

**Interfaces:**
- Produces: `interpolate(overlay: &mut RpiTomlOverlay, user_vars: &BTreeMap<String, String>) -> anyhow::Result<bool>` — returns `true` when the overlay referenced any variable (parameterized). Derives `RPI_ENV_SLUG` internally from `BRANCH_NAME` when present.

- [ ] **Step 1: Failing tests:**

```rust
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
    assert_eq!(o.source.as_ref().unwrap().branch.as_deref(), Some("feature/login"));
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
    let err = interpolate(&mut o, &BTreeMap::new()).unwrap_err().to_string();
    assert!(err.contains("BRANCH_NAME"), "got: {err}");
}
```

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement:**

```rust
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

    let mut vars = user_vars.clone();
    if let Some(branch) = user_vars.get("BRANCH_NAME") {
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
```

Note the "missing BRANCH_NAME" case falls out naturally: with no user vars, the map is empty and `substitute` fails with "unknown variable 'BRANCH_NAME'". Keep it that way (test asserts the message contains `BRANCH_NAME`).

- [ ] **Step 4: Run tests** → PASS. **Step 5: Commit** `feat(cli): overlay variable interpolation with allow-list`.

### Task 4: Typed merge + shared base validation refactor

**Files:**
- Modify: `crates/bin/src/cli/rpitoml.rs` (extract `validate_common`, add `--` ban + `[environment]` rejection in base)
- Modify: `crates/bin/src/cli/overlay.rs` (add `apply_overlay`)

**Interfaces:**
- Produces: `RpiToml::validate_common(&self) -> anyhow::Result<()>` (everything `parse` checks today EXCEPT the schema==1 check and the `--` ban — post-merge revalidation calls this), `apply_overlay(base: &mut RpiToml, overlay: RpiTomlOverlay)`.
- `RpiToml::parse` behavior change: additionally rejects `--` in `[project].name` and any `[environment]` section in the base file.

- [ ] **Step 1: Failing tests.** In `rpitoml.rs` tests:

```rust
#[test]
fn base_name_with_double_dash_is_rejected() {
    let toml = SAMPLE.replace("name = \"rateme\"", "name = \"rate--me\"");
    let err = RpiToml::parse(&toml).unwrap_err().to_string();
    assert!(err.contains("--"), "got: {err}");
}

#[test]
fn environment_section_in_base_is_rejected() {
    let toml = format!("{SAMPLE}\n[environment]\nttl = \"7d\"\n");
    let err = RpiToml::parse(&toml).unwrap_err().to_string();
    assert!(err.contains("[environment]"), "got: {err}");
}
```

In `overlay.rs` tests (uses `RpiToml::parse` on the same style of sample):

```rust
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
    assert_eq!(base.source.repo, "git@github.com:acme/myapp.git", "untouched");
    assert_eq!(base.ingress.hostname.as_deref(), Some("test.example.com"));
    assert_eq!(base.ingress.service, "web", "untouched");
    assert_eq!(base.secrets.env.as_deref(), Some(".env.test"));
}

#[test]
fn empty_string_resets_optional_fields() {
    let mut base = crate::cli::rpitoml::RpiToml::parse(BASE).unwrap();
    let o = overlay("[ingress]\nhostname = \"\"\n\n[secrets]\nenv = \"\"\n\n[healthcheck]\npath = \"\"\n");
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
    assert!(!commands.contains_key("seed"), "base commands must be replaced, not merged");
}

#[test]
fn secrets_files_replace_wholesale() {
    let mut base = crate::cli::rpitoml::RpiToml::parse(
        &BASE.replace("env = \".env\"", "env = \".env\"\nfiles = [\"a.pem\", \"b.pem\"]"),
    )
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
    assert!(base.validate_common().is_err(), "bad merged duration must fail");
}
```

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement.** In `rpitoml.rs`:
  1. Add to the `RpiToml` struct (next to `legacy_env`): `#[serde(default, rename = "environment")] environment_section: Option<toml::Value>,`
  2. Move the body of `parse` after the schema check into a new `pub fn validate_common(&self) -> anyhow::Result<()>` (the duration loops, `validate_expect`, expose check, legacy `[env]` check, secret-path checks, commands checks — everything currently between the schema check and `Ok(parsed)`), and have `parse` call it:

```rust
pub fn parse(text: &str) -> anyhow::Result<RpiToml> {
    let parsed: RpiToml = toml::from_str(text)?;
    if parsed.schema != 1 {
        anyhow::bail!(
            "unsupported rpi.toml schema {} (this rpi supports schema 1)",
            parsed.schema
        );
    }
    if parsed.project.name.contains("--") {
        anyhow::bail!(
            "rpi.toml [project].name '{}' must not contain '--' (reserved for environment keys; rename the project)",
            parsed.project.name
        );
    }
    if parsed.environment_section.is_some() {
        anyhow::bail!(
            "rpi.toml: [environment] is only allowed in overlay files (rpi.<env>.toml)"
        );
    }
    parsed.validate_common()?;
    Ok(parsed)
}
```

  In `overlay.rs`:

```rust
use crate::cli::rpitoml::RpiToml;

fn reset_or(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

/// Typed schema-aware merge (spec: scalars replace, tables field-wise,
/// arrays and [commands] wholesale, "" resets optionals).
pub fn apply_overlay(base: &mut RpiToml, overlay: RpiTomlOverlay) {
    if let Some(s) = overlay.source {
        if let Some(repo) = s.repo { base.source.repo = repo; }
        if let Some(branch) = s.branch { base.source.branch = branch; }
    }
    if let Some(b) = overlay.build {
        if let Some(compose) = b.compose { base.build.compose = compose; }
    }
    if let Some(i) = overlay.ingress {
        if let Some(hostname) = i.hostname { base.ingress.hostname = reset_or(hostname); }
        if let Some(service) = i.service { base.ingress.service = service; }
        if let Some(port) = i.port { base.ingress.port = port; }
        if let Some(expose) = i.expose { base.ingress.expose = reset_or(expose); }
    }
    if let Some(t) = overlay.timeouts {
        if let Some(v) = t.fetch { base.timeouts.fetch = reset_or(v); }
        if let Some(v) = t.build { base.timeouts.build = reset_or(v); }
        if let Some(v) = t.up { base.timeouts.up = reset_or(v); }
        if let Some(v) = t.command { base.timeouts.command = reset_or(v); }
    }
    if let Some(h) = overlay.healthcheck {
        if let Some(v) = h.path { base.healthcheck.path = reset_or(v); }
        if let Some(v) = h.expect { base.healthcheck.expect = reset_or(v); }
        if let Some(v) = h.timeout { base.healthcheck.timeout = reset_or(v); }
    }
    if let Some(s) = overlay.secrets {
        if let Some(env) = s.env { base.secrets.env = reset_or(env); }
        if let Some(files) = s.files { base.secrets.files = files; }
    }
    if let Some(commands) = overlay.commands {
        base.commands = Some(commands);
    }
}
```

  Note: `TimeoutsSection`/`HealthcheckSection`/`SecretsSection` fields are already `pub`; `RpiToml` fields are `pub` (except `legacy_env`/`environment_section`, which merge never touches).

- [ ] **Step 4: Run tests** — full `rtk cargo test --locked` (the rpitoml refactor touches existing tests) → PASS.
- [ ] **Step 5: Commit** `feat(cli): typed overlay merge and shared rpi.toml validation`.

### Task 5: Resolution entry point

**Files:**
- Modify: `crates/bin/src/cli/overlay.rs`

**Interfaces:**
- Produces:

```rust
pub struct EnvSelection {
    pub env: String,
    pub base: String,
    pub slug: Option<String>,
    pub key: String,
    pub ttl_secs: Option<u64>,
    pub on_create: Option<String>,
}
pub struct Resolved {
    pub rpitoml: RpiToml,          // merged; project.name == key when env selected
    pub env: Option<EnvSelection>,
}
/// Loads ./rpi.toml (+ ./rpi.<env>.toml), resolves everything.
pub fn resolve(env: Option<&str>, vars: &[String]) -> anyhow::Result<Resolved>;
/// Same but from explicit texts — unit-testable without touching the fs.
pub fn resolve_from(
    base_text: &str,
    overlay: Option<(&str, &str)>, // (env name, overlay text)
    vars: &[String],
) -> anyhow::Result<Resolved>;
```

- Consumes: `crate::duration::parse_duration_secs` (note: it is `pub(crate)`, callable from `cli::overlay`).

- [ ] **Step 1: Failing tests:**

```rust
#[test]
fn resolve_named_env_derives_key_and_ttl() {
    let r = resolve_from(
        BASE,
        Some(("test", "[source]\nbranch = \"develop\"\n\n[environment]\nttl = \"7d\"\non_create = \"seed\"\n")),
        &[],
    )
    .unwrap();
    assert_eq!(r.rpitoml.project.name, "myapp--test");
    let env = r.env.unwrap();
    assert_eq!(env.key, "myapp--test");
    assert_eq!(env.base, "myapp");
    assert_eq!(env.slug, None);
    assert_eq!(env.ttl_secs, Some(7 * 24 * 3600));
    assert_eq!(env.on_create.as_deref(), Some("seed"));
}

#[test]
fn resolve_parameterized_env_uses_slug_in_key() {
    let r = resolve_from(
        BASE,
        Some(("branch", "[source]\nbranch = \"${BRANCH_NAME}\"\n\n[ingress]\nhostname = \"${RPI_ENV_SLUG}.preview.example.com\"\n")),
        &["BRANCH_NAME=feature/login".into()],
    )
    .unwrap();
    assert_eq!(r.rpitoml.project.name, "myapp--branch--feature-login");
    assert_eq!(r.env.as_ref().unwrap().slug.as_deref(), Some("feature-login"));
    assert_eq!(r.rpitoml.source.branch, "feature/login");
}

#[test]
fn resolve_without_env_keeps_base_and_rejects_vars() {
    let r = resolve_from(BASE, None, &[]).unwrap();
    assert_eq!(r.rpitoml.project.name, "myapp");
    assert!(r.env.is_none());
    let err = resolve_from(BASE, None, &["BRANCH_NAME=x".into()]).unwrap_err().to_string();
    assert!(err.contains("--env"), "got: {err}");
}

#[test]
fn vars_for_static_overlay_are_rejected() {
    let err = resolve_from(
        BASE,
        Some(("test", "[source]\nbranch = \"develop\"\n")),
        &["BRANCH_NAME=x".into()],
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("not parameterized"), "got: {err}");
}

#[test]
fn on_create_must_reference_a_merged_command() {
    let err = resolve_from(
        BASE,
        Some(("test", "[environment]\non_create = \"nope\"\n")),
        &[],
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("nope"), "got: {err}");
    // [commands] replaced wholesale without the referenced command -> error too
    let err = resolve_from(
        BASE,
        Some(("test", "[commands]\nother = \"run x\"\n\n[environment]\non_create = \"seed\"\n")),
        &[],
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("seed"), "got: {err}");
}

#[test]
fn merged_config_is_revalidated() {
    let err = resolve_from(
        BASE,
        Some(("test", "[ingress]\nexpose = \"public\"\n")),
        &[],
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("expose"), "got: {err}");
}

#[test]
fn bad_ttl_is_rejected() {
    let err = resolve_from(BASE, Some(("test", "[environment]\nttl = \"soon\"\n")), &[])
        .unwrap_err()
        .to_string();
    assert!(err.contains("ttl"), "got: {err}");
}
```

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement:**

```rust
pub struct EnvSelection {
    pub env: String,
    pub base: String,
    pub slug: Option<String>,
    pub key: String,
    pub ttl_secs: Option<u64>,
    pub on_create: Option<String>,
}

pub struct Resolved {
    pub rpitoml: RpiToml,
    pub env: Option<EnvSelection>,
}

pub fn resolve(env: Option<&str>, vars: &[String]) -> anyhow::Result<Resolved> {
    let base_text = std::fs::read_to_string("rpi.toml")
        .map_err(|e| anyhow::anyhow!("cannot read rpi.toml: {e} (run from the project root, see §12)"))?;
    let overlay = match env {
        None => None,
        Some(name) => {
            validate_env_name(name)?;
            let file = format!("rpi.{name}.toml");
            let text = std::fs::read_to_string(&file).map_err(|e| {
                anyhow::anyhow!("cannot read {file}: {e}{}", available_overlays_hint())
            })?;
            Some((name, text))
        }
    };
    resolve_from(
        &base_text,
        overlay.as_ref().map(|(n, t)| (*n, t.as_str())),
        vars,
    )
}

/// " (found overlays: rpi.test.toml, rpi.branch.toml)" or "" — for the missing-file error.
fn available_overlays_hint() -> String {
    let mut found: Vec<String> = std::fs::read_dir(".")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("rpi.") && n.ends_with(".toml") && *n != "rpi.toml")
        .collect();
    found.sort();
    if found.is_empty() {
        String::new()
    } else {
        format!(" (found overlays: {})", found.join(", "))
    }
}

pub fn resolve_from(
    base_text: &str,
    overlay: Option<(&str, &str)>,
    vars: &[String],
) -> anyhow::Result<Resolved> {
    let mut base = RpiToml::parse(base_text)?;
    let Some((env_name, overlay_text)) = overlay else {
        if !vars.is_empty() {
            anyhow::bail!("--vars requires --env (variables are only used in overlays)");
        }
        return Ok(Resolved { rpitoml: base, env: None });
    };

    validate_env_name(env_name)?;
    let file = format!("rpi.{env_name}.toml");
    let mut overlay = RpiTomlOverlay::parse(overlay_text, &file)?;
    let user_vars = parse_vars(vars)?;
    let parameterized = interpolate(&mut overlay, &user_vars)?;
    if !parameterized && !user_vars.is_empty() {
        anyhow::bail!("{file} is not parameterized (no ${{...}} references) - remove --vars");
    }
    let slug = if parameterized {
        let branch = user_vars.get("BRANCH_NAME").expect("checked in interpolate");
        Some(derive_slug(branch)?)
    } else {
        None
    };

    let environment = overlay.environment.take();
    let base_name = base.project.name.clone();
    apply_overlay(&mut base, overlay);
    let key = derive_key(&base_name, env_name, slug.as_deref());
    base.project.name = key.clone();
    base.validate_common()
        .map_err(|e| anyhow::anyhow!("{file}: merged configuration is invalid: {e}"))?;

    let ttl_secs = match environment.as_ref().and_then(|e| e.ttl.as_deref()) {
        Some(ttl) => Some(
            crate::duration::parse_duration_secs(ttl)
                .map_err(|e| anyhow::anyhow!("{file} [environment].ttl: {e}"))?,
        ),
        None => None,
    };
    let on_create = environment.and_then(|e| e.on_create);
    if let Some(cmd) = &on_create {
        let declared = base.commands.as_ref().is_some_and(|c| c.contains_key(cmd));
        if !declared {
            anyhow::bail!(
                "{file} [environment].on_create: command '{cmd}' is not declared in the merged [commands]"
            );
        }
    }

    Ok(Resolved {
        rpitoml: base,
        env: Some(EnvSelection {
            env: env_name.to_string(),
            base: base_name,
            slug,
            key,
            ttl_secs,
            on_create,
        }),
    })
}
```

Note: `parse_duration_secs` is `pub(crate)` in `crates/bin/src/duration.rs` — reachable as `crate::duration::parse_duration_secs`.

- [ ] **Step 4: Run** `rtk cargo test --locked -p pi overlay` → PASS. **Step 5: Commit** `feat(cli): environment resolution entry point`.

### Task 6: `rpi config show`

**Files:**
- Modify: `crates/bin/src/cli/rpitoml.rs` (add `Serialize` derives)
- Modify: `crates/bin/src/cli/commands.rs` (add `config_show`)
- Modify: `crates/bin/src/main.rs` (add `Config` subcommand)

**Interfaces:**
- Produces: `commands::config_show(env: Option<String>, vars: Vec<String>) -> anyhow::Result<()>` (local, no agent); `overlay::render_resolved(r: &Resolved) -> anyhow::Result<String>`.

- [ ] **Step 1: Failing test** (in `overlay.rs` tests):

```rust
#[test]
fn render_resolved_prints_toml_with_key_and_environment() {
    let r = resolve_from(
        BASE,
        Some(("test", "[source]\nbranch = \"develop\"\n\n[environment]\nttl = \"7d\"\non_create = \"seed\"\n")),
        &[],
    )
    .unwrap();
    let text = render_resolved(&r).unwrap();
    assert!(text.contains("name = \"myapp--test\""), "got:\n{text}");
    assert!(text.contains("branch = \"develop\""));
    assert!(text.contains("[environment]"));
    assert!(text.contains("ttl_secs = 604800"));
    assert!(text.contains("on_create = \"seed\""));
    // resolved output must round-trip as valid TOML
    toml::from_str::<toml::Value>(&text).unwrap();
}
```

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement.**
  1. In `rpitoml.rs` add `serde::Serialize` derives with skips so `None` fields and internal fields are omitted:
     - `RpiToml`: `#[derive(Debug, Deserialize, Serialize)]`, on `legacy_env` and `environment_section` add `#[serde(skip_serializing)]`, on `commands` add `#[serde(skip_serializing_if = "Option::is_none")]`.
     - `ProjectSection`, `SourceSection`, `BuildSection`, `IngressSection` (add `#[serde(skip_serializing_if = "Option::is_none")]` on `hostname` and `expose`), `TimeoutsSection` (skip_serializing_if on all four), `HealthcheckSection` (same on all three), `SecretsSection` (skip on `env`; add `#[serde(skip_serializing_if = "Vec::is_empty")]` on `files`), `CommandValue`, `CommandRun`: add `Serialize` to each derive.
  2. In `overlay.rs`:

```rust
pub fn render_resolved(r: &Resolved) -> anyhow::Result<String> {
    let mut text = toml::to_string_pretty(&r.rpitoml)?;
    if let Some(env) = &r.env {
        text.push_str("\n[environment]\n");
        text.push_str(&format!("env = {}\n", toml_str(&env.env)));
        text.push_str(&format!("base = {}\n", toml_str(&env.base)));
        if let Some(slug) = &env.slug {
            text.push_str(&format!("slug = {}\n", toml_str(slug)));
        }
        if let Some(ttl) = env.ttl_secs {
            text.push_str(&format!("ttl_secs = {ttl}\n"));
        }
        if let Some(cmd) = &env.on_create {
            text.push_str(&format!("on_create = {}\n", toml_str(cmd)));
        }
    }
    Ok(text)
}

fn toml_str(s: &str) -> String {
    format!("{:?}", s) // TOML basic strings share JSON-style escaping for our charset
}
```

  3. In `commands.rs`:

```rust
pub async fn config_show(env: Option<String>, vars: Vec<String>) -> anyhow::Result<()> {
    let resolved = crate::cli::overlay::resolve(env.as_deref(), &vars)?;
    print!("{}", crate::cli::overlay::render_resolved(&resolved)?);
    Ok(())
}
```

  4. In `main.rs` add to `Cmd`:

```rust
/// Show the resolved configuration (base + overlay), locally.
Config {
    #[command(subcommand)]
    cmd: ConfigCmd,
},
```

```rust
#[derive(clap::Subcommand)]
enum ConfigCmd {
    /// Print the resolved rpi.toml (with --env: base + overlay merged).
    Show {
        #[arg(long)]
        env: Option<String>,
        #[arg(long = "vars")]
        vars: Vec<String>,
    },
}
```

  Match arm: `Cmd::Config { cmd: ConfigCmd::Show { env, vars } } => cli::commands::config_show(env, vars).await,`

- [ ] **Step 4: Run tests + smoke:** `rtk cargo test --locked -p pi` → PASS; manual smoke: from a temp dir with a sample rpi.toml, `cargo run -p pi -- config show` prints TOML.
- [ ] **Step 5: Commit** `feat(cli): rpi config show with resolved overlays`.

### Task 7: `--env`/`--vars` on deploy, secrets, command

**Files:**
- Modify: `crates/bin/src/main.rs` (flags on `Deploy`, `SecretsCmd::Send`, `SecretsCmd::Ls`, `Command`)
- Modify: `crates/bin/src/cli/commands.rs` (`deploy`, `deploy_cancel`, `secrets_send`, `secrets_ls`, `command` signatures + resolution)

**Interfaces:**
- Consumes: `overlay::resolve(env, vars) -> Resolved`.
- Produces: changed signatures — `deploy(git_ref: Option<String>, no_gh_key: bool, env: Option<String>, vars: Vec<String>, connect: ConnectOpts)`, `deploy_cancel(env: Option<String>, vars: Vec<String>, connect: ConnectOpts)`, `secrets_send(apply: bool, env: Option<String>, vars: Vec<String>, connect: ConnectOpts)`, `secrets_ls(env: Option<String>, vars: Vec<String>, connect: ConnectOpts)`, `command(name: Option<String>, args: Vec<String>, full: bool, env: Option<String>, vars: Vec<String>, connect: ConnectOpts)`.

- [ ] **Step 1: Add flags in `main.rs`.** To each of `Deploy`, `SecretsCmd::Send`, `SecretsCmd::Ls`, `Command` add:

```rust
    /// Deploy/operate an environment defined by rpi.<env>.toml
    #[arg(long)]
    env: Option<String>,
    /// Overlay variables, e.g. --vars BRANCH_NAME=feature/login (repeatable)
    #[arg(long = "vars")]
    vars: Vec<String>,
```

Update the corresponding match arms to pass `env, vars` through.

- [ ] **Step 2: Rewire `commands.rs`.** In each function replace

```rust
let rpitoml = RpiToml::load(Path::new("rpi.toml"))?;
```

with

```rust
let resolved = crate::cli::overlay::resolve(env.as_deref(), &vars)?;
let rpitoml = resolved.rpitoml;
```

Every subsequent use stays the same (`rpitoml.project.name` is already the derived key; `rpitoml.to_project_config()` produces the config with the key as name; `rpitoml.secrets` is the merged secrets section, so `secrets_send` picks up `.env.test` automatically). In `deploy`, keep `resolved.env` in a variable — Phase 2 Task 14 attaches it to the request; for now it is unused: bind as `let _env_selection = resolved.env;`.

- [ ] **Step 3: Run** `rtk cargo test --locked && rtk cargo clippy --all-targets --locked -- -D warnings` → PASS (fix any missed call sites the compiler reports — `main.rs` match arms are the complete list).
- [ ] **Step 4: Commit** `feat(cli): --env/--vars flags on deploy, secrets and command`.

---

## Phase 2 — Env-aware agent

### Task 8: Domain + wire types for the environment block

**Files:**
- Modify: `crates/domain/src/entities.rs` (add `EnvironmentMeta`, extend `ProjectConfig`, `Project`)
- Modify: `crates/bin/src/proto.rs` (add `EnvironmentDto`, extend `DeployRequest`, conversions)
- Modify (mechanical, compiler-driven): every `ProjectConfig`/`Project` literal in tests and adapters — the compiler lists them; known sites: `crates/application/src/deploy.rs` tests (`sample_config`, `Project { .. }` literals), `crates/application/src/command.rs` tests, `crates/application/src/list.rs`, `crates/application/src/test_support.rs`, `crates/infrastructure/src/repo.rs` (`row_to_project` — Task 9 fills real values; here set `environment: None, on_create_done: false, last_success_at: None`), `crates/bin/src/agent/http.rs` tests, `crates/bin/src/cli/rpitoml.rs` (`to_project_config` sets `environment: None`).

**Interfaces:**
- Produces (entities.rs):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentMeta {
    pub env: String,
    pub base: String,
    pub slug: Option<String>,
    pub ttl_secs: Option<u64>,
    pub on_create: Option<String>,
}
// ProjectConfig gains:  pub environment: Option<EnvironmentMeta>,
// Project gains:        pub on_create_done: bool,  pub last_success_at: Option<i64>,
```

- Produces (proto.rs):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentDto {
    pub env: String,
    pub base: String,
    #[serde(default)] pub slug: Option<String>,
    #[serde(default)] pub ttl_secs: Option<u64>,
    #[serde(default)] pub on_create: Option<String>,
}
// DeployRequest gains:
//   #[serde(default, skip_serializing_if = "Option::is_none")]
//   pub environment: Option<EnvironmentDto>,
// plus From<EnvironmentDto> for EnvironmentMeta and From<&EnvironmentMeta> for EnvironmentDto (field-for-field).
```

- `ProjectDto` is NOT changed (spec). `From<ProjectDto> for ProjectConfig` sets `environment: None` (the handler fills it from the request block); `From<&ProjectConfig> for ProjectDto` ignores `environment`.

- [ ] **Step 1: Failing test** (proto.rs tests module — create one if absent):

```rust
#[test]
fn deploy_request_environment_roundtrips_and_is_optional() {
    let json = r#"{"project":{"name":"myapp--test","repo":"r","branch":"b","compose":"docker-compose.yml","service":"web","port":3000,"hostname":null},"ref":null,"environment":{"env":"test","base":"myapp"}}"#;
    let req: DeployRequest = serde_json::from_str(json).unwrap();
    let env = req.environment.unwrap();
    assert_eq!(env.env, "test");
    assert_eq!(env.base, "myapp");
    assert_eq!(env.slug, None);
    // absent block (old CLI) still deserializes
    let json = r#"{"project":{"name":"myapp","repo":"r","branch":"b","compose":"c","service":"web","port":3000,"hostname":null},"ref":null}"#;
    let req: DeployRequest = serde_json::from_str(json).unwrap();
    assert!(req.environment.is_none());
}
```

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement** the entities/proto changes above, then run `rtk cargo build --locked` and fix every literal the compiler reports by adding the three default fields (`environment: None`, `on_create_done: false`, `last_success_at: None`).
- [ ] **Step 4: Run** full `rtk cargo test --locked` → PASS. **Step 5: Commit** `feat(proto): environment block in deploy request`.

### Task 9: Registry columns + repository methods

**Files:**
- Modify: `crates/infrastructure/src/sqlite.rs` (migration 5)
- Modify: `crates/infrastructure/src/repo.rs` (columns, `row_to_project`, upsert, new methods)
- Modify: `crates/domain/src/contracts.rs` (`ProjectRepository` — 3 new methods)

**Interfaces:**
- Produces (contracts.rs, inside `trait ProjectRepository`):

```rust
    /// Environment entries only (env_name set); base=None -> all bases.
    async fn list_environments(&self, base: Option<&str>) -> Result<Vec<Project>, DomainError>;
    /// Sets last_success_at (TTL sliding anchor).
    async fn mark_deploy_success(&self, name: &str, at: i64) -> Result<(), DomainError>;
    async fn set_on_create_done(&self, name: &str, done: bool) -> Result<(), DomainError>;
```

- [ ] **Step 1: Failing tests** (in `repo.rs` tests, following the file's existing test style with an in-memory `Db`):

```rust
fn env_config(name: &str, base: &str, env: &str) -> ProjectConfig {
    let mut config = sample_config(name); // reuse/extend the file's existing test helper
    config.environment = Some(pi_domain::entities::EnvironmentMeta {
        env: env.into(),
        base: base.into(),
        slug: None,
        ttl_secs: Some(3600),
        on_create: Some("seed".into()),
    });
    config
}

#[tokio::test]
async fn upsert_persists_environment_meta_and_flags_survive_update() {
    let repo = repo(); // existing helper building SqliteProjectRepo over an in-memory Db
    let p = repo.upsert(&env_config("myapp--test", "myapp", "test")).await.unwrap();
    let meta = p.config.environment.as_ref().unwrap();
    assert_eq!((meta.env.as_str(), meta.base.as_str()), ("test", "myapp"));
    assert!(!p.on_create_done);
    assert_eq!(p.last_success_at, None);

    repo.set_on_create_done("myapp--test", true).await.unwrap();
    repo.mark_deploy_success("myapp--test", 12345).await.unwrap();
    // re-upsert (new deploy) must NOT reset the runtime flags
    let p = repo.upsert(&env_config("myapp--test", "myapp", "test")).await.unwrap();
    assert!(p.on_create_done);
    assert_eq!(p.last_success_at, Some(12345));
}

#[tokio::test]
async fn list_environments_filters_by_base_and_excludes_base_projects() {
    let repo = repo();
    repo.upsert(&sample_config("myapp")).await.unwrap();
    repo.upsert(&env_config("myapp--test", "myapp", "test")).await.unwrap();
    repo.upsert(&env_config("other--test", "other", "test")).await.unwrap();
    let all = repo.list_environments(None).await.unwrap();
    assert_eq!(all.len(), 2);
    let mine = repo.list_environments(Some("myapp")).await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].config.name, "myapp--test");
}
```

(If `repo.rs` has no `sample_config(name)`/`repo()` test helpers, add them modeled on the existing tests in that file.)

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement.**
  1. `sqlite.rs` — append migration 5 to the `migrations()` vec:

```rust
M::up(
    "ALTER TABLE projects ADD COLUMN env_name TEXT;
     ALTER TABLE projects ADD COLUMN env_base TEXT;
     ALTER TABLE projects ADD COLUMN env_slug TEXT;
     ALTER TABLE projects ADD COLUMN env_ttl_secs INTEGER;
     ALTER TABLE projects ADD COLUMN env_on_create TEXT;
     ALTER TABLE projects ADD COLUMN env_on_create_done INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE projects ADD COLUMN last_success_at INTEGER;",
),
```

  2. `repo.rs` — extend the `SELECT` const with `, env_name, env_base, env_slug, env_ttl_secs, env_on_create, env_on_create_done, last_success_at` (columns 12..18); in `row_to_project` build:

```rust
let environment = match row.get::<_, Option<String>>(12)? {
    Some(env) => Some(EnvironmentMeta {
        env,
        base: row.get::<_, Option<String>>(13)?.unwrap_or_default(),
        slug: row.get(14)?,
        ttl_secs: row.get::<_, Option<i64>>(15)?.map(|v| v as u64),
        on_create: row.get(16)?,
    }),
    None => None,
};
// config.environment = environment;
// Project { config, host_port, created_at, on_create_done: row.get::<_, i64>(17)? != 0, last_success_at: row.get(18)? }
```

(Adjust indices to the actual SELECT order.) In `upsert` UPDATE/INSERT add the five `env_*` meta columns (from `config.environment`), leaving `env_on_create_done`/`last_success_at` untouched on UPDATE. New methods:

```rust
async fn list_environments(&self, base: Option<&str>) -> Result<Vec<Project>, DomainError> {
    // SELECT ... WHERE env_name IS NOT NULL [AND env_base = ?1] ORDER BY name
}
async fn mark_deploy_success(&self, name: &str, at: i64) -> Result<(), DomainError> {
    // UPDATE projects SET last_success_at = ?2 WHERE name = ?1
}
async fn set_on_create_done(&self, name: &str, done: bool) -> Result<(), DomainError> {
    // UPDATE projects SET env_on_create_done = ?2 WHERE name = ?1
}
```

  3. `contracts.rs` — add the three methods to the trait (automock updates itself). Fix non-Sqlite implementors if any exist (compiler will list them).

- [ ] **Step 4: Run** full `rtk cargo test --locked` → PASS. **Step 5: Commit** `feat(agent): environment metadata in the project registry`.

### Task 10: Deploy-time guards in `create_deployment`

**Files:**
- Modify: `crates/bin/src/agent/state.rs` (`AppState` += `pub projects: Arc<dyn ProjectRepository>`; `build_state` passes the existing `SqliteProjectRepo` Arc)
- Modify: `crates/bin/src/agent/http.rs` (`create_deployment`)

**Interfaces:**
- Consumes: `state.projects.get(name)`, `EnvironmentMeta` from Task 8.
- Produces: validation/guard behavior (below); `config.environment` populated before `scheduler.submit`.

- [ ] **Step 1: Failing tests** (http.rs test module already builds an `AppState` with mocks — follow its pattern):

```rust
#[tokio::test]
async fn deploy_env_key_mismatch_is_rejected() {
    // environment { env:"test", base:"myapp" } but project.name == "myapp--prod" -> 400
}

#[tokio::test]
async fn base_deploy_into_environment_key_is_conflict() {
    // registry get("myapp--test") returns a Project whose config.environment is Some(...)
    // request without environment block -> 409
}

#[tokio::test]
async fn env_deploy_into_base_key_is_conflict() {
    // registry get returns Project with environment None; request WITH environment block -> 409
}

#[tokio::test]
async fn base_name_with_double_dash_is_rejected_agent_side() {
    // no environment block, name "my--app" -> 400
}
```

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement** — in `create_deployment`, after the existing name/service/port/commands validation and before `DeployRef::parse`:

```rust
let env_meta: Option<EnvironmentMeta> = req.environment.map(Into::into);
match &env_meta {
    Some(env) => {
        if !is_valid_name(&env.base) || env.base.contains("--") {
            return Err(ApiError(DomainError::Invalid(
                "environment.base must match ^[a-z0-9][a-z0-9_-]*$ and must not contain '--'".into(),
            )));
        }
        let expected = match &env.slug {
            Some(slug) => format!("{}--{}--{}", env.base, env.env, slug),
            None => format!("{}--{}", env.base, env.env),
        };
        if expected != config.name {
            return Err(ApiError(DomainError::Invalid(format!(
                "project.name '{}' does not match the environment key '{expected}'",
                config.name
            ))));
        }
    }
    None => {
        if config.name.contains("--") {
            return Err(ApiError(DomainError::Invalid(
                "project.name must not contain '--' (reserved for environment keys; deploy with --env)".into(),
            )));
        }
    }
}
if let Some(existing) = state.projects.get(&config.name).await.map_err(ApiError)? {
    let existing_is_env = existing.config.environment.is_some();
    if existing_is_env != env_meta.is_some() {
        let (a, b) = if existing_is_env { ("an environment", "a base project") } else { ("a base project", "an environment") };
        return Err(ApiError(DomainError::Conflict(format!(
            "'{}' is registered as {a}; refusing to deploy it as {b}",
            config.name
        ))));
    }
}
let mut config = config;
config.environment = env_meta;
```

- [ ] **Step 4: Run** `rtk cargo test --locked -p pi` → PASS. **Step 5: Commit** `feat(agent): environment kind guards on deploy`.

### Task 11: `on_create` hook + success timestamp in the deploy pipeline

**Files:**
- Modify: `crates/application/src/deploy.rs`

**Interfaces:**
- Consumes: `project.on_create_done` (from `projects.upsert` result), `config.environment`, `self.runtime.exec`, `self.projects.set_on_create_done`, `self.projects.mark_deploy_success`.
- Produces: new deploy stage `"on_create"` (wire name rendered by the CLI pipeline automatically — stages are generic); `last_success_at` updated on every successful deploy.

- [ ] **Step 1: Failing tests** (deploy.rs tests):

```rust
fn env_sample_config(on_create: Option<&str>) -> ProjectConfig {
    let mut config = sample_config();
    config.name = "rateme--test".into();
    config.hostname = None;
    config.environment = Some(pi_domain::entities::EnvironmentMeta {
        env: "test".into(),
        base: "rateme".into(),
        slug: None,
        ttl_secs: None,
        on_create: on_create.map(String::from),
    });
    if let Some(cmd) = on_create {
        config.commands.insert(cmd.to_string(), pi_domain::entities::CommandSpec::new(vec!["node".into(), "seed.js".into()]));
    }
    config
}

#[tokio::test]
async fn on_create_runs_once_after_healthy_deploy_and_marks_done() {
    // upsert returns Project { on_create_done: false, .. }
    // expect runtime.exec called once with argv ["node","seed.js"], service "web"
    // expect projects.set_on_create_done("rateme--test", true) called once
    // expect projects.mark_deploy_success called once
    // deploy Success
}

#[tokio::test]
async fn on_create_skipped_when_already_done() {
    // upsert returns Project { on_create_done: true, .. } -> exec NOT called, deploy Success
}

#[tokio::test]
async fn on_create_nonzero_exit_fails_deploy_and_keeps_flag() {
    // exec returns Ok(1) -> deploy Failed, set_on_create_done never called,
    // stage events contain ("on_create", Failed)
}

#[tokio::test]
async fn successful_base_deploy_marks_last_success() {
    // plain sample_config() -> mark_deploy_success called once with finished_at
}
```

(Wire the mocks exactly like the file's existing tests; `m.projects.expect_upsert().returning(...)` must now return `Project { config, host_port, created_at, on_create_done, last_success_at }`.)

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement.**
  1. In `run_stages`, after the ingress/`route` block and before the `gc` stage:

```rust
// environments spec: on_create runs once, after the first fully successful
// deploy of the key (health + route done). Failure fails the deploy; the
// flag stays false so the next deploy retries.
if let Some(env) = &config.environment {
    if let (Some(cmd_name), false) = (&env.on_create, project.on_create_done) {
        let spec = config.commands.get(cmd_name).ok_or_else(|| {
            DomainError::Invalid(format!("on_create command '{cmd_name}' is not declared in [commands]"))
        })?;
        let service = spec.service.clone().unwrap_or_else(|| config.service.clone());
        let secs = config.command_timeout_secs.unwrap_or(600);
        let argv = spec.argv.clone();
        tracked(&log, "on_create", staged("on_create", secs, async {
            let code = self.runtime.exec(&stack, &service, &argv, log.clone()).await?;
            if code != 0 {
                return Err(DomainError::Runtime(format!(
                    "on_create '{cmd_name}' exited with code {code}"
                )));
            }
            Ok(())
        }))
        .await?;
        self.projects.set_on_create_done(&config.name, true).await?;
        log.line(&format!("on_create '{cmd_name}' completed"));
    }
}
```

  2. In `execute`, in the `Ok(commit_sha)` branch right before `record_finished`:

```rust
if let Err(err) = self.projects.mark_deploy_success(&config.name, finished_at).await {
    log.line(&format!("warning: could not record deploy success time: {err}"));
}
```

  Note: existing tests that use `mocks()` need `m.projects.expect_mark_deploy_success().returning(|_, _| Ok(()));` in the shared helpers (`mocks()` or `ok_pre_stages`) — add it there once.

- [ ] **Step 4: Run** `rtk cargo test --locked -p pi-application` → PASS, then full suite. **Step 5: Commit** `feat(agent): on_create hook and sliding success timestamp`.

### Task 12: Cloudflare DNS record deletion

**Files:**
- Modify: `crates/domain/src/contracts.rs` (`CloudflareApi` += `delete_tunnel_cname`)
- Modify: `crates/infrastructure/src/cloudflare.rs` (impl)
- Modify: `crates/infrastructure/src/cloudflared.rs` (`remove()` deletes DNS; also when the ingress rule was already absent)
- Modify: `crates/bin/src/cli/commands.rs` (`rm` note text)

**Interfaces:**
- Produces (contracts.rs):

```rust
    /// Delete the proxied CNAME for <name> if it exists. Absent record is Ok.
    async fn delete_tunnel_cname(&self, zone: &str, name: &str) -> Result<(), DomainError>;
```

- [ ] **Step 1: Failing tests.** `cloudflared.rs` has tests with a mock `CloudflareApi`; add:

```rust
#[tokio::test]
async fn remove_deletes_dns_record() {
    // existing remove-success fixture + expect delete_tunnel_cname(zone, hostname) called once
}

#[tokio::test]
async fn remove_without_matching_rule_still_deletes_dns() {
    // config.yml without the hostname rule -> no restart, but delete_tunnel_cname called once, Ok
}
```

`cloudflare.rs`: follow the file's existing HTTP-test approach if one exists (mock server); if the file has no HTTP tests, unit-test at the `cloudflared.rs` level only and rely on the uniform request pattern.

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement.**
  1. `cloudflare.rs` — copy the lookup from `put_tunnel_cname`:

```rust
async fn delete_tunnel_cname(&self, zone: &str, name: &str) -> Result<(), DomainError> {
    let zid = self.zone_id(zone).await?;
    let existing: serde_json::Value = self
        .client
        .get(format!("{}/zones/{zid}/dns_records?type=CNAME&name={name}", self.base))
        .bearer_auth(&self.token)
        .send().await.map_err(api_err)?
        .error_for_status().map_err(api_err)?
        .json().await.map_err(api_err)?;
    let Some(rid) = existing["result"][0]["id"].as_str() else {
        return Ok(()); // already absent
    };
    self.client
        .delete(format!("{}/zones/{zid}/dns_records/{rid}", self.base))
        .bearer_auth(&self.token)
        .send().await.map_err(api_err)?
        .error_for_status().map_err(api_err)?;
    Ok(())
}
```

  2. `cloudflared.rs` `remove()` — after the config-edit/restart path completes (and in the "no matching rule" early path, before returning), add:

```rust
self.cf.delete_tunnel_cname(&self.zone, hostname).await?;
log.line(&format!("DNS record for {hostname} removed"));
```

  `DisabledIngress::remove` stays a no-op.
  3. `commands.rs` `rm` — replace the "may still exist; delete it manually" note with:

```rust
output::note(format!(
    "if the agent has Cloudflare ingress enabled, the DNS record for {hostname} was removed; \
     otherwise delete it manually in the Cloudflare dashboard"
));
```

- [ ] **Step 4: Run** full `rtk cargo test --locked` → PASS. **Step 5: Commit** `feat(ingress): delete Cloudflare DNS record on project removal`.

### Task 13: Environment use-cases + agent endpoints

**Files:**
- Create: `crates/application/src/environments.rs`
- Modify: `crates/application/src/lib.rs` (`pub mod environments;`)
- Modify: `crates/bin/src/proto.rs` (`EnvironmentViewDto`, `EnvironmentActionResponse`)
- Modify: `crates/bin/src/agent/state.rs` (AppState += `list_envs`, `destroy_env`, `reset_env`; build in `build_state`)
- Modify: `crates/bin/src/agent/http.rs` (3 routes + handlers)

**Interfaces:**
- Produces (`environments.rs`):

```rust
pub struct ListEnvironments { projects: Arc<dyn ProjectRepository> }
impl ListEnvironments {
    pub fn new(projects: Arc<dyn ProjectRepository>) -> Arc<ListEnvironments>;
    pub async fn execute(&self, base: Option<&str>) -> Result<Vec<Project>, DomainError>; // pass-through of list_environments
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestroyOutcome { pub key: String, pub already_absent: bool }

pub struct DestroyEnvironment {
    projects: Arc<dyn ProjectRepository>,
    remove: Arc<RemoveProject>,
}
impl DestroyEnvironment {
    pub fn new(projects: Arc<dyn ProjectRepository>, remove: Arc<RemoveProject>) -> Arc<DestroyEnvironment>;
    /// Missing key -> Ok(already_absent). Base key -> Conflict. Active deploy -> Conflict (via RemoveProject).
    /// Delegates the actual teardown to RemoveProject::execute(key, remove_volumes=true).
    pub async fn execute(&self, key: &str, log: Arc<dyn LogSink>) -> Result<DestroyOutcome, DomainError>;
}

pub struct ResetEnvironmentData {
    projects: Arc<dyn ProjectRepository>,
    history: Arc<dyn DeploymentHistory>,
    runtime: Arc<dyn ContainerRuntime>,
    source: Arc<dyn Source>,
    overrides: Arc<dyn OverrideStore>,
}
impl ResetEnvironmentData {
    pub fn new(...) -> Arc<ResetEnvironmentData>;
    /// down -v (when compose file exists) + set_on_create_done(false).
    pub async fn execute(&self, key: &str, log: Arc<dyn LogSink>) -> Result<(), DomainError>;
}
```

- Produces (proto.rs):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentViewDto {
    pub key: String,
    pub base: String,
    pub env: String,
    #[serde(default)] pub slug: Option<String>,
    pub created_at: i64,
    #[serde(default)] pub last_success_at: Option<i64>,
    #[serde(default)] pub ttl_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentActionResponse {
    pub key: String,
    #[serde(default)] pub already_absent: bool,
}
```

- Produces (http.rs routes):
  - `GET /v1/environments` (`?base=<name>` via `Query<EnvListQuery>` where `struct EnvListQuery { base: Option<String> }`) → `Json<Vec<EnvironmentViewDto>>`
  - `DELETE /v1/environments/{key}` → `Json<EnvironmentActionResponse>`
  - `POST /v1/environments/{key}/reset-data` → `Json<EnvironmentActionResponse>`

- [ ] **Step 1: Failing tests** (`environments.rs` tests with mockall, mirroring `remove.rs`-style):

```rust
#[tokio::test]
async fn destroy_missing_key_is_ok_already_absent() { /* projects.get -> None */ }

#[tokio::test]
async fn destroy_base_key_is_conflict() { /* projects.get -> Some(project with environment None) */ }

#[tokio::test]
async fn destroy_environment_delegates_to_remove_with_volumes() {
    /* environment project; RemoveProject built from mocks: expect down(remove_volumes=true),
       ingress.remove (hostname present), source.cleanup, secrets.remove, overrides.remove,
       history.remove_project, projects.remove — same wiring as remove.rs tests */
}

#[tokio::test]
async fn reset_data_downs_volumes_and_clears_flag() {
    /* environment project; runtime.down(.., true, ..) once; projects.set_on_create_done(key, false) once */
}

#[tokio::test]
async fn reset_data_with_active_deploy_is_conflict() { /* history.active -> non-empty */ }

#[tokio::test]
async fn reset_data_on_base_key_is_conflict() { /* environment None -> Conflict */ }
```

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement** `environments.rs`:

```rust
use std::sync::Arc;

use pi_domain::contracts::{
    ContainerRuntime, DeploymentHistory, LogSink, OverrideStore, ProjectRepository, Source,
};
use pi_domain::entities::{ComposeStack, Project};
use pi_domain::error::DomainError;

use crate::remove::RemoveProject;

// ...structs from Interfaces above...

impl DestroyEnvironment {
    pub async fn execute(
        &self,
        key: &str,
        log: Arc<dyn LogSink>,
    ) -> Result<DestroyOutcome, DomainError> {
        let Some(existing) = self.projects.get(key).await? else {
            return Ok(DestroyOutcome { key: key.to_string(), already_absent: true });
        };
        if existing.config.environment.is_none() {
            return Err(DomainError::Conflict(format!(
                "'{key}' is a base project, not an environment - use `rpi rm` for base projects"
            )));
        }
        self.remove.execute(key, true, log).await?;
        Ok(DestroyOutcome { key: key.to_string(), already_absent: false })
    }
}

impl ResetEnvironmentData {
    pub async fn execute(&self, key: &str, log: Arc<dyn LogSink>) -> Result<(), DomainError> {
        let Some(existing) = self.projects.get(key).await? else {
            return Err(DomainError::NotFound(format!("environment {key}")));
        };
        if existing.config.environment.is_none() {
            return Err(DomainError::Conflict(format!(
                "'{key}' is a base project, not an environment"
            )));
        }
        if !self.history.active(key).await?.is_empty() {
            return Err(DomainError::Conflict(format!(
                "environment {key} has an active deployment; wait for it or cancel it first"
            )));
        }
        let workdir = self.source.workdir(key);
        let compose_file = workdir.join(&existing.config.compose_path);
        if compose_file.exists() {
            let stack = ComposeStack {
                project_name: existing.config.name.clone(),
                workdir,
                compose_file,
                override_file: self.overrides.path(key),
            };
            self.runtime.down(&stack, true, Arc::clone(&log)).await?;
        }
        self.projects.set_on_create_done(key, false).await?;
        Ok(())
    }
}
```

  Then proto DTOs, AppState fields, `build_state` wiring (construct with the same Arcs used for `RemoveProject`), and http handlers:

```rust
async fn list_environments_handler(
    State(state): State<AppState>,
    Query(q): Query<EnvListQuery>,
) -> Result<Json<Vec<EnvironmentViewDto>>, ApiError> {
    let envs = state.list_envs.execute(q.base.as_deref()).await.map_err(ApiError)?;
    Ok(Json(envs.into_iter().map(environment_view).collect()))
}

fn environment_view(p: Project) -> EnvironmentViewDto {
    let meta = p.config.environment.clone().expect("list_environments returns environments only");
    EnvironmentViewDto {
        key: p.config.name.clone(),
        base: meta.base,
        env: meta.env,
        slug: meta.slug,
        created_at: p.created_at,
        last_success_at: p.last_success_at,
        ttl_secs: meta.ttl_secs,
    }
}

async fn destroy_environment_handler(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<EnvironmentActionResponse>, ApiError> {
    if !is_valid_name(&key) {
        return Err(ApiError(DomainError::Invalid("invalid environment key".into())));
    }
    let outcome = state.destroy_env.execute(&key, Arc::new(TracingSink)).await.map_err(ApiError)?;
    Ok(Json(EnvironmentActionResponse { key: outcome.key, already_absent: outcome.already_absent }))
}

async fn reset_environment_handler(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<EnvironmentActionResponse>, ApiError> {
    if !is_valid_name(&key) {
        return Err(ApiError(DomainError::Invalid("invalid environment key".into())));
    }
    state.reset_env.execute(&key, Arc::new(TracingSink)).await.map_err(ApiError)?;
    Ok(Json(EnvironmentActionResponse { key, already_absent: false }))
}
```

  Routes: `.route("/v1/environments", get(list_environments_handler))`, `.route("/v1/environments/{key}", delete(destroy_environment_handler))`, `.route("/v1/environments/{key}/reset-data", post(reset_environment_handler))`.

- [ ] **Step 4: Run** full `rtk cargo test --locked` → PASS. **Step 5: Commit** `feat(agent): environment list/destroy/reset endpoints`.

### Task 14: CLI `rpi env` group + deploy sends the block + compat feature

**Files:**
- Modify: `crates/bin/src/compat.rs` (`Feature::Environments`)
- Modify: `crates/bin/src/cli/api.rs` (3 methods)
- Create: `crates/bin/src/cli/envcmds.rs`
- Modify: `crates/bin/src/cli/mod.rs` (`pub mod envcmds;`)
- Modify: `crates/bin/src/cli/commands.rs` (deploy attaches block)
- Modify: `crates/bin/src/main.rs` (`Env` subcommand)

**Interfaces:**
- compat: `Feature::Environments` — capability `"environments"`, label `"environments"`, policy `Required`, since `"0.24.0"`, appended to `Feature::ALL` (drift test `version_advertises_every_registered_feature` keeps `/v1/version` honest).
- api.rs:

```rust
pub async fn list_environments(&self, base: Option<&str>) -> anyhow::Result<Vec<EnvironmentViewDto>>;
pub async fn destroy_environment(&self, key: &str) -> anyhow::Result<EnvironmentActionResponse>;
pub async fn reset_environment(&self, key: &str) -> anyhow::Result<EnvironmentActionResponse>;
```

(GET `/v1/environments?base=...` / DELETE `/v1/environments/{key}` / POST `/v1/environments/{key}/reset-data`, each via `extract_error`; list/destroy/reset go through `expect_feature(resp, Feature::Environments)` like secrets does.)
- envcmds.rs:

```rust
pub async fn env_ls(all: bool, connect: ConnectOpts) -> anyhow::Result<()>;
pub async fn env_destroy(env: String, vars: Vec<String>, yes: bool, connect: ConnectOpts) -> anyhow::Result<()>;
pub async fn env_reset_data(env: String, vars: Vec<String>, yes: bool, connect: ConnectOpts) -> anyhow::Result<()>;
```

- main.rs:

```rust
/// Manage environments (overlays of rpi.toml)
Env {
    #[command(subcommand)]
    cmd: EnvCmd,
},

#[derive(clap::Subcommand)]
enum EnvCmd {
    /// List environments registered on the agent
    Ls {
        /// All environments on the agent, not only this project's
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Destroy an environment: stack, volumes, ingress, DNS, secrets, registry
    Destroy {
        env: String,
        #[arg(long = "vars")]
        vars: Vec<String>,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Remove the environment's volumes and re-run on_create on next deploy
    ResetData {
        env: String,
        #[arg(long = "vars")]
        vars: Vec<String>,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
}
```

- [ ] **Step 1: compat + api.** Add the `Environments` variant (arms in `capability()`, `label()`, `policy()` → `Policy::Required`, `since()` → `"0.24.0"`, append to `ALL`). Add the three `ApiClient` methods copying the `deploy`/`projects`/`cancel_deployment` patterns with `expect_feature(resp, Feature::Environments)`.
- [ ] **Step 2: envcmds.rs implementation:**

```rust
use crate::cli::config::ConnectOpts;
use crate::cli::connect::AgentConn;
use crate::output;

/// Derive the target key from ./rpi.toml + overlay: full resolution, so the
/// same validation applies as on deploy.
fn resolve_key(env: &str, vars: &[String]) -> anyhow::Result<String> {
    let resolved = crate::cli::overlay::resolve(Some(env), vars)?;
    Ok(resolved.env.expect("resolve with env returns selection").key)
}

fn confirm_key(action: &str, key: &str, yes: bool) -> anyhow::Result<()> {
    if yes {
        return Ok(());
    }
    output::warn(format!("this will {action} environment '{key}'"));
    eprint!("type the environment key to confirm: ");
    use std::io::Write;
    std::io::stderr().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if input.trim() != key {
        anyhow::bail!("confirmation failed: expected '{key}'");
    }
    Ok(())
}

pub async fn env_ls(all: bool, connect: ConnectOpts) -> anyhow::Result<()> {
    let base = if all {
        None
    } else {
        match crate::cli::overlay::resolve(None, &[]) {
            Ok(r) => Some(r.rpitoml.project.name),
            Err(_) => anyhow::bail!("no rpi.toml in the current directory - use `rpi env ls --all`"),
        }
    };
    let AgentConn { tunnel: _tunnel, api, compat } =
        crate::cli::connect::connect_agent(connect).await?;
    compat.gate(crate::compat::Feature::Environments)?;
    let envs = api.list_environments(base.as_deref()).await?;
    if envs.is_empty() {
        output::info("no environments registered");
        return Ok(());
    }
    let mut table = output::table();
    table.set_header(output::header(["KEY", "BASE", "ENV", "SLUG", "LAST DEPLOY", "TTL"]));
    for e in envs {
        table.add_row(vec![
            output::cell(e.key),
            output::cell(e.base),
            output::cell(e.env),
            output::cell(e.slug.unwrap_or_else(|| "-".into())),
            output::cell(e.last_success_at.map(|t| t.to_string()).unwrap_or_else(|| "-".into())),
            output::cell(e.ttl_secs.map(|t| format!("{t}s")).unwrap_or_else(|| "-".into())),
        ]);
    }
    println!("{table}");
    Ok(())
}

pub async fn env_destroy(env: String, vars: Vec<String>, yes: bool, connect: ConnectOpts) -> anyhow::Result<()> {
    let key = resolve_key(&env, &vars)?;
    confirm_key("DESTROY (stack, volumes, ingress, DNS, secrets, registry) of", &key, yes)?;
    let AgentConn { tunnel: _tunnel, api, compat } =
        crate::cli::connect::connect_agent(connect).await?;
    compat.gate(crate::compat::Feature::Environments)?;
    let resp = api.destroy_environment(&key).await?;
    if resp.already_absent {
        output::info(format!("environment '{key}' does not exist - nothing to destroy"));
    } else {
        output::success(format!("environment '{key}' destroyed"));
    }
    Ok(())
}

pub async fn env_reset_data(env: String, vars: Vec<String>, yes: bool, connect: ConnectOpts) -> anyhow::Result<()> {
    let key = resolve_key(&env, &vars)?;
    confirm_key("REMOVE ALL DATA (volumes) of", &key, yes)?;
    let AgentConn { tunnel: _tunnel, api, compat } =
        crate::cli::connect::connect_agent(connect).await?;
    compat.gate(crate::compat::Feature::Environments)?;
    api.reset_environment(&key).await?;
    output::success(format!(
        "environment '{key}' data removed - the next `rpi deploy --env {env}` re-runs on_create"
    ));
    Ok(())
}
```

- [ ] **Step 3: deploy attaches the block.** In `commands.rs::deploy`, replace `let _env_selection = resolved.env;` with:

```rust
let env_selection = resolved.env;
if env_selection.is_some() {
    compat.gate(crate::compat::Feature::Environments)?;
}
```

(after `connect_agent`), and build the request as:

```rust
let req = DeployRequest {
    project: (&project).into(),
    git_ref,
    environment: env_selection.as_ref().map(|s| crate::proto::EnvironmentDto {
        env: s.env.clone(),
        base: s.base.clone(),
        slug: s.slug.clone(),
        ttl_secs: s.ttl_secs,
        on_create: s.on_create.clone(),
    }),
};
```

Also update `deploy_cancel`'s `DeployRequest`-free flow (no change needed) and any other `DeployRequest { ... }` literal (compiler lists them; add `environment: None`).

- [ ] **Step 4: main.rs wiring** (enum + match arms calling `cli::envcmds::*`).
- [ ] **Step 5: Run** full `rtk cargo test --locked` + clippy → PASS. **Step 6: Commit** `feat(cli): rpi env group and environment-aware deploy`.

---

## Phase 3 — TTL + reaper

### Task 15: Reaper use-case + agent background task

**Files:**
- Modify: `crates/application/src/environments.rs` (add `ReapEnvironments`)
- Modify: `crates/bin/src/agent/config.rs` (`[environments] reap_interval`)
- Modify: `crates/bin/src/agent/state.rs` (build `ReapEnvironments`, expose from `build_state` or via AppState)
- Modify: `crates/bin/src/agent/run.rs` (spawn the loop)

**Interfaces:**
- Produces:

```rust
pub struct ReapEnvironments {
    projects: Arc<dyn ProjectRepository>,
    history: Arc<dyn DeploymentHistory>,
    destroy: Arc<DestroyEnvironment>,
    clock: Arc<dyn Clock>,
}
impl ReapEnvironments {
    pub fn new(...) -> Arc<ReapEnvironments>;
    /// One sweep. Returns the keys destroyed. Errors on one env are logged
    /// (tracing::warn) and do not stop the sweep; only listing errors bubble.
    pub async fn execute(&self) -> Result<Vec<String>, DomainError>;
}
```

- config.rs: `#[serde(default)] pub environments: EnvironmentsSection` with

```rust
#[derive(Debug, Default, Deserialize)]
pub struct EnvironmentsSection {
    /// Reaper tick interval ("1h", "30m", bare seconds). Default 1h.
    pub reap_interval: Option<String>,
}
```

and `AgentConfig::reap_interval_secs(&self) -> anyhow::Result<u64>` (parse via the agent-side duration parser used for `timeouts`; default 3600).

- [ ] **Step 1: Failing tests** (`environments.rs`):

```rust
#[tokio::test]
async fn reaper_destroys_only_expired_environments() {
    // list_environments returns:
    //  A: env, ttl 100, last_success_at Some(now-200)  -> destroyed
    //  B: env, ttl 100, last_success_at Some(now-50)   -> kept
    //  C: env, ttl None                                 -> kept
    //  D: env, ttl 100, last_success_at None, created_at now-200 -> destroyed (anchor = created_at)
    // history.active("") empty for all
}

#[tokio::test]
async fn reaper_skips_environments_with_active_deploys() {
    // expired env, history.active -> [one running deployment] -> destroy NOT called
}

#[tokio::test]
async fn reaper_continues_after_one_failed_destroy() {
    // two expired envs, destroy of the first errors -> second still destroyed; execute returns Ok
}
```

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement:**

```rust
impl ReapEnvironments {
    pub async fn execute(&self) -> Result<Vec<String>, DomainError> {
        let now = self.clock.now_unix();
        let mut destroyed = Vec::new();
        for p in self.projects.list_environments(None).await? {
            let Some(meta) = &p.config.environment else { continue };
            let Some(ttl) = meta.ttl_secs else { continue };
            let anchor = p.last_success_at.unwrap_or(p.created_at);
            if anchor + ttl as i64 > now {
                continue;
            }
            let key = p.config.name.clone();
            match self.history.active(&key).await {
                Ok(active) if !active.is_empty() => continue, // retry next tick
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!("reaper: cannot check active deploys of {key}: {err}");
                    continue;
                }
            }
            match self.destroy.execute(&key, Arc::new(crate::TracingSink)).await {
                Ok(_) => {
                    tracing::info!("reaper: environment {key} expired (ttl {ttl}s) and was removed");
                    destroyed.push(key);
                }
                Err(err) => tracing::warn!("reaper: destroying {key} failed: {err} (will retry)"),
            }
        }
        Ok(destroyed)
    }
}
```

(If `TracingSink` lives in the bin crate, add a tiny local `struct ReaperSink;` implementing `LogSink` via `tracing::info!` instead — check where `TracingSink` is defined (`agent/http.rs`) and do NOT import bin code into application; define `ReaperSink` in `environments.rs`.)

  run.rs — after `build_state`, before `axum::serve`:

```rust
let reap_secs = config.reap_interval_secs()?;
{
    let reaper = pi_application::environments::ReapEnvironments::new(
        state.projects.clone(),
        state.history.clone(),
        state.destroy_env.clone(),
        Arc::new(pi_infrastructure::sys::SystemClock::new()),
    );
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(reap_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if let Err(err) = reaper.execute().await {
                tracing::warn!("environment reaper sweep failed: {err}");
            }
        }
    });
}
```

(Adapt the `SystemClock` path to wherever `run.rs` already gets its clock for `sweep_interrupted`.)

- [ ] **Step 4: Run** full suite → PASS. **Step 5: Commit** `feat(agent): TTL reaper for environments`.

### Task 16: E2E scenarios

**Files:**
- Create: `tests/e2e/scenarios/env-overlay/scenario.sh`
- Create: `tests/e2e/scenarios/env-overlay/app/` (copy `tests/e2e/app.default/` and add `rpi.test.toml`, a `seed` command, and a volume)
- Create: `tests/e2e/scenarios/env-ttl/scenario.sh`
- Create: `tests/e2e/scenarios/env-ttl/app/` (adds `rpi.temp.toml` with a short ttl)
- Create: `tests/e2e/scenarios/env-ttl/agent.toml` (short reap interval; copy `tests/e2e/agent.default.toml` + `[environments] reap_interval = "5s"`)

**Interfaces:**
- Consumes: harness conventions — `scenario.sh` sources `/opt/e2e/lib.sh`, calls `e2e_bootstrap`, uses `run_capture`/`assert_log`/`assert_deploy_log`, ends with `echo 'rpi e2e: PASS'`. Fixture app dir is the scenario working dir.

- [ ] **Step 1: env-overlay fixture.** In `app/`: extend the default fixture's `compose.yaml` with a named volume mounted at `/data` on `web`; add to `rpi.toml`:

```toml
[commands]
seed = "sh -c 'test ! -f /data/seeded && date > /data/seeded'"
```

Add `app/rpi.test.toml`:

```toml
[environment]
on_create = "seed"
```

- [ ] **Step 2: env-overlay scenario.** `scenario.sh` outline (follow `happy-path/scenario.sh` style):

```bash
#!/usr/bin/env bash
set -euo pipefail
source /opt/e2e/lib.sh
e2e_bootstrap

run_capture deploy-base rpi deploy "${CONNECT[@]}"
assert_deploy_log deploy-base

run_capture deploy-env rpi deploy --env test "${CONNECT[@]}"
assert_deploy_log deploy-env
assert_log deploy-env "on_create 'seed'"

run_capture ls rpi ls "${CONNECT[@]}"
assert_log ls "e2e-fixture"
assert_log ls "e2e-fixture--test"

# redeploy: on_create must NOT run again
run_capture deploy-env-2 rpi deploy --env test "${CONNECT[@]}"
if grep -Fq "on_create 'seed'" /artifacts/deploy-env-2; then
  fail "on_create ran twice"
fi

run_capture env-ls rpi env ls "${CONNECT[@]}"
assert_log env-ls "e2e-fixture--test"

# reset-data wipes the volume; next deploy seeds again
run_capture reset rpi env reset-data test --yes "${CONNECT[@]}"
run_capture deploy-env-3 rpi deploy --env test "${CONNECT[@]}"
assert_log deploy-env-3 "on_create 'seed'"

run_capture destroy rpi env destroy test --yes "${CONNECT[@]}"
assert_log destroy "destroyed"
run_capture destroy-again rpi env destroy test --yes "${CONNECT[@]}"
assert_log destroy-again "nothing to destroy"
run_capture env-ls-2 rpi env ls "${CONNECT[@]}"
assert_log env-ls-2 "no environments"

# base project untouched
run_capture ls-2 rpi ls "${CONNECT[@]}"
assert_log ls-2 "e2e-fixture"

echo 'rpi e2e: PASS'
```

- [ ] **Step 3: env-ttl scenario.** `app/rpi.temp.toml`:

```toml
[environment]
ttl = "10s"
```

`agent.toml` = copy of `tests/e2e/agent.default.toml` plus:

```toml
[environments]
reap_interval = "5s"
```

`scenario.sh`: deploy `--env temp`, assert it appears in `rpi env ls`, then poll (up to 60s, sleep 5) until `rpi env ls` no longer lists `e2e-fixture--temp`; assert base project still listed in `rpi ls`; `PASS`.

- [ ] **Step 4: Run locally** (needs Docker): `npm run test:e2e -- --only env-overlay` if the runner supports filtering (check `tests/e2e/run.mjs` flags; otherwise run the full suite) → PASS.
- [ ] **Step 5: Commit** `test(e2e): environment overlay and ttl scenarios`.

### Task 17: Documentation

**Files:**
- Create: `docs/architecture/flows/environments.md` (new flow doc per the `architecture-diagrams` skill conventions: Mermaid diagram + walkthrough + Source anchors)
- Modify: `docs/architecture/overview.md` (mention environments where the deploy/remove flows are enumerated)
- Modify: `docs/architecture/flows/deploy.md` (on_create stage, environment guards)
- Modify: `.claude/skills/rpi-toml/SKILL.md` (overlay files, `[environment]`, merge rules)
- Modify: `.claude/skills/rpi-cli/SKILL.md` (`--env/--vars`, `rpi env`, `rpi config show`)
- Modify: `docs/potential-features/environment-overlays.md` (add a line: superseded by the 2026-07-24 spec)

- [ ] **Step 1:** Invoke the `architecture-diagrams` skill and follow its update procedure for the affected docs (deploy flow, overview, new environments flow: CLI resolve → deploy with env block → registry/kind guard → on_create → reaper/destroy teardown).
- [ ] **Step 2:** Update the two skills with the new commands/flags and overlay schema summary.
- [ ] **Step 3:** `rtk cargo fmt --all -- --check && rtk cargo clippy --all-targets --locked -- -D warnings && rtk cargo test --locked` → all PASS.
- [ ] **Step 4: Commit** `docs: environment overlays architecture and skill updates`.

---

## Plan Self-Review Notes

- Spec coverage: config model (T1, T4), interpolation (T3), slug/keys (T2), resolution+errors (T5), `config show` (T6), CLI flags (T7), wire+guards (T8, T10), registry (T9), on_create (T11), DNS delete (T12), endpoints+use-cases (T13), `rpi env`+compat (T14), reaper (T15), e2e (T16), docs (T17). Safety invariants: 1 → T5/T10; 2 → inherited per-key secrets (no code change needed, asserted in e2e); 3 → T15 filter; 4 → T3; 5 → T14 confirmations.
- Type consistency: `EnvironmentMeta`/`EnvironmentDto`/`EnvSelection` field names match across T5/T8/T10/T14; repository methods `list_environments`/`mark_deploy_success`/`set_on_create_done` used in T9/T11/T13/T15 with identical signatures.
- The exact mock/test wiring inside agent-side test skeletons (T10, T13, T15) follows the named existing tests in each file; implementers must copy those fixtures rather than invent new ones.
