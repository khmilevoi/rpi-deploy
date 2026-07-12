# Unified CLI ↔ Agent Compatibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One encapsulated abstraction (`compat` module) through which the CLI learns which features the agent supports, applies a per-feature policy when one is missing, and owns every version/compatibility notice.

**Architecture:** The agent's `/v1/version` response gains a flat `features` capability list generated from a shared `Feature` enum (CLI and agent are one binary, `crates/bin`). The CLI builds a `CompatSession` once per command via a new `connect_agent` helper; call sites replace ad-hoc 404 sniffing with `session.gate(Feature)`. A frozen version matrix reconstructs capabilities for agents that predate the handshake field. Cross-version e2e builds a legacy agent from git tag `v0.17.1`.

**Tech Stack:** Rust (axum, reqwest, serde, anyhow, tokio), Node test runner + Docker Compose for e2e.

**Spec:** `docs/superpowers/specs/2026-07-12-compat-versioning-design.md`

## Global Constraints

- Run checks through rtk: `rtk cargo fmt --all -- --check`, `rtk cargo clippy --all-targets --locked -- -D warnings`, `rtk cargo test --locked` must all pass before a task is complete. If fmt reports a diff, run `rtk cargo fmt --all` and include the result in the commit.
- Prefix git commands with `rtk` (`rtk git add`, `rtk git commit`).
- Capability strings (verbatim): `secrets`, `commands`, `source-check`, `stats`.
- Version floors (verbatim, from git history): secrets `0.9.0`, commands `0.9.0`, stats `0.9.0`, source-check `0.18.0`.
- npm update hint (verbatim): `npm i -g rpi-deploy@latest`.
- Legacy e2e tag (verbatim): `v0.17.1`.
- Message punctuation: plain ASCII (` - `, `>=`), matching existing output style.
- Code comments and commit messages in English.
- Host is Windows; use the Bash tool for POSIX one-liners, and remember `.sh` files must keep LF endings (the e2e Dockerfile strips CR, but keep them clean in git anyway — `.gitattributes` is not part of this plan).

---

### Task 1: `compat` module core — Feature registry, version parsing, legacy matrix

**Files:**
- Create: `crates/bin/src/compat.rs`
- Modify: `crates/bin/src/main.rs` (add `mod compat;` next to the existing `mod proto;` declaration block, lines 1–5)

**Interfaces:**
- Produces: `crate::compat::Feature` (enum: `Secrets`, `Commands`, `SourceCheck`, `Stats`) with `capability() -> &'static str`, `label() -> &'static str`, `policy() -> Policy`, `since() -> &'static str`, `Feature::ALL: &'static [Feature]`, `Feature::advertised() -> Vec<String>`.
- Produces: `crate::compat::Policy` (enum: `Required`, `Degradable`, `Silent`).
- Produces: `crate::compat::legacy_capabilities(agent_version: &str) -> BTreeSet<String>`.
- Produces (private, used by Task 3 in the same file): `parse_version(&str) -> Option<(u64, u64, u64)>`, `version_at_least(&str, &str) -> bool`.

- [ ] **Step 1: Write the failing tests**

Create `crates/bin/src/compat.rs` with only the test module first:

```rust
//! Unified CLI <-> agent compatibility (spec 2026-07-12).
//!
//! The single source of truth for which semantic features exist, which
//! capability string each one advertises in the `/v1/version` handshake,
//! and what happens when the other side lacks one.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_strings_are_stable() {
        assert_eq!(Feature::Secrets.capability(), "secrets");
        assert_eq!(Feature::Commands.capability(), "commands");
        assert_eq!(Feature::SourceCheck.capability(), "source-check");
        assert_eq!(Feature::Stats.capability(), "stats");
    }

    #[test]
    fn advertised_covers_every_feature() {
        let adv = Feature::advertised();
        for f in Feature::ALL {
            assert!(adv.contains(&f.capability().to_string()), "{} missing", f.capability());
        }
    }

    #[test]
    fn version_parsing_tolerates_prefix_and_suffix() {
        assert_eq!(parse_version("0.19.1"), Some((0, 19, 1)));
        assert_eq!(parse_version("v0.19.1"), Some((0, 19, 1)));
        assert_eq!(parse_version("1.2.3-dev+abc"), Some((1, 2, 3)));
        assert_eq!(parse_version("not-a-version"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn version_at_least_compares_numerically() {
        assert!(version_at_least("0.18.0", "0.18.0"));
        assert!(version_at_least("0.19.1", "0.18.0"));
        assert!(!version_at_least("0.17.1", "0.18.0"));
        // 0.10 > 0.9 numerically (string compare would get this wrong)
        assert!(version_at_least("0.10.0", "0.9.0"));
        // unparseable versions never satisfy a floor
        assert!(!version_at_least("garbage", "0.1.0"));
    }

    #[test]
    fn legacy_matrix_reconstructs_capabilities_by_version() {
        let caps = legacy_capabilities("0.17.1");
        assert!(caps.contains("secrets"));
        assert!(caps.contains("commands"));
        assert!(caps.contains("stats"));
        assert!(!caps.contains("source-check"), "source-check ships in 0.18.0");

        let caps = legacy_capabilities("0.18.0");
        assert!(caps.contains("source-check"));

        assert!(legacy_capabilities("unknown").is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk cargo test --locked -p pi compat`
Expected: FAIL to compile — `Feature`, `parse_version`, etc. not defined. (Also add `mod compat;` to `crates/bin/src/main.rs` now or the module is not even compiled.)

- [ ] **Step 3: Write the implementation**

Above the test module in `crates/bin/src/compat.rs`:

```rust
use std::collections::BTreeSet;

/// What happens when a feature is missing on the other side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Error out with an update hint.
    Required,
    /// One-shot warning banner, then the caller takes its fallback path.
    Degradable,
    /// Skip quietly (spec 2026-07-10 mandates this for source-check).
    Silent,
}

/// A semantic feature of the CLI<->agent protocol. One feature may cover
/// several routes. A breaking change to a feature is a NEW variant with a
/// `name/2`-style capability string — never a mutation of the old one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    Secrets,
    Commands,
    SourceCheck,
    Stats,
}

impl Feature {
    pub const ALL: &'static [Feature] = &[
        Feature::Secrets,
        Feature::Commands,
        Feature::SourceCheck,
        Feature::Stats,
    ];

    /// The string this feature advertises in the `/v1/version` handshake.
    pub fn capability(self) -> &'static str {
        match self {
            Feature::Secrets => "secrets",
            Feature::Commands => "commands",
            Feature::SourceCheck => "source-check",
            Feature::Stats => "stats",
        }
    }

    /// Human name used in user-facing messages.
    pub fn label(self) -> &'static str {
        match self {
            Feature::Secrets => "secrets",
            Feature::Commands => "container commands",
            Feature::SourceCheck => "deploy-key preflight",
            Feature::Stats => "stats",
        }
    }

    pub fn policy(self) -> Policy {
        match self {
            Feature::Secrets => Policy::Required,
            Feature::Commands => Policy::Required,
            Feature::SourceCheck => Policy::Silent,
            Feature::Stats => Policy::Required,
        }
    }

    /// First released agent version that serves this feature — powers the
    /// "requires agent >= X" hint.
    pub fn since(self) -> &'static str {
        match self {
            Feature::Secrets => "0.9.0",
            Feature::Commands => "0.9.0",
            Feature::SourceCheck => "0.18.0",
            Feature::Stats => "0.9.0",
        }
    }

    /// Everything THIS binary serves when it runs as the agent. Dropping a
    /// feature generation later means removing it from this list only.
    pub fn advertised() -> Vec<String> {
        Feature::ALL.iter().map(|f| f.capability().to_string()).collect()
    }
}

/// FROZEN (spec 2026-07-12): capabilities of agents that predate the
/// `features` handshake field, keyed by the version that introduced them.
/// This table never grows — every feature shipped after the handshake is
/// advertised explicitly, so the table dies out with old agents.
const LEGACY_MATRIX: &[(&str, &str)] = &[
    ("secrets", "0.9.0"),
    ("commands", "0.9.0"),
    ("stats", "0.9.0"),
    ("source-check", "0.18.0"),
];

/// Capability set of a pre-handshake agent, inferred from its version.
/// An unparseable version yields the empty set: Required features then fail
/// with an update hint instead of guessing.
pub fn legacy_capabilities(agent_version: &str) -> BTreeSet<String> {
    LEGACY_MATRIX
        .iter()
        .filter(|(_, since)| version_at_least(agent_version, since))
        .map(|(cap, _)| (*cap).to_string())
        .collect()
}

/// Lenient x.y.z parser: tolerates a leading `v` and `-`/`+` suffixes.
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = match parts.next() {
        Some(p) => p.parse().ok()?,
        None => return None,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn version_at_least(version: &str, floor: &str) -> bool {
    match (parse_version(version), parse_version(floor)) {
        (Some(a), Some(b)) => a >= b,
        _ => false,
    }
}
```

Note: `parse_version("1.2")` returns `None` by design (three components required); the test suite pins the accepted shapes.

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test --locked -p pi compat`
Expected: PASS (5 tests). Expect `dead_code` warnings for not-yet-used items — silence them by adding `#[allow(dead_code)]` NOWHERE; instead check clippy at the end of the task: if `-D warnings` trips on dead code, add `pub(crate)` usage does not exist yet, so put `#![allow(dead_code)]` at the top of `compat.rs` with a `// TODO(task-5): remove` comment, and REMOVE it in Task 5 when call sites land.

- [ ] **Step 5: Gate and commit**

```bash
rtk cargo fmt --all
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
rtk git add crates/bin/src/compat.rs crates/bin/src/main.rs
rtk git commit -m "feat(compat): feature registry, version parsing, frozen legacy matrix"
```

---

### Task 2: Handshake protocol — agent advertises `features`

**Files:**
- Modify: `crates/bin/src/proto.rs:12-16` (`VersionInfo`)
- Modify: `crates/bin/src/agent/http.rs:86-91` (`version` handler)
- Test: `crates/bin/src/agent/http.rs` (tests module, near the existing `/v1/version` test around line 1035)

**Interfaces:**
- Consumes: `crate::compat::Feature::advertised()` (Task 1).
- Produces: `VersionInfo { version: String, api: String, features: Option<Vec<String>> }` — `features` is `None` when deserializing an old agent's response (missing field), `Some(list)` from a current agent. Tasks 3–5 rely on exactly this `Option` semantics.

- [ ] **Step 1: Write the failing test**

In the tests module of `crates/bin/src/agent/http.rs`, next to the existing `/v1/version` test (~line 1035; reuse the same request helpers that test uses):

```rust
/// Drift guard (spec 2026-07-12): the agent must advertise exactly the
/// registered feature set — shipping a feature without registering it, or
/// hand-editing the handler, fails here.
#[tokio::test]
async fn version_advertises_every_registered_feature() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(state_with(
        dir.path(),
        Arc::new(ok_source()),
        Arc::new(ok_runtime()),
    ));
    let (status, json) = request(app, get_req("/v1/version")).await;
    assert_eq!(status, StatusCode::OK);
    let advertised: Vec<String> = json["features"]
        .as_array()
        .expect("features array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(advertised, crate::compat::Feature::advertised());
}
```

(Same fixtures as the sibling `version_handshake` test at ~line 1030: `router`, `state_with`, `ok_source`, `ok_runtime`, `request`, `get_req`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `rtk cargo test --locked -p pi version_advertises`
Expected: FAIL — `json["features"]` is null (field absent), `.as_array()` panics with "features array".

- [ ] **Step 3: Implement**

`crates/bin/src/proto.rs` — extend `VersionInfo`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub api: String,
    /// `None`: the agent predates the capability handshake (spec
    /// 2026-07-12) — reconstruct via `compat::legacy_capabilities`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<Vec<String>>,
}
```

`crates/bin/src/agent/http.rs` — the handler:

```rust
async fn version() -> Json<VersionInfo> {
    Json(VersionInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        api: "v1".to_string(),
        features: Some(crate::compat::Feature::advertised()),
    })
}
```

Fix any other `VersionInfo { ... }` construction sites the compiler reports (add `features: None` in CLI-side test fixtures if any exist).

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test --locked -p pi`
Expected: PASS, including the existing `/v1/version` test (`api == "v1"` still holds) and the new drift guard.

- [ ] **Step 5: Gate and commit**

```bash
rtk cargo fmt --all
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
rtk git add crates/bin/src/proto.rs crates/bin/src/agent/http.rs
rtk git commit -m "feat(agent): advertise feature capabilities in /v1/version handshake"
```

---

### Task 3: `CompatSession` — gates, banners, direction-aware hints

**Files:**
- Modify: `crates/bin/src/compat.rs` (append; tests go into the existing tests module)

**Interfaces:**
- Consumes: `VersionInfo` (Task 2), `Feature`/`Policy`/`legacy_capabilities` (Task 1).
- Produces (Tasks 4–5 use these exact signatures):
  - `CompatSession::new(cli_version: &str, info: &VersionInfo) -> CompatSession` (warns via `crate::output::warn`)
  - `CompatSession::with_sink(cli_version: &str, info: &VersionInfo, sink: Box<dyn Fn(&str) + Send + Sync>) -> CompatSession`
  - `supports(&self, Feature) -> bool`
  - `gate(&self, Feature) -> anyhow::Result<bool>` — `Ok(true)` available; missing: `Required` → `Err`, `Degradable` → warn once + `Ok(false)`, `Silent` → `Ok(false)`
  - `pick(&self, &[Feature]) -> Option<Feature>` — first supported, in preference order
  - `emit_version_banners(&self)` — version-skew banners, deduped
  - `agent_version(&self) -> &str`, `agent_api(&self) -> &str`
  - Free function `feature_unavailable_error(feature: Feature) -> anyhow::Error` — version-less text for the transport safety net (Task 4)

- [ ] **Step 1: Write the failing tests**

Append to the tests module in `crates/bin/src/compat.rs`:

```rust
use crate::proto::VersionInfo;
use std::sync::{Arc, Mutex};

fn info(version: &str, features: Option<&[&str]>) -> VersionInfo {
    VersionInfo {
        version: version.to_string(),
        api: "v1".to_string(),
        features: features.map(|f| f.iter().map(|s| s.to_string()).collect()),
    }
}

fn session(cli: &str, info: &VersionInfo) -> (CompatSession, Arc<Mutex<Vec<String>>>) {
    let warnings: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = warnings.clone();
    let s = CompatSession::with_sink(
        cli,
        info,
        Box::new(move |m| sink.lock().unwrap().push(m.to_string())),
    );
    (s, warnings)
}

#[test]
fn handshake_features_win_over_legacy_matrix() {
    // An agent that explicitly advertises only `stats` supports exactly that,
    // whatever its version says.
    let (s, _) = session("0.20.0", &info("0.20.0", Some(&["stats"])));
    assert!(s.supports(Feature::Stats));
    assert!(!s.supports(Feature::Secrets));
}

#[test]
fn legacy_agent_falls_back_to_version_matrix() {
    let (s, _) = session("0.20.0", &info("0.17.1", None));
    assert!(s.supports(Feature::Secrets));
    assert!(!s.supports(Feature::SourceCheck));
}

#[test]
fn gate_required_missing_is_error_with_update_agent_hint() {
    let (s, warnings) = session("0.20.0", &info("0.17.1", Some(&[])));
    let err = s.gate(Feature::Secrets).unwrap_err().to_string();
    assert!(err.contains("secrets"), "{err}");
    assert!(err.contains(">= 0.9.0"), "{err}");
    assert!(err.contains("update the agent on the Pi"), "{err}");
    assert!(warnings.lock().unwrap().is_empty());
}

#[test]
fn gate_required_missing_on_newer_agent_says_update_cli() {
    let (s, _) = session("0.20.0", &info("0.30.0", Some(&[])));
    let err = s.gate(Feature::Secrets).unwrap_err().to_string();
    assert!(err.contains("update the CLI"), "{err}");
    assert!(err.contains("npm i -g rpi-deploy@latest"), "{err}");
}

#[test]
fn gate_silent_missing_is_quiet_false() {
    let (s, warnings) = session("0.20.0", &info("0.17.1", None));
    assert!(!s.gate(Feature::SourceCheck).unwrap());
    assert!(warnings.lock().unwrap().is_empty());
}

#[test]
fn gate_available_is_true() {
    let (s, _) = session("0.20.0", &info("0.20.0", Some(&["secrets"])));
    assert!(s.gate(Feature::Secrets).unwrap());
}

#[test]
fn pick_returns_first_supported_preference() {
    let (s, _) = session("0.20.0", &info("0.20.0", Some(&["secrets"])));
    assert_eq!(
        s.pick(&[Feature::Stats, Feature::Secrets]),
        Some(Feature::Secrets)
    );
    assert_eq!(s.pick(&[Feature::Stats]), None);
}

#[test]
fn version_banner_older_agent_and_dedup() {
    let (s, warnings) = session("0.20.0", &info("0.17.1", None));
    s.emit_version_banners();
    s.emit_version_banners();
    let w = warnings.lock().unwrap();
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("update the agent on the Pi"), "{}", w[0]);
}

#[test]
fn version_banner_newer_agent_says_update_cli() {
    let (s, warnings) = session("0.20.0", &info("0.30.0", Some(&[])));
    s.emit_version_banners();
    let w = warnings.lock().unwrap();
    assert!(w[0].contains("update the CLI: npm i -g rpi-deploy@latest"), "{}", w[0]);
}

#[test]
fn version_banner_equal_versions_is_silent() {
    let (s, warnings) = session("0.20.0", &info("0.20.0", Some(&[])));
    s.emit_version_banners();
    assert!(warnings.lock().unwrap().is_empty());
}

#[test]
fn version_banner_same_parse_but_different_strings_warns_generically() {
    // "0.20.0-dirty" parses to the same (0,20,0) triple, but the strings
    // differ — warn generically rather than staying silent.
    let (s, warnings) = session("0.20.0", &info("0.20.0-dirty", Some(&[])));
    s.emit_version_banners();
    let w = warnings.lock().unwrap();
    assert_eq!(w.len(), 1);
    assert!(w[0].contains("differ"), "{}", w[0]);
}

#[test]
fn version_banner_unparseable_agent_version_warns_generically() {
    let (s, warnings) = session("0.20.0", &info("garbage", Some(&[])));
    s.emit_version_banners();
    let w = warnings.lock().unwrap();
    assert_eq!(w.len(), 1);
    assert!(w[0].contains("differ"), "{}", w[0]);
}

#[test]
fn safety_net_error_matches_gate_wording() {
    let err = feature_unavailable_error(Feature::Commands).to_string();
    assert!(err.contains("container commands"), "{err}");
    assert!(err.contains(">= 0.9.0"), "{err}");
    assert!(err.contains("update the agent on the Pi"), "{err}");
}
```

Note the wording contract: `0.20.0` vs `0.20.0-dirty` parse to the same triple `(0,20,0)` but the strings differ → generic "differ" banner. That is intended: equal-parsing-but-different strings warn generically, byte-equal strings stay silent.

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk cargo test --locked -p pi compat`
Expected: FAIL to compile — `CompatSession` not defined.

- [ ] **Step 3: Implement**

Append to `crates/bin/src/compat.rs` (above the tests module):

```rust
use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::Mutex;

use crate::proto::VersionInfo;

/// Version-less variant of the missing-feature error, for the transport
/// safety net (agent swapped between handshake and call, spec 2026-07-12).
pub fn feature_unavailable_error(feature: Feature) -> anyhow::Error {
    anyhow::anyhow!(
        "agent does not support {} (requires agent >= {}) - update the agent on the Pi",
        feature.label(),
        feature.since()
    )
}

/// One agent connection's negotiated compatibility. Built once per command
/// by `cli::connect::connect_agent`; every availability check and every
/// version/compat notice goes through here.
pub struct CompatSession {
    cli_version: String,
    agent_version: String,
    agent_api: String,
    capabilities: BTreeSet<String>,
    /// Dedup: each banner key prints at most once per command run.
    warned: Mutex<HashSet<String>>,
    sink: Box<dyn Fn(&str) + Send + Sync>,
}

impl CompatSession {
    pub fn new(cli_version: &str, info: &VersionInfo) -> CompatSession {
        Self::with_sink(cli_version, info, Box::new(|m| crate::output::warn(m)))
    }

    pub fn with_sink(
        cli_version: &str,
        info: &VersionInfo,
        sink: Box<dyn Fn(&str) + Send + Sync>,
    ) -> CompatSession {
        let capabilities = match &info.features {
            Some(list) => list.iter().cloned().collect(),
            None => legacy_capabilities(&info.version),
        };
        CompatSession {
            cli_version: cli_version.to_string(),
            agent_version: info.version.clone(),
            agent_api: info.api.clone(),
            capabilities,
            warned: Mutex::new(HashSet::new()),
            sink,
        }
    }

    pub fn agent_version(&self) -> &str {
        &self.agent_version
    }

    pub fn agent_api(&self) -> &str {
        &self.agent_api
    }

    /// Raw capability membership — no side effects, no policy.
    pub fn supports(&self, feature: Feature) -> bool {
        self.capabilities.contains(feature.capability())
    }

    /// Apply the feature's declared policy. `Ok(true)` — go ahead;
    /// `Ok(false)` — take the fallback path (Degradable already warned once,
    /// Silent stayed quiet); `Err` — Required feature missing.
    pub fn gate(&self, feature: Feature) -> anyhow::Result<bool> {
        if self.supports(feature) {
            return Ok(true);
        }
        match feature.policy() {
            Policy::Required => Err(self.missing_error(feature)),
            Policy::Degradable => {
                self.warn_once(
                    feature.capability(),
                    &format!(
                        "{} is not available on agent v{} (requires agent >= {}) - skipping",
                        feature.label(),
                        self.agent_version,
                        feature.since()
                    ),
                );
                Ok(false)
            }
            Policy::Silent => Ok(false),
        }
    }

    /// Best supported generation, in preference order (newest first).
    pub fn pick(&self, preference: &[Feature]) -> Option<Feature> {
        preference.iter().copied().find(|f| self.supports(*f))
    }

    /// Version-skew banners (§9.1 successor): agent older -> update agent,
    /// agent newer -> update CLI, unparseable-but-different -> generic.
    pub fn emit_version_banners(&self) {
        if self.agent_version == self.cli_version {
            return;
        }
        let msg = match (
            parse_version(&self.agent_version),
            parse_version(&self.cli_version),
        ) {
            (Some(agent), Some(cli)) => match agent.cmp(&cli) {
                Ordering::Less => format!(
                    "CLI v{} is newer than agent v{} - update the agent on the Pi",
                    self.cli_version, self.agent_version
                ),
                Ordering::Greater => format!(
                    "agent v{} is newer than CLI v{} - update the CLI: npm i -g rpi-deploy@latest",
                    self.agent_version, self.cli_version
                ),
                Ordering::Equal => format!(
                    "CLI v{} and agent v{} differ - update the agent on the Pi",
                    self.cli_version, self.agent_version
                ),
            },
            _ => format!(
                "CLI v{} and agent v{} differ - update the agent on the Pi",
                self.cli_version, self.agent_version
            ),
        };
        self.warn_once("version-skew", &msg);
    }

    fn missing_error(&self, feature: Feature) -> anyhow::Error {
        let agent_newer = matches!(
            (
                parse_version(&self.agent_version),
                parse_version(&self.cli_version),
            ),
            (Some(a), Some(c)) if a > c
        );
        if agent_newer {
            anyhow::anyhow!(
                "{} is not supported by agent v{}; CLI v{} is too old - update the CLI: npm i -g rpi-deploy@latest",
                feature.label(),
                self.agent_version,
                self.cli_version
            )
        } else {
            anyhow::anyhow!(
                "{} is not available on agent v{} (requires agent >= {}) - update the agent on the Pi",
                feature.label(),
                self.agent_version,
                feature.since()
            )
        }
    }

    fn warn_once(&self, key: &str, msg: &str) {
        if self.warned.lock().unwrap().insert(key.to_string()) {
            (self.sink)(msg);
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test --locked -p pi compat`
Expected: PASS (all Task 1 + Task 3 tests).

- [ ] **Step 5: Gate and commit**

```bash
rtk cargo fmt --all
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
rtk git add crates/bin/src/compat.rs
rtk git commit -m "feat(compat): CompatSession with policy gates and version-skew banners"
```

---

### Task 4: Centralized 404 safety net in `ApiClient`

**Files:**
- Modify: `crates/bin/src/cli/api.rs` — add `json_404_error` + `expect_feature`; rewire `send_secrets` (line ~301), `list_secrets` (~312), `list_commands` (~338), `run_command` (~351), `stats` (~145); delete `commands_not_found` (~324) and `extract_secrets_error` (~407); update the router tests that pinned the old messages (`list_commands_404_without_body_prompts_agent_update` ~623, `list_commands_404_with_json_error_uses_message_verbatim` ~633, and the secrets 404 tests in the same module).

**Interfaces:**
- Consumes: `crate::compat::{Feature, feature_unavailable_error}` (Tasks 1, 3).
- Produces: private `async fn expect_feature(resp: reqwest::Response, feature: Feature) -> anyhow::Result<reqwest::Response>` used by the five methods above. `source_check` keeps its exact `Ok(None)`-on-any-404 contract (Silent fallback + race safety, per spec deviation note 2026-07-10) — do NOT change its behavior, only its comment.

- [ ] **Step 1: Update the router tests to the new contract (failing first)**

In the tests module of `crates/bin/src/cli/api.rs`, replace the message assertions of the two commands-404 tests (keep test names and mock routers as they are):

```rust
#[tokio::test]
async fn list_commands_404_without_body_prompts_agent_update() {
    let app = Router::new().route("/v1/projects/demo/commands", get(not_found_plain));
    let api = ApiClient::new(spawn_app(app).await);
    let err = api.list_commands("demo").await.unwrap_err().to_string();
    assert!(err.contains("container commands"), "{err}");
    assert!(err.contains(">= 0.9.0"), "{err}");
    assert!(err.contains("update the agent on the Pi"), "{err}");
}
```

(The JSON-404 test keeps asserting the verbatim domain message — unchanged behavior.) Add the same style of bare-404 test for `stats`:

```rust
#[tokio::test]
async fn stats_bare_404_means_old_agent_and_prompts_update() {
    let app = Router::new(); // no /v1/stats route -> bare 404
    let api = ApiClient::new(spawn_app(app).await);
    let err = api.stats(None).await.unwrap_err().to_string();
    assert!(err.contains("stats"), "{err}");
    assert!(err.contains("update the agent on the Pi"), "{err}");
}
```

And update the secrets 404 tests (same module) to expect the unified wording: `secrets`, `>= 0.9.0`, `update the agent on the Pi` for the bare 404; verbatim message for JSON 404.

- [ ] **Step 2: Run tests to verify the new assertions fail**

Run: `rtk cargo test --locked -p pi api`
Expected: FAIL — old messages ("agent does not support [commands]; update rpi-agent on the Pi", "agent does not support the secrets API...") don't match, and `stats` currently returns the raw `404 Not Found`.

- [ ] **Step 3: Implement**

In `crates/bin/src/cli/api.rs`, add near `extract_error`:

```rust
/// The {"error"} message of a JSON 404 body, or `None` for a bare 404.
/// Every rpi-agent error carries a JSON body; axum's bare 404 (route not
/// registered) does not — that is how an old agent reveals itself.
async fn json_404_error(resp: reqwest::Response) -> Option<String> {
    let bytes = resp.bytes().await.unwrap_or_default();
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string))
}

/// Race safety net (spec 2026-07-12): the agent may have been swapped
/// between the handshake and this call. A bare 404 means the running agent
/// does not serve `feature`; a JSON 404 is a domain not-found and passes
/// through verbatim.
async fn expect_feature(
    resp: reqwest::Response,
    feature: crate::compat::Feature,
) -> anyhow::Result<reqwest::Response> {
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return match json_404_error(resp).await {
            Some(msg) => Err(anyhow::anyhow!("{msg}")),
            None => Err(crate::compat::feature_unavailable_error(feature)),
        };
    }
    extract_error(resp).await
}
```

Rewire the five methods (each keeps its request-building code; only the response handling changes):

```rust
// stats:
Ok(expect_feature(resp, crate::compat::Feature::Stats).await?.json().await?)
// send_secrets / list_secrets:
Ok(expect_feature(resp, crate::compat::Feature::Secrets).await?.json().await?)
// list_commands: replace the `if resp.status() == NOT_FOUND { ... }` block + extract_error with:
Ok(expect_feature(resp, crate::compat::Feature::Commands).await?.json().await?)
// run_command: replace its NOT_FOUND block and `let resp = extract_error(resp).await?;` with:
let resp = expect_feature(resp, crate::compat::Feature::Commands).await?;
```

Delete `commands_not_found` and `extract_secrets_error`. In `source_check`, replace the old doc comment's last sentence with a pointer to the spec: the blanket `Ok(None)`-on-404 stays as the Silent policy's transport-level fallback (spec 2026-07-12).

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test --locked -p pi`
Expected: PASS — including `source_check_404_means_old_agent_and_returns_none` untouched.

- [ ] **Step 5: Gate and commit**

```bash
rtk cargo fmt --all
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
rtk git add crates/bin/src/cli/api.rs
rtk git commit -m "refactor(cli): one expect_feature safety net replaces per-route 404 sniffing"
```

---

### Task 5: `connect_agent` helper + call-site migration

**Files:**
- Create: `crates/bin/src/cli/connect.rs`
- Modify: `crates/bin/src/cli/mod.rs` (add `pub mod connect;`)
- Modify: `crates/bin/src/cli/commands.rs` — migrate `deploy` (~15), `deploy_cancel` (~117), `secrets_send` (~155), `gc` (~256), `secrets_ls` (~270), `ls` (~318), `logs` (~362), `stats` (~378), `lifecycle` (~420), `command` (~437), `rm` (~490), `status` (~532); delete `version_mismatch_warning` (~775) and its test (~951). `doctor`, `agent_status`, `agent_logs` keep their bespoke raw-`ApiClient` flows (diagnostics / fallback-to-ssh paths) — do not touch them.
- Modify: `crates/bin/src/compat.rs` — remove the `#![allow(dead_code)]` escape hatch if Task 1 added it.

**Interfaces:**
- Consumes: `ApiClient::version()` (existing), `CompatSession` (Task 3), `ConnectOpts::resolve()`, `SshTunnel::open` (existing).
- Produces: `cli::connect::AgentConn { tunnel: SshTunnel, api: ApiClient, compat: CompatSession }` and `async fn connect_agent(connect: ConnectOpts) -> anyhow::Result<AgentConn>`.

- [ ] **Step 1: Write `connect.rs`**

```rust
use crate::cli::api::ApiClient;
use crate::cli::config::ConnectOpts;
use crate::cli::tunnel::SshTunnel;
use crate::compat::CompatSession;

/// One live agent connection. Keep `tunnel` alive for as long as `api` is
/// used — dropping it closes the SSH forward.
pub struct AgentConn {
    pub tunnel: SshTunnel,
    pub api: ApiClient,
    pub compat: CompatSession,
}

/// The single entry point every agent-talking command goes through:
/// tunnel, client, `/v1/version` handshake, version-skew banners.
pub async fn connect_agent(connect: ConnectOpts) -> anyhow::Result<AgentConn> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());
    let info = api.version().await?;
    let compat = CompatSession::new(env!("CARGO_PKG_VERSION"), &info);
    compat.emit_version_banners();
    Ok(AgentConn { tunnel, api, compat })
}
```

Add `pub mod connect;` to `crates/bin/src/cli/mod.rs` (alphabetical: between `config` and `init`).

- [ ] **Step 2: Migrate call sites**

Uniform replacement — the three lines

```rust
let profile = connect.resolve()?;
let tunnel = SshTunnel::open(&profile).await?;
let api = ApiClient::new(tunnel.base_url.clone());
```

become

```rust
let AgentConn { tunnel, api, compat } = crate::cli::connect::connect_agent(connect).await?;
```

with `use crate::cli::connect::AgentConn;` added to the imports of `commands.rs`.

Destructuring rule — the tunnel is a keep-alive guard: bind it as `tunnel: _tunnel` (a named binding lives to end of scope), NEVER as `tunnel: _` (a bare `_` drops the guard immediately and kills the SSH forward mid-command):

- Gating sites (`deploy`, `secrets_send`, `secrets_ls`, `stats`): `let AgentConn { tunnel: _tunnel, api, compat } = ...`
- Non-gating sites (`deploy_cancel`, `gc`, `ls`, `logs`, `lifecycle`, `rm`, `status`): `let AgentConn { tunnel: _tunnel, api, compat: _compat } = ...`
- `command` (gating + explicit drop before `process::exit`): `let AgentConn { tunnel, api, compat } = ...`; the existing `drop(tunnel);` line stays.
- `stats` watch branch: delete the old `let _tunnel = tunnel;` line — the `_tunnel` binding from the destructure already keeps the tunnel alive through the watch loop.

Per-site gating additions:

`deploy` — replace the version/warning block (lines 28–32) and wrap the preflight:

```rust
let AgentConn { tunnel: _tunnel, api, compat } = crate::cli::connect::connect_agent(connect).await?;
output::status(format!(
    "agent {} (api {})",
    compat.agent_version(),
    compat.agent_api()
));

if compat.gate(crate::compat::Feature::SourceCheck)? {
    crate::cli::sourcekey::preflight(
        &crate::cli::sourcekey::GhCli,
        &api,
        &rpitoml.project.name,
        &project.repo,
        no_gh_key,
    )
    .await?;
}
```

(Note the banner now prints before the `agent ... (api ...)` status line — accepted ordering change, spec section "CLI side".)

`secrets_send` and `secrets_ls` — after connecting:

```rust
compat.gate(crate::compat::Feature::Secrets)?;
```

`command` — after connecting:

```rust
compat.gate(crate::compat::Feature::Commands)?;
```

`stats` — after connecting (before the watch branch):

```rust
compat.gate(crate::compat::Feature::Stats)?;
```

Delete `version_mismatch_warning` (~775) and its test `version_mismatch_produces_warning_only_on_difference` (~951) — the behavior now lives in `CompatSession::emit_version_banners` with its own tests. Remove now-unused imports (`SshTunnel`, `ApiClient` may still be needed by remaining raw sites — let the compiler decide; `doctor`/`agent_status`/`agent_logs` still use them).

- [ ] **Step 3: Run the full suite**

Run: `rtk cargo test --locked`
Expected: PASS. Also `rtk cargo clippy --all-targets --locked -- -D warnings` — this is where any leftover `dead_code` allowance from Task 1 must come off.

- [ ] **Step 4: Manual smoke (optional but cheap)**

If a dev agent is running locally (`rpi agent run` per rpi-cli skill), `cargo run -p pi -- status` against it shows no banner (same version). Skip if no local agent is set up — e2e in Task 6 covers the real cross-version path.

- [ ] **Step 5: Gate and commit**

```bash
rtk cargo fmt --all
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
rtk git add crates/bin/src/cli/connect.rs crates/bin/src/cli/mod.rs crates/bin/src/cli/commands.rs crates/bin/src/compat.rs
rtk git commit -m "feat(cli): connect_agent handshake + compat gates at every call site"
```

---

### Task 6: Cross-version e2e — legacy agent from `v0.17.1`

**Files:**
- Create: `tests/e2e/prepare-legacy.mjs`
- Create: `tests/e2e/prepare-legacy.test.mjs`
- Create: `tests/e2e/scenarios/compat-legacy-agent/scenario.sh`
- Create: `tests/e2e/scenarios/compat-legacy-agent/agent-bin`
- Modify: `tests/e2e/run.mjs` (generate the tarball before `compose build client`, ~line 442)
- Modify: `tests/e2e/Dockerfile` (add `legacy_builder` stage + runtime COPY)
- Modify: `tests/e2e/entrypoints/target-entrypoint.sh` (per-scenario agent binary override)
- Modify: `.gitignore` (ignore the generated tarball)
- Modify: `.github/workflows/ci.yml` (fetch the tag + generate the tarball before the image build)

**Interfaces:**
- Consumes: the compat behavior from Tasks 1–5 (banner text `update the agent on the Pi`, deploy status line `agent 0.17.1 (api v1)`, stats degradation warning `no host history from the agent`).
- Produces: `LEGACY_TAG = 'v0.17.1'`, `prepareLegacyTar()` (exported from `prepare-legacy.mjs`), `/usr/local/bin/rpi-legacy` in the runtime image, `agent-bin` per-scenario override contract (file contains an absolute binary path).

- [ ] **Step 1: `prepare-legacy.mjs` + node test (test first)**

`tests/e2e/prepare-legacy.test.mjs`:

```js
import assert from 'node:assert/strict';
import { test } from 'node:test';
import path from 'node:path';
import { tmpdir } from 'node:os';
import { LEGACY_TAG, LEGACY_TAR, prepareLegacyTar } from './prepare-legacy.mjs';

test('legacy tag is a pinned release tag', () => {
  assert.match(LEGACY_TAG, /^v\d+\.\d+\.\d+$/);
});

test('tarball filename contract is stable', () => {
  assert.ok(LEGACY_TAR.endsWith('.legacy-src.tar'));
});

test('rejects with a fetch hint when the tag is missing', async () => {
  await assert.rejects(
    prepareLegacyTar({ tag: 'v999.999.999', out: path.join(tmpdir(), 'rpi-e2e-no.tar') }),
    /git archive v999\.999\.999 failed/,
  );
});
```

(The first assertion of the second test is Windows-path noise-prone — simplify to just the `endsWith` check if it fights back; the value is pinning the filename contract.)

`tests/e2e/prepare-legacy.mjs`:

```js
import { spawn } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, '..', '..');

/** Pinned legacy agent for cross-version compat e2e (spec 2026-07-12):
 * the newest tag that predates the `features` handshake field, lacks
 * source-check (< 0.18.0), and still runs under this harness. */
export const LEGACY_TAG = 'v0.17.1';
export const LEGACY_TAR = path.join(HERE, '.legacy-src.tar');

/** `git archive` the pinned tag into the build context. Deterministic for a
 * given tag, so the docker layer that ADDs it stays cached. */
export function prepareLegacyTar({ tag = LEGACY_TAG, out = LEGACY_TAR, cwd = ROOT } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn('git', ['archive', '--format=tar', '-o', out, tag], {
      cwd,
      stdio: ['ignore', 'inherit', 'pipe'],
      windowsHide: true,
    });
    let stderr = '';
    child.stderr.on('data', (chunk) => { stderr += chunk; });
    child.on('error', reject);
    child.on('close', (code) => {
      if (code === 0) {
        resolve(out);
      } else {
        reject(new Error(
          `git archive ${tag} failed (exit ${code}): ${stderr.trim()} ` +
          `— shallow clone? fetch the tag: git fetch --no-tags origin +refs/tags/${tag}:refs/tags/${tag}`,
        ));
      }
    });
  });
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  prepareLegacyTar().then(
    (out) => console.log(`rpi e2e: legacy source ready: ${out} (${LEGACY_TAG})`),
    (error) => {
      console.error(`rpi e2e: ${error.message}`);
      process.exit(1);
    },
  );
}
```

Run: `npm run test:node`
Expected: the three new tests PASS (and existing `run.test.mjs`/`contracts.test.mjs` stay green — if `contracts.test.mjs` pins the Dockerfile or entrypoint content, update those fixtures as part of the later steps, not by weakening the contract).

- [ ] **Step 2: Wire generation into `run.mjs`**

In `tests/e2e/run.mjs`: `import { prepareLegacyTar } from './prepare-legacy.mjs';` and inside `runE2E`, in the `if (!prebuilt) { ... }` block (line ~442), BEFORE the `compose build client` call:

```js
await prepareLegacyTar();
```

(Failure rejects with the fetch hint and fails the run — same contract as a broken image build.)

- [ ] **Step 3: Dockerfile stage + runtime binary**

In `tests/e2e/Dockerfile`, after the existing `builder` stage:

```dockerfile
FROM rust:1.88-bookworm AS legacy_builder
WORKDIR /src
# Generated by tests/e2e/prepare-legacy.mjs (git archive of LEGACY_TAG);
# ADD auto-extracts the tar into /src.
ADD tests/e2e/.legacy-src.tar /src/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,id=legacy-target,target=/src/target \
    cargo build --locked -p pi && \
    install -D -m 0755 target/debug/rpi /out/rpi-legacy
```

In the `runtime` stage, next to the existing `COPY --from=builder`:

```dockerfile
COPY --from=legacy_builder /out/rpi-legacy /usr/local/bin/rpi-legacy
```

and extend the sanity `RUN` line's tail: `... && rpi --version && rpi-legacy --version`.

Add to `.gitignore`:

```
/tests/e2e/.legacy-src.tar
```

- [ ] **Step 4: Per-scenario agent binary override**

In `tests/e2e/entrypoints/target-entrypoint.sh`, after the `AGENT_CONFIG` resolution (lines 4–6):

```bash
AGENT_BIN=/usr/local/bin/rpi
AGENT_BIN_OVERRIDE=/opt/e2e/scenarios/$SCENARIO/agent-bin
if [[ -f $AGENT_BIN_OVERRIDE ]]; then
  AGENT_BIN=$(tr -d '[:space:]' <"$AGENT_BIN_OVERRIDE")
fi
```

and change the launch line to use it:

```bash
  "$AGENT_BIN" agent run --config "$AGENT_CONFIG" &
```

(keeping the surrounding `runuser -u rpi-agent -- env ...` wrapper exactly as is).

- [ ] **Step 5: The scenario**

`tests/e2e/scenarios/compat-legacy-agent/agent-bin` (one line, no trailing spaces):

```
/usr/local/bin/rpi-legacy
```

`tests/e2e/scenarios/compat-legacy-agent/scenario.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

source /opt/e2e/lib.sh
e2e_bootstrap

rpi --version

# Current CLI against a v0.17.1 agent: the handshake has no `features`
# field, so capabilities come from the frozen legacy matrix. source-check
# is absent on 0.17.1 and Silent-gated — the deploy must sail through.
run_capture deploy.log rpi deploy "${CONNECT[@]}"
assert_deploy_log deploy.log
assert_log deploy.log 'agent 0.17.1 (api v1)'
# Version-skew banner: CLI is newer than the legacy agent.
assert_log deploy.log 'update the agent on the Pi'

# secrets is matrix-inferred as available on 0.17.1: the gate passes and the
# legacy route answers.
run_capture secrets-ls.log rpi secrets ls "${CONNECT[@]}"
assert_log secrets-ls.log 'no secrets stored'

# stats gate passes the same way; the legacy agent has no host_history, so
# the CLI's additive-field degradation warning must appear.
run_capture stats.log rpi stats e2e-fixture "${CONNECT[@]}"
assert_log stats.log 'e2e-fixture'
assert_log stats.log 'no host history from the agent'

echo 'rpi e2e: PASS'
```

- [ ] **Step 6: CI — fetch the tag and generate the tarball before the image build**

In `.github/workflows/ci.yml`, e2e job, insert between `actions/setup-node` and `docker/setup-buildx-action`:

```yaml
      - name: Prepare legacy agent source (cross-version e2e)
        run: |
          git fetch --no-tags --depth=1 origin +refs/tags/v0.17.1:refs/tags/v0.17.1
          node tests/e2e/prepare-legacy.mjs
```

- [ ] **Step 7: Run the scenario locally**

Requires Docker Desktop running.

```bash
node tests/e2e/run.mjs compat-legacy-agent
```

Expected: `rpi e2e: 1/1 scenarios passed`. On failure, artifacts land under `target/e2e-artifacts/<project>/compat-legacy-agent/` — read `scenario.log` and `nested.log`. Known risks and their fixes:
- `git archive` fails → tag missing locally (`git fetch --tags`).
- v0.17.1 build fails in `legacy_builder` → check `build.log`; the tag builds with `cargo build --locked -p pi` on rust:1.88 (same toolchain family as the era's CI).
- Banner text mismatch → the assertion strings above must match Task 3's exact wording.

Then run the neighbors to prove no regression in the shared entrypoint:

```bash
node tests/e2e/run.mjs happy-path stats
```

Expected: `2/2 scenarios passed`.

- [ ] **Step 8: Gate and commit**

```bash
npm run test:node
rtk git add tests/e2e/prepare-legacy.mjs tests/e2e/prepare-legacy.test.mjs tests/e2e/run.mjs tests/e2e/Dockerfile tests/e2e/entrypoints/target-entrypoint.sh tests/e2e/scenarios/compat-legacy-agent .gitignore .github/workflows/ci.yml
rtk git commit -m "test(e2e): cross-version scenario runs current CLI against a v0.17.1 agent"
```

---

### Task 7: Full verification sweep

**Files:** none new — verification only, plus any fixes it forces.

- [ ] **Step 1: The full local gate**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
npm run test:node
```

Expected: all green. Fix anything red before proceeding (and commit fixes with a `fix:` prefix).

- [ ] **Step 2: Full e2e suite**

```bash
npm run test:e2e
```

Expected: all scenarios pass, including the three pre-existing ones (their target-entrypoint path changed) and `compat-legacy-agent`.

- [ ] **Step 3: Behavior spot-check against the spec**

Walk the spec's "Initial feature registry" table and confirm each row's runtime behavior exists in code (grep is enough): `gate(Feature::Secrets)` in `secrets_send`/`secrets_ls`, `gate(Feature::Commands)` in `command`, `gate(Feature::Stats)` in `stats`, `gate(Feature::SourceCheck)` wrapping the deploy preflight, `expect_feature` on the five ApiClient methods, `emit_version_banners` in `connect_agent`.

- [ ] **Step 4: Final commit if anything moved**

```bash
rtk git status
rtk git add -A && rtk git commit -m "chore(compat): verification sweep fixes"
```

(Skip the commit if the tree is clean.)
