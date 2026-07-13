//! Release-artifact math shared by `rpi agent update` (board-side) and
//! `rpi upgrade` (client-side). A Rust port of the download/verify recipe in
//! `scripts/postinstall.js` (`TARGET_TRIPLES`, `assetName`, `parseSha256Sums`).
//! Pure helpers live here; the `Sys`-driven download orchestration is added in
//! the same file (see `download_verified_binary`).

#![allow(dead_code)]

use std::collections::HashMap;

/// GitHub `owner/repo` that publishes rpi releases. Mirrors
/// `scripts/postinstall.js`'s `REPO`.
pub const REPO: &str = "khmilevoi/rpi-deploy";

/// Release-download base URL. `RPI_RELEASE_BASE_URL` overrides it (required for
/// offline tests); otherwise the canonical GitHub Releases download root.
pub fn release_base_url() -> String {
    std::env::var("RPI_RELEASE_BASE_URL")
        .unwrap_or_else(|_| format!("https://github.com/{REPO}/releases/download"))
}

/// GitHub REST API base for the repo. `RPI_RELEASE_API_URL` overrides it.
pub fn api_base_url() -> String {
    std::env::var("RPI_RELEASE_API_URL")
        .unwrap_or_else(|_| format!("https://api.github.com/repos/{REPO}"))
}

/// Map `uname -m` to the Rust target triple whose prebuilt archive the release
/// publishes. Mirrors `postinstall.js`'s `TARGET_TRIPLES` (Linux entries only —
/// the agent only ever runs on Linux).
pub fn target_triple(uname_m: &str) -> Option<&'static str> {
    match uname_m.trim() {
        "aarch64" | "arm64" => Some("aarch64-unknown-linux-musl"),
        "x86_64" | "amd64" => Some("x86_64-unknown-linux-musl"),
        _ => None,
    }
}

/// Release asset file name for a version + triple. Linux is always `tar.gz`.
pub fn asset_name(version: &str, triple: &str) -> String {
    format!("rpi-v{version}-{triple}.tar.gz")
}

/// Parse `sha256sum` output — `"<hash>  <name>"` (text) or `"<hash> *<name>"`
/// (binary) — into `name -> hash`. Accepts only lowercase 64-hex hashes, like
/// `postinstall.js`'s `/^([0-9a-f]{64})[ *]+(.+)$/`.
pub fn parse_sha256sums(text: &str) -> HashMap<String, String> {
    let mut sums = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.len() < 66 {
            continue; // 64-hex hash + >=1 separator + >=1 name char
        }
        let (hash, rest) = line.split_at(64);
        let hash_ok = hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hash_ok {
            continue;
        }
        let name = rest.trim_start_matches([' ', '*']).trim();
        if !name.is_empty() {
            sums.insert(name.to_string(), hash.to_string());
        }
    }
    sums
}

/// Extract and normalize the `tag_name` (strip a leading `v`) from a GitHub
/// `releases/latest` JSON body.
pub fn parse_latest_tag(body: &str) -> Result<String, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("parse releases/latest json: {e}"))?;
    let tag = v
        .get("tag_name")
        .and_then(|t| t.as_str())
        .ok_or("releases/latest response has no tag_name")?;
    Ok(tag.trim_start_matches('v').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_triple_maps_supported_arches() {
        assert_eq!(target_triple("aarch64"), Some("aarch64-unknown-linux-musl"));
        assert_eq!(target_triple("arm64"), Some("aarch64-unknown-linux-musl"));
        assert_eq!(target_triple("x86_64"), Some("x86_64-unknown-linux-musl"));
        assert_eq!(target_triple("amd64"), Some("x86_64-unknown-linux-musl"));
        // tolerates trailing newline from `uname -m`
        assert_eq!(
            target_triple("aarch64\n"),
            Some("aarch64-unknown-linux-musl")
        );
    }

    #[test]
    fn target_triple_rejects_unsupported() {
        assert_eq!(target_triple("armv7l"), None);
        assert_eq!(target_triple("riscv64"), None);
    }

    #[test]
    fn asset_name_is_targz_on_linux() {
        assert_eq!(
            asset_name("0.22.0", "aarch64-unknown-linux-musl"),
            "rpi-v0.22.0-aarch64-unknown-linux-musl.tar.gz"
        );
    }

    #[test]
    fn parse_sha256sums_reads_text_and_binary_lines() {
        let h1 = "a".repeat(64);
        let h2 = "b".repeat(64);
        let text = format!(
            "{h1}  rpi-v0.22.0-x86_64-unknown-linux-musl.tar.gz\n\
             {h2} *rpi-v0.22.0-aarch64-unknown-linux-musl.tar.gz\n\
             \nnot a sums line\n"
        );
        let sums = parse_sha256sums(&text);
        assert_eq!(sums["rpi-v0.22.0-x86_64-unknown-linux-musl.tar.gz"], h1);
        assert_eq!(sums["rpi-v0.22.0-aarch64-unknown-linux-musl.tar.gz"], h2);
        assert_eq!(sums.len(), 2);
    }

    #[test]
    fn parse_sha256sums_ignores_uppercase_and_short_hashes() {
        let text = "ABCDEF  x.tar.gz\ndeadbeef  y.tar.gz\n";
        assert!(parse_sha256sums(text).is_empty());
    }

    #[test]
    fn parse_latest_tag_strips_leading_v() {
        assert_eq!(
            parse_latest_tag(r#"{"tag_name":"v0.22.0"}"#).unwrap(),
            "0.22.0"
        );
        assert_eq!(
            parse_latest_tag(r#"{"tag_name":"0.22.0"}"#).unwrap(),
            "0.22.0"
        );
    }

    #[test]
    fn parse_latest_tag_errors_without_tag_name() {
        assert!(parse_latest_tag(r#"{"name":"x"}"#).is_err());
        assert!(parse_latest_tag("not json").is_err());
    }
}
