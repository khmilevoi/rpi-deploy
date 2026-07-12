//! Unified CLI <-> agent compatibility (spec 2026-07-12).
//!
//! The single source of truth for which semantic features exist, which
//! capability string each one advertises in the `/v1/version` handshake,
//! and what happens when the other side lacks one.

#![allow(dead_code)] // TODO(task-5): remove

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
        Feature::ALL
            .iter()
            .map(|f| f.capability().to_string())
            .collect()
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
            assert!(
                adv.contains(&f.capability().to_string()),
                "{} missing",
                f.capability()
            );
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
        assert!(
            !caps.contains("source-check"),
            "source-check ships in 0.18.0"
        );

        let caps = legacy_capabilities("0.18.0");
        assert!(caps.contains("source-check"));

        assert!(legacy_capabilities("unknown").is_empty());
    }
}
