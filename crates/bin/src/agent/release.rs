//! Release-artifact math shared by `rpi agent update` (board-side) and
//! `rpi upgrade` (client-side). A Rust port of the download/verify recipe in
//! `scripts/postinstall.js` (`TARGET_TRIPLES`, `assetName`, `parseSha256Sums`).
//! Pure helpers live here; the `Sys`-driven download orchestration is added in
//! the same file (see `download_verified_binary`).

use crate::agent::setup::Sys;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

/// Create a fresh temp working directory via `mktemp -d`.
pub async fn make_tempdir(sys: &dyn Sys) -> Result<String, String> {
    sys.run("mktemp", &["-d"])
        .await
        .map(|s| s.trim().to_string())
        .map_err(|e| format!("mktemp -d: {e}"))
}

/// Resolve the newest published release version (no leading `v`) via the GitHub
/// API. Shells `curl` through `Sys` and parses `tag_name` — no async HTTP
/// client needed on the board. `api_base` is passed in (read from env by the
/// caller) so this stays env-free and unit-testable.
pub async fn resolve_latest_version(sys: &dyn Sys, api_base: &str) -> Result<String, String> {
    let url = format!("{api_base}/releases/latest");
    let body = sys
        .run(
            "curl",
            &["-fsSL", "-H", "Accept: application/vnd.github+json", &url],
        )
        .await
        .map_err(|e| format!("query {url}: {e}"))?;
    parse_latest_tag(&body)
}

/// Download the release archive for `version` targeting this host's arch, verify
/// its SHA256 against the release `SHA256SUMS`, extract it into `workdir`, and
/// return the path to the extracted `rpi` binary. All I/O goes through `Sys`
/// (curl/sha256sum/tar), mirroring `setup::ensure_cloudflared_binary`.
/// `base_url` is passed in (read from env by the caller) so this stays env-free.
pub async fn download_verified_binary(
    sys: &dyn Sys,
    base_url: &str,
    version: &str,
    workdir: &str,
) -> Result<PathBuf, String> {
    let arch = sys
        .run("uname", &["-m"])
        .await
        .map_err(|e| format!("uname -m: {e}"))?;
    let triple =
        target_triple(&arch).ok_or_else(|| format!("unsupported architecture: {}", arch.trim()))?;
    let asset = asset_name(version, triple);
    let archive = format!("{workdir}/{asset}");
    let sums = format!("{workdir}/SHA256SUMS");
    let asset_url = format!("{base_url}/v{version}/{asset}");
    let sums_url = format!("{base_url}/v{version}/SHA256SUMS");

    sys.run("curl", &["-fsSL", "-o", &archive, &asset_url])
        .await
        .map_err(|e| format!("download {asset_url}: {e}"))?;
    sys.run("curl", &["-fsSL", "-o", &sums, &sums_url])
        .await
        .map_err(|e| format!("download {sums_url}: {e}"))?;

    let sums_text = sys
        .read(Path::new(&sums))
        .ok_or_else(|| format!("cannot read {sums}"))?;
    let expected = parse_sha256sums(&sums_text)
        .get(&asset)
        .cloned()
        .ok_or_else(|| format!("{asset} not listed in SHA256SUMS"))?;
    let actual_line = sys
        .run("sha256sum", &[&archive])
        .await
        .map_err(|e| format!("sha256sum {archive}: {e}"))?;
    let actual = actual_line.split_whitespace().next().unwrap_or("");
    if actual != expected {
        return Err(format!(
            "sha256 mismatch for {asset}: expected {expected}, got {actual}"
        ));
    }

    sys.run("tar", &["-xf", &archive, "-C", workdir])
        .await
        .map_err(|e| format!("tar extract {archive}: {e}"))?;
    let bin = PathBuf::from(format!("{workdir}/rpi"));
    if !sys.exists(&bin) {
        return Err(format!("archive {asset} did not contain rpi"));
    }
    Ok(bin)
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

    use crate::agent::setup::fake::FakeSys;
    use std::path::Path;

    const API: &str = "https://api.github.com/repos/khmilevoi/rpi-deploy";
    const BASE: &str = "file:///rel";

    #[tokio::test]
    async fn resolve_latest_version_reads_tag_name() {
        let mut sys = FakeSys::default();
        let url = format!("{API}/releases/latest");
        sys.ok.insert(
            FakeSys::key(
                "curl",
                &["-fsSL", "-H", "Accept: application/vnd.github+json", &url],
            ),
            r#"{"tag_name":"v0.22.0"}"#.into(),
        );
        assert_eq!(resolve_latest_version(&sys, API).await.unwrap(), "0.22.0");
    }

    #[tokio::test]
    async fn download_verified_binary_happy_path() {
        let version = "0.22.0";
        let triple = "aarch64-unknown-linux-musl";
        let asset = asset_name(version, triple);
        let hash = "c".repeat(64);
        let work = "/tmp/wd";
        let archive = format!("{work}/{asset}");
        let sums = format!("{work}/SHA256SUMS");

        let mut sys = FakeSys::default();
        sys.ok
            .insert(FakeSys::key("uname", &["-m"]), "aarch64".into());
        sys.ok.insert(
            FakeSys::key(
                "curl",
                &[
                    "-fsSL",
                    "-o",
                    &archive,
                    &format!("{BASE}/v{version}/{asset}"),
                ],
            ),
            String::new(),
        );
        sys.ok.insert(
            FakeSys::key(
                "curl",
                &[
                    "-fsSL",
                    "-o",
                    &sums,
                    &format!("{BASE}/v{version}/SHA256SUMS"),
                ],
            ),
            String::new(),
        );
        sys.files.insert(sums.clone(), format!("{hash}  {asset}\n"));
        sys.ok.insert(
            FakeSys::key("sha256sum", &[&archive]),
            format!("{hash}  {archive}"),
        );
        sys.ok.insert(
            FakeSys::key("tar", &["-xf", &archive, "-C", work]),
            String::new(),
        );
        sys.paths.insert(format!("{work}/rpi"));

        let bin = download_verified_binary(&sys, BASE, version, work)
            .await
            .unwrap();
        assert_eq!(bin, Path::new("/tmp/wd/rpi"));
    }

    #[tokio::test]
    async fn download_verified_binary_rejects_sha_mismatch() {
        let version = "0.22.0";
        let asset = asset_name(version, "aarch64-unknown-linux-musl");
        let work = "/tmp/wd";
        let archive = format!("{work}/{asset}");
        let sums = format!("{work}/SHA256SUMS");
        let mut sys = FakeSys::default();
        sys.ok
            .insert(FakeSys::key("uname", &["-m"]), "aarch64".into());
        sys.ok.insert(
            FakeSys::key(
                "curl",
                &[
                    "-fsSL",
                    "-o",
                    &archive,
                    &format!("{BASE}/v{version}/{asset}"),
                ],
            ),
            String::new(),
        );
        sys.ok.insert(
            FakeSys::key(
                "curl",
                &[
                    "-fsSL",
                    "-o",
                    &sums,
                    &format!("{BASE}/v{version}/SHA256SUMS"),
                ],
            ),
            String::new(),
        );
        sys.files
            .insert(sums.clone(), format!("{}  {asset}\n", "a".repeat(64)));
        sys.ok.insert(
            FakeSys::key("sha256sum", &[&archive]),
            format!("{}  {archive}", "b".repeat(64)),
        );
        let err = download_verified_binary(&sys, BASE, version, work)
            .await
            .unwrap_err();
        assert!(err.contains("sha256 mismatch"), "{err}");
    }

    #[tokio::test]
    async fn download_verified_binary_rejects_unsupported_arch() {
        let mut sys = FakeSys::default();
        sys.ok
            .insert(FakeSys::key("uname", &["-m"]), "armv7l".into());
        let err = download_verified_binary(&sys, BASE, "0.22.0", "/tmp/wd")
            .await
            .unwrap_err();
        assert!(err.contains("unsupported architecture"), "{err}");
    }

    #[tokio::test]
    async fn download_verified_binary_errors_when_asset_not_in_sums() {
        let version = "0.22.0";
        let asset = asset_name(version, "aarch64-unknown-linux-musl");
        let work = "/tmp/wd";
        let archive = format!("{work}/{asset}");
        let sums = format!("{work}/SHA256SUMS");
        let mut sys = FakeSys::default();
        sys.ok
            .insert(FakeSys::key("uname", &["-m"]), "aarch64".into());
        sys.ok.insert(
            FakeSys::key(
                "curl",
                &[
                    "-fsSL",
                    "-o",
                    &archive,
                    &format!("{BASE}/v{version}/{asset}"),
                ],
            ),
            String::new(),
        );
        sys.ok.insert(
            FakeSys::key(
                "curl",
                &[
                    "-fsSL",
                    "-o",
                    &sums,
                    &format!("{BASE}/v{version}/SHA256SUMS"),
                ],
            ),
            String::new(),
        );
        sys.files.insert(
            sums.clone(),
            format!("{}  some-other-file.tar.gz\n", "a".repeat(64)),
        );
        let err = download_verified_binary(&sys, BASE, version, work)
            .await
            .unwrap_err();
        assert!(err.contains("not listed in SHA256SUMS"), "{err}");
    }
}
