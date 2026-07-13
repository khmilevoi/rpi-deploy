# Client-triggered Agent Update Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `rpi upgrade` (client) and `rpi agent update` (board) so an operator can update a board's rpi binary to a chosen version over the existing SSH + sudo + GitHub-Releases trust boundary, plus a no-npm `install.sh` bootstrap installer.

**Architecture:** The board-side `rpi agent update` obtains a fresh binary (npm channel, or GitHub-direct download verified against `SHA256SUMS`), swaps `/usr/local/bin/rpi` via the existing `self_install::ensure_installed`, re-runs the idempotent `setup()`, and restarts the unit — all through the existing `Sys` trait (curl/sha256sum/tar), so it is unit-testable with `FakeSys`. The client-side `rpi upgrade` resolves a target version, shows `current → target`, and triggers the board with `ssh -t <user>@<host> sudo rpi agent update --version <X>`. `install.sh` shares the same download-and-verify recipe in POSIX shell.

**Tech Stack:** Rust (async, `anyhow`, `reqwest`, `serde_json`, the repo's `Sys`/`FakeSys` test abstraction), POSIX shell, Node's built-in `node --test`, the Docker-in-Docker e2e harness.

## Global Constraints

- **Repo / owner:** `khmilevoi/rpi-deploy` (matches `scripts/postinstall.js`'s `REPO`). Copy this constant verbatim; do not invent a different owner.
- **Arch → target triple map (Linux only):** `aarch64` | `arm64` → `aarch64-unknown-linux-musl`; `x86_64` | `amd64` → `x86_64-unknown-linux-musl`; anything else is unsupported (hard error). Mirrors `postinstall.js`'s `TARGET_TRIPLES`.
- **Asset name:** `rpi-v<version>-<triple>.tar.gz` (Linux is always `tar.gz`).
- **Release download base URL:** default `https://github.com/khmilevoi/rpi-deploy/releases/download`, overridable via env `RPI_RELEASE_BASE_URL`. Download URL = `<base>/v<version>/<asset>`; sums URL = `<base>/v<version>/SHA256SUMS`. The env override is REQUIRED for testability — never hardcode the base.
- **GitHub API base:** default `https://api.github.com/repos/khmilevoi/rpi-deploy`, overridable via env `RPI_RELEASE_API_URL`. Latest = `<api>/releases/latest`, read `tag_name`, strip a leading `v`.
- **No new heavy crates.** Do the download/verify/extract by shelling `curl` / `sha256sum` / `tar` / `mktemp` through the existing `Sys` trait (exactly like `agent::setup::ensure_cloudflared_binary`). Parse JSON with the already-present `serde_json`. The client's `latest` lookup uses the already-present `reqwest`.
- **No new agent privilege.** The agent process never touches its own binary. The privileged swap happens only under `sudo` on the board, over an authenticated SSH session.
- **Version bump is `/release`'s job.** Do NOT hand-edit `package.json` / `Cargo.toml` versions in these tasks. (This feature is a new minor: 0.21.1 → 0.22.0.)
- **CLI-command wiring:** adding `rpi upgrade` and `rpi agent update` — load and follow the `add-cli-command` skill when touching `crates/bin/src/main.rs`, dispatch, and command modules.
- **Before considering ANY task complete**, run (per `CLAUDE.md`):
  ```bash
  rtk cargo fmt --all -- --check
  rtk cargo clippy --all-targets --locked -- -D warnings
  rtk cargo test --locked
  ```
  If `fmt --check` reports a diff, run `rtk cargo fmt --all` and commit the result — do not hand-edit formatting. Node-side tasks additionally run `npm run test:node`.
- **Language:** all artifacts (code, comments, docs, commit messages) in English; chat replies to the user in Russian.

---

### Task 1: Release asset math (pure helpers)

Pure, side-effect-free functions ported from `scripts/postinstall.js`. No I/O — everything here is unit-testable without `Sys`.

**Files:**
- Create: `crates/bin/src/agent/release.rs`
- Modify: `crates/bin/src/agent/mod.rs` (register the module)

**Interfaces:**
- Produces (used by Tasks 2, 3, 6):
  - `pub const REPO: &str = "khmilevoi/rpi-deploy";`
  - `pub fn release_base_url() -> String`
  - `pub fn api_base_url() -> String`
  - `pub fn target_triple(uname_m: &str) -> Option<&'static str>`
  - `pub fn asset_name(version: &str, triple: &str) -> String`
  - `pub fn parse_sha256sums(text: &str) -> std::collections::HashMap<String, String>`
  - `pub fn parse_latest_tag(body: &str) -> Result<String, String>`

- [ ] **Step 1: Register the module**

In `crates/bin/src/agent/mod.rs`, add the line so it sorts with the others:

```rust
pub mod release;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/bin/src/agent/release.rs` with only the test module first:

```rust
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
        assert_eq!(target_triple("aarch64\n"), Some("aarch64-unknown-linux-musl"));
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
        assert_eq!(parse_latest_tag(r#"{"tag_name":"v0.22.0"}"#).unwrap(), "0.22.0");
        assert_eq!(parse_latest_tag(r#"{"tag_name":"0.22.0"}"#).unwrap(), "0.22.0");
    }

    #[test]
    fn parse_latest_tag_errors_without_tag_name() {
        assert!(parse_latest_tag(r#"{"name":"x"}"#).is_err());
        assert!(parse_latest_tag("not json").is_err());
    }
}
```

> **Why no env-mutation test here:** `release_base_url()` / `api_base_url()` read
> process-global env. Rust runs tests in parallel threads, and mutating env
> (`set_var`/`remove_var`) races with any concurrent `var()` — flaky at best,
> UB at worst. So the base URLs are read from env exactly once at the top of
> `run_cmd` (Task 3) and passed as parameters into the `Sys`-driven functions;
> those functions are then tested with explicit base strings and never touch
> env. The two trivial one-line getters are left untested here and exercised
> via the e2e (Task 8, which sets `RPI_RELEASE_BASE_URL`).

- [ ] **Step 3: Run the tests to verify they fail**

Run: `rtk cargo test --locked -p pi release::`
Expected: FAIL to compile ("cannot find function `target_triple`").

- [ ] **Step 4: Write the implementation**

Prepend to `crates/bin/src/agent/release.rs` (above the test module):

```rust
//! Release-artifact math shared by `rpi agent update` (board-side) and
//! `rpi upgrade` (client-side). A Rust port of the download/verify recipe in
//! `scripts/postinstall.js` (`TARGET_TRIPLES`, `assetName`, `parseSha256Sums`).
//! Pure helpers live here; the `Sys`-driven download orchestration is added in
//! the same file (see `download_verified_binary`).

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
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `rtk cargo test --locked -p pi release::`
Expected: PASS (all `release::tests::*`).

- [ ] **Step 6: Full gate + commit**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
rtk git add crates/bin/src/agent/release.rs crates/bin/src/agent/mod.rs
rtk git commit -m "feat(agent): add release asset math for agent update"
```

---

### Task 2: Sys-driven download + verify + latest-version resolution

Add the `Sys`-based orchestration to `release.rs`: query the latest version, and download-verify-extract the archive. All OS effects go through the existing `Sys` trait so `FakeSys` can drive them.

**Files:**
- Modify: `crates/bin/src/agent/release.rs`

**Interfaces:**
- Consumes: `crate::agent::setup::Sys` (trait), `target_triple`, `asset_name`, `parse_sha256sums`, `parse_latest_tag` (Task 1).
- Produces (used by Task 3):
  - `pub async fn make_tempdir(sys: &dyn Sys) -> Result<String, String>`
  - `pub async fn resolve_latest_version(sys: &dyn Sys, api_base: &str) -> Result<String, String>`
  - `pub async fn download_verified_binary(sys: &dyn Sys, base_url: &str, version: &str, workdir: &str) -> Result<std::path::PathBuf, String>`
- Note: base/api URLs are passed in as parameters (never read from env inside these functions), so tests use explicit strings and never mutate process env.

- [ ] **Step 1: Write the failing tests**

Add these tests inside the existing `#[cfg(test)] mod tests` block in `crates/bin/src/agent/release.rs`:

```rust
    use crate::agent::setup::fake::FakeSys;
    use std::path::Path;

    const API: &str = "https://api.github.com/repos/khmilevoi/rpi-deploy";
    const BASE: &str = "file:///rel";

    #[tokio::test]
    async fn resolve_latest_version_reads_tag_name() {
        let mut sys = FakeSys::default();
        let url = format!("{API}/releases/latest");
        sys.ok.insert(
            FakeSys::key("curl", &["-fsSL", "-H", "Accept: application/vnd.github+json", &url]),
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
        sys.ok.insert(FakeSys::key("uname", &["-m"]), "aarch64".into());
        sys.ok.insert(
            FakeSys::key("curl", &["-fsSL", "-o", &archive, &format!("{BASE}/v{version}/{asset}")]),
            String::new(),
        );
        sys.ok.insert(
            FakeSys::key("curl", &["-fsSL", "-o", &sums, &format!("{BASE}/v{version}/SHA256SUMS")]),
            String::new(),
        );
        sys.files.insert(sums.clone(), format!("{hash}  {asset}\n"));
        sys.ok.insert(FakeSys::key("sha256sum", &[&archive]), format!("{hash}  {archive}"));
        sys.ok.insert(FakeSys::key("tar", &["-xf", &archive, "-C", work]), String::new());
        sys.paths.insert(format!("{work}/rpi"));

        let bin = download_verified_binary(&sys, BASE, version, work).await.unwrap();
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
        sys.ok.insert(FakeSys::key("uname", &["-m"]), "aarch64".into());
        sys.ok.insert(
            FakeSys::key("curl", &["-fsSL", "-o", &archive, &format!("{BASE}/v{version}/{asset}")]),
            String::new(),
        );
        sys.ok.insert(
            FakeSys::key("curl", &["-fsSL", "-o", &sums, &format!("{BASE}/v{version}/SHA256SUMS")]),
            String::new(),
        );
        sys.files.insert(sums.clone(), format!("{}  {asset}\n", "a".repeat(64)));
        sys.ok.insert(
            FakeSys::key("sha256sum", &[&archive]),
            format!("{}  {archive}", "b".repeat(64)),
        );
        let err = download_verified_binary(&sys, BASE, version, work).await.unwrap_err();
        assert!(err.contains("sha256 mismatch"), "{err}");
    }

    #[tokio::test]
    async fn download_verified_binary_rejects_unsupported_arch() {
        let mut sys = FakeSys::default();
        sys.ok.insert(FakeSys::key("uname", &["-m"]), "armv7l".into());
        let err = download_verified_binary(&sys, BASE, "0.22.0", "/tmp/wd").await.unwrap_err();
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
        sys.ok.insert(FakeSys::key("uname", &["-m"]), "aarch64".into());
        sys.ok.insert(
            FakeSys::key("curl", &["-fsSL", "-o", &archive, &format!("{BASE}/v{version}/{asset}")]),
            String::new(),
        );
        sys.ok.insert(
            FakeSys::key("curl", &["-fsSL", "-o", &sums, &format!("{BASE}/v{version}/SHA256SUMS")]),
            String::new(),
        );
        sys.files.insert(sums.clone(), format!("{}  some-other-file.tar.gz\n", "a".repeat(64)));
        let err = download_verified_binary(&sys, BASE, version, work).await.unwrap_err();
        assert!(err.contains("not listed in SHA256SUMS"), "{err}");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `rtk cargo test --locked -p pi release::`
Expected: FAIL to compile ("cannot find function `download_verified_binary`").

- [ ] **Step 3: Write the implementation**

Add to `crates/bin/src/agent/release.rs` (below `parse_latest_tag`, above the test module). Add `use crate::agent::setup::Sys;` and `use std::path::{Path, PathBuf};` to the file's imports:

```rust
use crate::agent::setup::Sys;
use std::path::{Path, PathBuf};

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
        .run("curl", &["-fsSL", "-H", "Accept: application/vnd.github+json", &url])
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
    let arch = sys.run("uname", &["-m"]).await.map_err(|e| format!("uname -m: {e}"))?;
    let triple = target_triple(&arch)
        .ok_or_else(|| format!("unsupported architecture: {}", arch.trim()))?;
    let asset = asset_name(version, triple);
    let base = base_url;
    let archive = format!("{workdir}/{asset}");
    let sums = format!("{workdir}/SHA256SUMS");
    let asset_url = format!("{base}/v{version}/{asset}");
    let sums_url = format!("{base}/v{version}/SHA256SUMS");

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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `rtk cargo test --locked -p pi release::`
Expected: PASS.

- [ ] **Step 5: Full gate + commit**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
rtk git add crates/bin/src/agent/release.rs
rtk git commit -m "feat(agent): download + verify release binary via Sys"
```

---

### Task 3: `rpi agent update` orchestration (board-side)

The board-side command: resolve version, detect channel (npm vs GitHub-direct), obtain the source binary, and apply it via the existing `ensure_installed` + `setup()` + `restart_agent_if_active` path.

**Files:**
- Create: `crates/bin/src/agent/update.rs`
- Modify: `crates/bin/src/agent/mod.rs` (register the module)
- Modify: `crates/bin/src/agent/setup.rs` (expose `resolve_npm_dist_binary`)

**Interfaces:**
- Consumes: `release::{make_tempdir, resolve_latest_version, download_verified_binary}` (Task 2); `setup::{Sys, HostSys, SetupOpts, setup, restart_agent_if_active, resolve_npm_dist_binary}`; `self_install::{ensure_installed, AGENT_BIN_PATH, SelfInstallAction}`.
- Produces (used by Task 7):
  - `pub async fn run_cmd(user: Option<String>, version: Option<String>, dry_run: bool) -> anyhow::Result<()>`
  - `pub(crate) async fn obtain_source_binary(sys: &dyn Sys, login_user: &str, base_url: &str, version: &str, workdir: &str) -> anyhow::Result<std::path::PathBuf>`

- [ ] **Step 1: Expose `resolve_npm_dist_binary`**

In `crates/bin/src/agent/setup.rs`, change the visibility of the existing function (near line 1109) from private to `pub(crate)`:

```rust
pub(crate) async fn resolve_npm_dist_binary(sys: &dyn Sys, login_user: &str) -> Option<PathBuf> {
```

(Only the signature's leading `async fn` gains `pub(crate)`; the body is unchanged.)

- [ ] **Step 2: Register the module**

In `crates/bin/src/agent/mod.rs`, add:

```rust
pub mod update;
```

- [ ] **Step 3: Write the failing tests**

Create `crates/bin/src/agent/update.rs` with the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::setup::fake::FakeSys;
    use std::path::Path;

    /// npm channel: `<npm root>/rpi-deploy/dist/rpi` exists → npm branch runs
    /// `npm i -g rpi-deploy@<version>` and returns the refreshed dist path.
    #[tokio::test]
    async fn obtain_source_uses_npm_branch_when_present() {
        let mut sys = FakeSys::default();
        sys.ok.insert(
            FakeSys::key("sudo", &["-u", "deploy", "-i", "--", "npm", "root", "-g"]),
            "/home/deploy/.npm-global/lib/node_modules".into(),
        );
        let dist = "/home/deploy/.npm-global/lib/node_modules/rpi-deploy/dist/rpi";
        sys.paths.insert(dist.into());
        sys.ok.insert(
            FakeSys::key(
                "sudo",
                &["-u", "deploy", "-i", "--", "npm", "i", "-g", "rpi-deploy@0.22.0"],
            ),
            String::new(),
        );
        let src = obtain_source_binary(&sys, "deploy", "file:///rel", "0.22.0", "/tmp/wd")
            .await
            .unwrap();
        assert_eq!(src, Path::new(dist));
        assert!(sys
            .calls()
            .iter()
            .any(|c| c.contains("npm i -g rpi-deploy@0.22.0")));
    }

    /// GitHub-direct channel: no npm dist → download+verify path is taken.
    #[tokio::test]
    async fn obtain_source_uses_github_branch_when_no_npm() {
        const BASE: &str = "file:///rel";
        let version = "0.22.0";
        let asset = crate::agent::release::asset_name(version, "aarch64-unknown-linux-musl");
        let work = "/tmp/wd";
        let archive = format!("{work}/{asset}");
        let sums = format!("{work}/SHA256SUMS");
        let hash = "d".repeat(64);
        let mut sys = FakeSys::default();
        // npm root fails → no npm branch
        sys.err.insert(FakeSys::key("sudo", &["-u", "deploy", "-i", "--", "npm", "root", "-g"]));
        sys.ok.insert(FakeSys::key("uname", &["-m"]), "aarch64".into());
        sys.ok.insert(
            FakeSys::key("curl", &["-fsSL", "-o", &archive, &format!("{BASE}/v{version}/{asset}")]),
            String::new(),
        );
        sys.ok.insert(
            FakeSys::key("curl", &["-fsSL", "-o", &sums, &format!("{BASE}/v{version}/SHA256SUMS")]),
            String::new(),
        );
        sys.files.insert(sums.clone(), format!("{hash}  {asset}\n"));
        sys.ok.insert(FakeSys::key("sha256sum", &[&archive]), format!("{hash}  {archive}"));
        sys.ok.insert(FakeSys::key("tar", &["-xf", &archive, "-C", work]), String::new());
        sys.paths.insert(format!("{work}/rpi"));

        let src = obtain_source_binary(&sys, "deploy", BASE, version, work).await.unwrap();
        assert_eq!(src, Path::new("/tmp/wd/rpi"));
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `rtk cargo test --locked -p pi update::`
Expected: FAIL to compile ("cannot find function `obtain_source_binary`").

- [ ] **Step 5: Write the implementation**

Prepend to `crates/bin/src/agent/update.rs` (above the test module):

```rust
//! `rpi agent update` — board-side, runs under sudo. Obtains a fresh rpi
//! binary (npm channel or GitHub-direct download) and applies it through the
//! same swap+setup+restart path as `agent setup`. See
//! docs/superpowers/specs/2026-07-13-rpi-remote-agent-update-design.md.

use super::self_install::{self, SelfInstallAction};
use super::setup::{self, HostSys, SetupOpts, Sys};
use std::path::{Path, PathBuf};

/// Resolve the source binary for `version`. npm branch when the login user's
/// global npm has `rpi-deploy` installed (refresh it to `@version`); otherwise
/// download + verify the GitHub release archive from `base_url`.
pub(crate) async fn obtain_source_binary(
    sys: &dyn Sys,
    login_user: &str,
    base_url: &str,
    version: &str,
    workdir: &str,
) -> anyhow::Result<PathBuf> {
    if setup::resolve_npm_dist_binary(sys, login_user).await.is_some() {
        sys.run(
            "sudo",
            &["-u", login_user, "-i", "--", "npm", "i", "-g", &format!("rpi-deploy@{version}")],
        )
        .await
        .map_err(|e| anyhow::anyhow!("npm i -g rpi-deploy@{version}: {e}"))?;
        return setup::resolve_npm_dist_binary(sys, login_user)
            .await
            .ok_or_else(|| anyhow::anyhow!("npm install succeeded but dist/rpi not found"));
    }
    super::release::download_verified_binary(sys, base_url, version, workdir)
        .await
        .map_err(|e| anyhow::anyhow!(e))
}

/// CLI entrypoint for `rpi agent update`. Must run as root (under sudo).
pub async fn run_cmd(
    user: Option<String>,
    version: Option<String>,
    dry_run: bool,
) -> anyhow::Result<()> {
    let login_user = user
        .or_else(|| std::env::var("SUDO_USER").ok())
        .filter(|u| !u.is_empty() && u != "root")
        .ok_or_else(|| {
            anyhow::anyhow!(
                "cannot determine the SSH login user; run via `sudo rpi agent update` or pass --user <name>"
            )
        })?;

    // Read the injectable base/api URLs from env exactly once, here, so the
    // downstream Sys-driven helpers stay env-free and unit-testable.
    let base_url = super::release::release_base_url();
    let api_base = super::release::api_base_url();

    let sys = HostSys;
    let version = match version {
        Some(v) => v.trim_start_matches('v').to_string(),
        None => super::release::resolve_latest_version(&sys, &api_base)
            .await
            .map_err(|e| anyhow::anyhow!(e))?,
    };
    crate::output::info(format!("updating agent to v{version}"));

    if dry_run {
        let channel = if setup::resolve_npm_dist_binary(&sys, &login_user).await.is_some() {
            "npm"
        } else {
            "github-direct"
        };
        crate::output::info(format!(
            "would update via the {channel} channel to v{version} (dry run — no changes made)"
        ));
        return Ok(());
    }

    let workdir = super::release::make_tempdir(&sys)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    let apply = apply_update(&sys, &login_user, &base_url, &version, &workdir).await;
    // Best-effort cleanup regardless of outcome.
    let _ = sys.run("rm", &["-rf", &workdir]).await;
    apply
}

/// Swap in the source binary, re-run the idempotent setup, and restart the
/// agent when the binary actually changed.
async fn apply_update(
    sys: &dyn Sys,
    login_user: &str,
    base_url: &str,
    version: &str,
    workdir: &str,
) -> anyhow::Result<()> {
    let source = obtain_source_binary(sys, login_user, base_url, version, workdir).await?;

    let action = self_install::ensure_installed(
        &source,
        Path::new(self_install::AGENT_BIN_PATH),
        false,
    )
    .map_err(|e| anyhow::anyhow!("self-install {}: {e}", self_install::AGENT_BIN_PATH))?;

    match &action {
        SelfInstallAction::UpToDate | SelfInstallAction::AlreadyCanonical => {
            crate::output::success(format!(
                "ok (already on the requested binary): {}",
                self_install::AGENT_BIN_PATH
            ));
        }
        SelfInstallAction::Installed => {
            crate::output::success(format!(
                "installed: {} (v{version})",
                self_install::AGENT_BIN_PATH
            ));
        }
    }

    let opts = SetupOpts {
        login_user: login_user.to_string(),
        with_cloudflared: false,
        dry_run: false,
        cf_token: None,
        domain: None,
        tunnel_name: None,
    };
    let report = setup::setup(sys, &opts).await;
    report.print();
    if !report.errors.is_empty() {
        anyhow::bail!("update completed with {} error(s); see above", report.errors.len());
    }

    if matches!(action, SelfInstallAction::Installed) {
        if let Some(note) = setup::restart_agent_if_active(sys).await {
            crate::output::info(note);
        }
    }
    Ok(())
}
```

Note: `setup::setup` takes `&dyn Sys`; `HostSys` coerces. `apply_update`/`obtain_source_binary` take `&dyn Sys` so the same body is exercised by `FakeSys` in tests.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `rtk cargo test --locked -p pi update::`
Expected: PASS. Also run `rtk cargo test --locked -p pi setup::` to confirm the visibility change did not break `setup`'s tests.

- [ ] **Step 7: Full gate + commit**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
rtk git add crates/bin/src/agent/update.rs crates/bin/src/agent/mod.rs crates/bin/src/agent/setup.rs
rtk git commit -m "feat(agent): add rpi agent update command"
```

---

### Task 4: SSH-exec TTY runner (client)

The client triggers the board with an allocated TTY (so a remote `sudo` can prompt) and inherited stdio, instead of the `-N -L` socket forward. Reuse the existing `SshExec` (it already handles profile/key), adding a TTY-exec method with a testable arg builder.

**Files:**
- Modify: `crates/bin/src/cli/ssh.rs`

**Interfaces:**
- Consumes: `crate::cli::config::ServerProfile`, `crate::cli::tunnel::expand_home` (already imported in ssh.rs).
- Produces (used by Task 6):
  - `pub(crate) fn tty_args(&self, remote: &[&str]) -> Vec<String>` on `SshExec`
  - `pub async fn run_tty(&self, remote: &[&str]) -> anyhow::Result<()>` on `SshExec`

- [ ] **Step 1: Write the failing test**

Add a test module at the bottom of `crates/bin/src/cli/ssh.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::config::ServerProfile;

    #[test]
    fn tty_args_include_tty_flag_and_remote_command() {
        let profile = ServerProfile {
            host: "pi.local".into(),
            user: "deploy".into(),
            key: Some("/home/u/.ssh/pi".into()),
        };
        let ssh = SshExec { profile: &profile };
        let args = ssh.tty_args(&["sudo", "rpi", "agent", "update", "--version", "0.22.0"]);
        assert_eq!(args[0], "-i");
        assert_eq!(args[1], "/home/u/.ssh/pi");
        assert!(args.contains(&"-t".to_string()));
        assert!(args.contains(&"deploy@pi.local".to_string()));
        // remote command tail is preserved in order
        let tail = &args[args.len() - 6..];
        assert_eq!(
            tail,
            &["sudo", "rpi", "agent", "update", "--version", "0.22.0"]
        );
        // BatchMode must NOT be forced (sudo may need to prompt)
        assert!(!args.iter().any(|a| a.contains("BatchMode")));
    }

    #[test]
    fn tty_args_omit_key_flag_when_no_key() {
        let profile = ServerProfile {
            host: "pi.local".into(),
            user: "deploy".into(),
            key: None,
        };
        let ssh = SshExec { profile: &profile };
        let args = ssh.tty_args(&["true"]);
        assert!(!args.contains(&"-i".to_string()));
        assert_eq!(args[0], "-t");
        assert!(args.contains(&"deploy@pi.local".to_string()));
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `rtk cargo test --locked -p pi ssh::tests`
Expected: FAIL to compile ("no method named `tty_args`").

- [ ] **Step 3: Write the implementation**

Add to the `impl SshExec<'_>` block in `crates/bin/src/cli/ssh.rs`:

```rust
    /// The full `ssh` argv (after the program name) for an interactive
    /// TTY-exec of `remote`: `-i <key>` when a key is set, `-t` to allocate a
    /// TTY (so a remote `sudo` can prompt), `<user>@<host>`, then the remote
    /// command words. Deliberately does NOT set `BatchMode=yes` — the
    /// interactive `-t` default is what ships.
    pub(crate) fn tty_args(&self, remote: &[&str]) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        if let Some(key) = &self.profile.key {
            args.push("-i".into());
            args.push(expand_home(key));
        }
        args.push("-t".into());
        args.push(format!("{}@{}", self.profile.user, self.profile.host));
        args.extend(remote.iter().map(|s| s.to_string()));
        args
    }

    /// Run `remote` on the board over an interactive `ssh -t` session with
    /// inherited stdio, so a remote `sudo` prompt reaches the operator's own
    /// terminal. Errors if the remote command exits nonzero.
    pub async fn run_tty(&self, remote: &[&str]) -> anyhow::Result<()> {
        let mut cmd = tokio::process::Command::new("ssh");
        cmd.args(self.tty_args(remote));
        cmd.stdin(std::process::Stdio::inherit());
        cmd.stdout(std::process::Stdio::inherit());
        cmd.stderr(std::process::Stdio::inherit());
        let status = cmd.status().await?;
        if !status.success() {
            anyhow::bail!("remote command exited with {status}");
        }
        Ok(())
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `rtk cargo test --locked -p pi ssh::tests`
Expected: PASS.

- [ ] **Step 5: Full gate + commit**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
rtk git add crates/bin/src/cli/ssh.rs
rtk git commit -m "feat(cli): add ssh -t exec runner for remote sudo"
```

---

### Task 5: `rpi upgrade` orchestration (client)

The client-side pult: resolve target version, show `current → target`, confirm, trigger over SSH, verify.

**Files:**
- Create: `crates/bin/src/cli/upgrade.rs`
- Modify: `crates/bin/src/cli/mod.rs` (register the module)

**Interfaces:**
- Consumes: `cli::config::{ConnectOpts, ServerProfile}`, `cli::ssh::SshExec` (Task 4), `cli::tunnel::SshTunnel`, `cli::api::ApiClient`, `cli::prompt::{InquirePrompter, Prompter}`, `crate::agent::release::parse_latest_tag` (Task 1).
- Produces (used by Task 7):
  - `pub async fn run(version: Option<String>, yes: bool, connect: ConnectOpts) -> anyhow::Result<()>`
  - `pub async fn resolve_target_version(flag: Option<String>) -> anyhow::Result<String>` (unit-tested for the non-network branches)

- [ ] **Step 1: Register the module**

In `crates/bin/src/cli/mod.rs`, add (keep alphabetical-ish ordering near `tunnel`):

```rust
pub mod upgrade;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/bin/src/cli/upgrade.rs` with the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn target_defaults_to_client_version() {
        let v = resolve_target_version(None).await.unwrap();
        assert_eq!(v, env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn explicit_version_strips_leading_v() {
        assert_eq!(resolve_target_version(Some("v0.22.0".into())).await.unwrap(), "0.22.0");
        assert_eq!(resolve_target_version(Some("0.22.0".into())).await.unwrap(), "0.22.0");
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `rtk cargo test --locked -p pi upgrade::`
Expected: FAIL to compile ("cannot find function `resolve_target_version`").

- [ ] **Step 4: Write the implementation**

Prepend to `crates/bin/src/cli/upgrade.rs` (above the test module):

```rust
//! `rpi upgrade` — client-side pult. Triggers the board to update its own rpi
//! binary via `ssh -t <user>@<host> sudo rpi agent update --version <X>`. See
//! docs/superpowers/specs/2026-07-13-rpi-remote-agent-update-design.md.

use crate::cli::api::ApiClient;
use crate::cli::config::{ConnectOpts, ServerProfile};
use crate::cli::prompt::{InquirePrompter, Prompter};
use crate::cli::ssh::SshExec;
use crate::cli::tunnel::SshTunnel;

/// Resolve the version `rpi upgrade` will bring the board to: no flag → the
/// client's own version (keeps the client↔agent pair aligned); `latest` → the
/// newest published release; otherwise the explicit version (leading `v`
/// stripped).
pub async fn resolve_target_version(flag: Option<String>) -> anyhow::Result<String> {
    match flag.as_deref() {
        None => Ok(env!("CARGO_PKG_VERSION").to_string()),
        Some("latest") => github_latest_version().await,
        Some(v) => Ok(v.trim_start_matches('v').to_string()),
    }
}

/// Newest published release version (no leading `v`) via the GitHub API.
async fn github_latest_version() -> anyhow::Result<String> {
    let url = format!("{}/releases/latest", crate::agent::release::api_base_url());
    let body = reqwest::Client::new()
        .get(url)
        .header("User-Agent", "rpi-deploy")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    crate::agent::release::parse_latest_tag(&body).map_err(|e| anyhow::anyhow!(e))
}

/// Read the agent's reported version through a short-lived tunnel + handshake.
async fn read_agent_version(profile: &ServerProfile) -> Option<String> {
    let tunnel = SshTunnel::open(profile).await.ok()?;
    let api = ApiClient::new(tunnel.base_url.clone());
    api.version().await.ok().map(|v| v.version)
}

pub async fn run(version: Option<String>, yes: bool, connect: ConnectOpts) -> anyhow::Result<()> {
    if std::env::var("PI_AGENT_URL").is_ok() {
        anyhow::bail!(
            "rpi upgrade needs SSH access to the board; it is not applicable with PI_AGENT_URL set (local dev)"
        );
    }
    let profile = connect.resolve()?;
    let target = resolve_target_version(version).await?;

    match read_agent_version(&profile).await {
        Some(current) => crate::output::info(format!("agent update: {current} -> {target}")),
        None => crate::output::info(format!("agent update: (current unknown) -> {target}")),
    }

    if !yes {
        let mut p = InquirePrompter;
        if !p.confirm(&format!("update the board to v{target}?"), true)? {
            crate::output::info("aborted");
            return Ok(());
        }
    }

    let ssh = SshExec { profile: &profile };
    ssh.run_tty(&["sudo", "rpi", "agent", "update", "--version", &target])
        .await?;

    match read_agent_version(&profile).await {
        Some(v) if v == target => crate::output::success(format!("board is now on v{v}")),
        Some(v) => crate::output::warn(format!(
            "board reports v{v}, expected v{target} (a restart may still be pending)"
        )),
        None => crate::output::warn("could not read the board version after update"),
    }
    Ok(())
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `rtk cargo test --locked -p pi upgrade::`
Expected: PASS.

- [ ] **Step 6: Full gate + commit**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
rtk git add crates/bin/src/cli/upgrade.rs crates/bin/src/cli/mod.rs
rtk git commit -m "feat(cli): add rpi upgrade command"
```

---

### Task 6: Wire both subcommands into the CLI

Add `rpi upgrade` (bare verb) and `rpi agent update` (agent namespace) to the clap tree and dispatch, with parse tests. Follow the `add-cli-command` skill.

**Files:**
- Modify: `crates/bin/src/main.rs`

**Interfaces:**
- Consumes: `cli::upgrade::run` (Task 5), `agent::update::run_cmd` (Task 3).

- [ ] **Step 1: Write the failing parse tests**

Add to the `#[cfg(test)] mod tests` block in `crates/bin/src/main.rs`:

```rust
    #[test]
    fn upgrade_flags_parse() {
        let cli = Cli::try_parse_from([
            "rpi", "upgrade", "--version", "0.22.0", "--yes", "--server", "home",
        ])
        .unwrap();
        match cli.cmd.unwrap() {
            Cmd::Upgrade { version, yes, connect } => {
                assert_eq!(version.as_deref(), Some("0.22.0"));
                assert!(yes);
                assert_eq!(connect.server.as_deref(), Some("home"));
            }
            _ => panic!("expected upgrade"),
        }
    }

    #[test]
    fn upgrade_bare_parses_with_defaults() {
        let cli = Cli::try_parse_from(["rpi", "upgrade"]).unwrap();
        match cli.cmd.unwrap() {
            Cmd::Upgrade { version, yes, .. } => {
                assert_eq!(version, None);
                assert!(!yes);
            }
            _ => panic!("expected upgrade"),
        }
    }

    #[test]
    fn agent_update_flags_parse() {
        let cli = Cli::try_parse_from([
            "rpi", "agent", "update", "--version", "0.22.0", "--user", "deploy", "--dry-run",
        ])
        .unwrap();
        match cli.cmd.unwrap() {
            Cmd::Agent { cmd: AgentCmd::Update { user, version, dry_run } } => {
                assert_eq!(user.as_deref(), Some("deploy"));
                assert_eq!(version.as_deref(), Some("0.22.0"));
                assert!(dry_run);
            }
            _ => panic!("expected agent update"),
        }
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `rtk cargo test --locked -p pi -- upgrade_flags_parse agent_update_flags_parse upgrade_bare_parses_with_defaults`
Expected: FAIL to compile ("no variant `Upgrade`").

- [ ] **Step 3: Add the `Upgrade` variant to `Cmd`**

In `crates/bin/src/main.rs`, inside `enum Cmd { … }`, add after the `Setup(SetupArgs),` line:

```rust
    /// Update the agent on the board to a chosen version (SSH + sudo)
    Upgrade {
        /// Target version (default: this CLI's version; `latest` = newest release)
        #[arg(long)]
        version: Option<String>,
        /// Skip the confirmation prompt
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
```

- [ ] **Step 4: Add the `Update` variant to `AgentCmd`**

In `enum AgentCmd { … }`, add after the `Setup { … }` variant (before `Migrate`):

```rust
    /// Update this board's agent binary (run with sudo; downloads, verifies, swaps, restarts)
    Update {
        /// SSH login user for npm-channel detection (default: $SUDO_USER)
        #[arg(long)]
        user: Option<String>,
        /// Target version (default: latest published release)
        #[arg(long)]
        version: Option<String>,
        /// Resolve + report without downloading or swapping
        #[arg(long)]
        dry_run: bool,
    },
```

- [ ] **Step 5: Add the dispatch arms**

In the `match cmd { … }` in `run()`, add the `Upgrade` arm after the `Cmd::Setup(a) => { … }` arm:

```rust
        Cmd::Upgrade { version, yes, connect } => cli::upgrade::run(version, yes, connect).await,
```

And add the agent arm alongside the other `Cmd::Agent { cmd: AgentCmd::… }` arms (e.g. after the `AgentCmd::Setup { … }` arm):

```rust
        Cmd::Agent {
            cmd: AgentCmd::Update { user, version, dry_run },
        } => agent::update::run_cmd(user, version, dry_run).await,
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `rtk cargo test --locked -p pi -- upgrade_flags_parse agent_update_flags_parse upgrade_bare_parses_with_defaults`
Expected: PASS.

- [ ] **Step 7: Full gate + commit**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
rtk git add crates/bin/src/main.rs
rtk git commit -m "feat(cli): wire rpi upgrade and rpi agent update"
```

---

### Task 7: `install.sh` — one-line, no-npm installer

A standalone POSIX shell installer that shares the GitHub-direct download/verify recipe. Verified offline by a Node test using a `file://` fixture release.

**Files:**
- Create: `scripts/install.sh`
- Create: `scripts/install.test.mjs`

**Interfaces:**
- Env overrides consumed: `RPI_VERSION`, `RPI_INSTALL_DIR`, `RPI_RELEASE_BASE_URL`, `RPI_RELEASE_API_URL`. Same defaults as the Global Constraints.

- [ ] **Step 1: Write the failing test**

Create `scripts/install.test.mjs`:

```javascript
import test from 'node:test';
import assert from 'node:assert';
import { spawnSync, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const INSTALL_SH = path.join(HERE, 'install.sh');

// The installer is POSIX shell that shells out to curl/tar/sha256sum — only
// runnable on a POSIX host that has those tools. Skip elsewhere (Windows CI).
function toolsAvailable() {
  if (process.platform === 'win32') return false;
  for (const t of ['sh', 'curl', 'tar', 'sha256sum']) {
    if (spawnSync('sh', ['-c', `command -v ${t}`], { stdio: 'ignore' }).status !== 0) return false;
  }
  return true;
}

const TRIPLES = {
  'linux-x64': 'x86_64-unknown-linux-musl',
  'linux-arm64': 'aarch64-unknown-linux-musl',
};
const triple = TRIPLES[`${process.platform}-${process.arch}`];
const skip = !toolsAvailable() || !triple;

// Build a fixture release dir: <rel>/v<version>/rpi-v<version>-<triple>.tar.gz
// (a tar containing a fake `rpi`) + a matching SHA256SUMS.
function stageRelease(root, version, { corrupt = false } = {}) {
  const relDir = path.join(root, 'v' + version);
  fs.mkdirSync(relDir, { recursive: true });
  const stage = fs.mkdtempSync(path.join(os.tmpdir(), 'rpi-stage-'));
  const fakeBin = path.join(stage, 'rpi');
  fs.writeFileSync(fakeBin, '#!/bin/sh\necho fake-rpi\n');
  fs.chmodSync(fakeBin, 0o755);
  const asset = `rpi-v${version}-${triple}.tar.gz`;
  execFileSync('tar', ['-C', stage, '-czf', path.join(relDir, asset), 'rpi']);
  const bytes = fs.readFileSync(path.join(relDir, asset));
  const hash = corrupt ? 'f'.repeat(64) : createHash('sha256').update(bytes).digest('hex');
  fs.writeFileSync(path.join(relDir, 'SHA256SUMS'), `${hash}  ${asset}\n`);
  return asset;
}

function runInstaller(env) {
  return spawnSync('sh', [INSTALL_SH], { env: { ...process.env, ...env }, encoding: 'utf8' });
}

test('install.sh downloads, verifies and installs the binary', { skip }, () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'rpi-rel-'));
  const binDir = fs.mkdtempSync(path.join(os.tmpdir(), 'rpi-bin-'));
  stageRelease(root, '9.9.9');
  const res = runInstaller({
    RPI_VERSION: '9.9.9',
    RPI_INSTALL_DIR: binDir,
    RPI_RELEASE_BASE_URL: pathToFileURL(root).href,
  });
  assert.equal(res.status, 0, res.stderr);
  const installed = path.join(binDir, 'rpi');
  assert.ok(fs.existsSync(installed), 'rpi binary installed');
  assert.match(fs.readFileSync(installed, 'utf8'), /fake-rpi/);
});

test('install.sh rejects a sha256 mismatch', { skip }, () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'rpi-rel-'));
  const binDir = fs.mkdtempSync(path.join(os.tmpdir(), 'rpi-bin-'));
  stageRelease(root, '9.9.9', { corrupt: true });
  const res = runInstaller({
    RPI_VERSION: '9.9.9',
    RPI_INSTALL_DIR: binDir,
    RPI_RELEASE_BASE_URL: pathToFileURL(root).href,
  });
  assert.notEqual(res.status, 0);
  assert.match(res.stderr + res.stdout, /sha256 mismatch/);
  assert.ok(!fs.existsSync(path.join(binDir, 'rpi')), 'nothing installed on mismatch');
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npm run test:node -- scripts/install.test.mjs`
Expected: FAIL (the tests error because `scripts/install.sh` does not exist yet → non-zero status / missing file).

- [ ] **Step 3: Write `scripts/install.sh`**

Create `scripts/install.sh`:

```sh
#!/bin/sh
# One-line installer for the rpi deploy CLI/agent binary (no npm/node needed).
# Usage:  curl -fsSL <raw-url>/scripts/install.sh | sh
# Env overrides:
#   RPI_VERSION           target version (default: latest published release)
#   RPI_INSTALL_DIR       install dir    (default: /usr/local/bin)
#   RPI_RELEASE_BASE_URL  release download root (default: GitHub Releases)
#   RPI_RELEASE_API_URL   GitHub API base (default: api.github.com repo)
# Shares the download/verify recipe with scripts/postinstall.js and the
# board-side `rpi agent update`.
set -eu

REPO="khmilevoi/rpi-deploy"
BASE_URL="${RPI_RELEASE_BASE_URL:-https://github.com/${REPO}/releases/download}"
API_URL="${RPI_RELEASE_API_URL:-https://api.github.com/repos/${REPO}}"
INSTALL_DIR="${RPI_INSTALL_DIR:-/usr/local/bin}"

log() { echo "rpi-install: $*"; }
fail() { echo "rpi-install: error: $*" >&2; exit 1; }

# 1. arch -> target triple
arch="$(uname -m)"
case "$arch" in
  aarch64 | arm64) triple="aarch64-unknown-linux-musl" ;;
  x86_64 | amd64) triple="x86_64-unknown-linux-musl" ;;
  *) fail "unsupported architecture: $arch" ;;
esac

# 2. resolve version
version="${RPI_VERSION:-}"
if [ -z "$version" ]; then
  version="$(curl -fsSL -H 'Accept: application/vnd.github+json' "${API_URL}/releases/latest" \
    | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -n1 \
    | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\{0,1\}\([^"]*\)".*/\1/')"
  [ -n "$version" ] || fail "could not resolve the latest release version"
fi
version="${version#v}"

asset="rpi-v${version}-${triple}.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# 3. download + verify
log "downloading ${asset} ..."
curl -fsSL -o "${tmp}/${asset}" "${BASE_URL}/v${version}/${asset}"
curl -fsSL -o "${tmp}/SHA256SUMS" "${BASE_URL}/v${version}/SHA256SUMS"

expected="$(awk -v a="$asset" '$2 == a || $2 == "*" a { print $1; exit }' "${tmp}/SHA256SUMS")"
[ -n "$expected" ] || fail "${asset} not listed in SHA256SUMS"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "${tmp}/${asset}" | awk '{ print $1 }')"
else
  actual="$(shasum -a 256 "${tmp}/${asset}" | awk '{ print $1 }')"
fi
[ "$actual" = "$expected" ] || fail "sha256 mismatch for ${asset}: expected ${expected}, got ${actual}"

# 4. extract + install
tar -xf "${tmp}/${asset}" -C "$tmp"
[ -f "${tmp}/rpi" ] || fail "archive did not contain rpi"
chmod 0755 "${tmp}/rpi"

if [ -w "$INSTALL_DIR" ]; then
  install -m 0755 "${tmp}/rpi" "${INSTALL_DIR}/rpi"
else
  log "sudo required to write ${INSTALL_DIR}"
  sudo install -m 0755 "${tmp}/rpi" "${INSTALL_DIR}/rpi"
fi

log "installed rpi v${version} to ${INSTALL_DIR}/rpi"

# 5. next steps (do NOT run setup here)
log "next steps:"
log "  Raspberry Pi agent: sudo rpi agent setup   (Docker must already be installed)"
log "  developer machine:  rpi setup"
```

- [ ] **Step 4: Verify shell syntax and run the test**

```bash
sh -n scripts/install.sh
npm run test:node -- scripts/install.test.mjs
```
Expected: `sh -n` prints nothing (valid syntax); the two install tests PASS (or SKIP on a non-POSIX host).

- [ ] **Step 5: Commit**

```bash
rtk git add scripts/install.sh scripts/install.test.mjs
rtk git commit -m "feat: add no-npm install.sh bootstrap installer"
```

---

### Task 8: e2e scenario — board-side `agent update` against a `file://` fixture

Prove the risky new board-side logic end-to-end: `rpi agent update` downloads a release-shaped archive from an env-injected `file://` base, verifies its SHA256, and swaps `/usr/local/bin/rpi`. The scenario downgrades the canonical binary to the pre-built legacy `rpi-legacy` (v0.17.1) so the swap is observable via `rpi --version`.

Scope note: the offline e2e exercises the board-side `agent update` (the new download/verify/swap). The client wrapper `rpi upgrade` is not run here — forwarding `RPI_RELEASE_BASE_URL` through ssh+sudo is out of scope for an offline harness, and its ssh-argv construction is unit-tested in Task 4. There is no systemd in the e2e container, so `restart_agent_if_active` is a no-op; the assertion is on the swapped on-disk binary, not a live agent restart.

**Files:**
- Modify: `tests/e2e/Dockerfile` (add `sudo` + a test-only NOPASSWD sudoers rule for `deploy`)
- Create: `tests/e2e/scenarios/agent-update/scenario.sh`

**Interfaces:**
- Consumes: the e2e `lib.sh` helpers (`e2e_bootstrap`, `run_capture`, `assert_log`, `fail`, `SSH`, `CONNECT`), the pre-built `/usr/local/bin/rpi-legacy` (Dockerfile), and the `RPI_RELEASE_BASE_URL` override (Task 2).

- [ ] **Step 1: Add sudo + sudoers to the e2e runtime image**

In `tests/e2e/Dockerfile`, add `sudo` to the runtime `apt-get install` list (line ~24-25). Change:

```dockerfile
RUN apt-get update && apt-get install -y --no-install-recommends \
      bash ca-certificates curl git libgcc-s1 openssh-client openssh-server passwd util-linux && \
    rm -rf /var/lib/apt/lists/*
```

to:

```dockerfile
RUN apt-get update && apt-get install -y --no-install-recommends \
      bash ca-certificates curl git libgcc-s1 openssh-client openssh-server passwd sudo util-linux && \
    rm -rf /var/lib/apt/lists/*
```

Then, in the `RUN groupadd … && … && rpi --version && rpi-legacy --version` block (line ~33-39), add a test-only NOPASSWD sudoers rule for `deploy` immediately after the `usermod --append --groups rpi-agent deploy && \` line:

```dockerfile
    usermod --append --groups rpi-agent deploy && \
    printf 'deploy ALL=(root) NOPASSWD: ALL\n' > /etc/sudoers.d/deploy-e2e && \
    chmod 0440 /etc/sudoers.d/deploy-e2e && \
```

(This is e2e-only; the shipped sudoers guidance in the README is the narrow `rpi agent update` rule, not this blanket one.)

- [ ] **Step 2: Write the scenario**

Create `tests/e2e/scenarios/agent-update/scenario.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

# Board-side `rpi agent update` against an offline file:// fixture release.
# Proves the new download -> verify(SHA256) -> swap path: we serve the
# pre-built legacy binary (v0.17.1) as the "release" for version 0.17.1 and
# assert `rpi agent update --version 0.17.1` swaps /usr/local/bin/rpi (which
# ships as the CURRENT build) down to it. No systemd here, so this asserts the
# on-disk swap, not a live agent restart.

source /opt/e2e/lib.sh
e2e_bootstrap

# Precondition: the canonical binary is NOT already 0.17.1 (else the swap would
# be a no-op and prove nothing).
before=$("${SSH[@]}" rpi --version)
echo "$before" | grep -q '0.17.1' && fail "precondition: /usr/local/bin/rpi is already 0.17.1"

# Stage a release-shaped fixture on the target, served via file://. The archive
# must contain a member named `rpi`, so copy rpi-legacy -> rpi before taring.
"${SSH[@]}" sudo sh -euc '
  arch=$(uname -m)
  case "$arch" in
    aarch64|arm64) triple=aarch64-unknown-linux-musl ;;
    x86_64|amd64)  triple=x86_64-unknown-linux-musl ;;
    *) echo "unsupported arch: $arch" >&2; exit 1 ;;
  esac
  work=$(mktemp -d)
  cp /usr/local/bin/rpi-legacy "$work/rpi"
  d=/opt/e2e-release/v0.17.1
  mkdir -p "$d"
  tar -C "$work" -czf "$d/rpi-v0.17.1-$triple.tar.gz" rpi
  ( cd "$d" && sha256sum "rpi-v0.17.1-$triple.tar.gz" > SHA256SUMS )
  rm -rf "$work"
'

# Run the update as root with the fixture base URL injected. `env` sets the var
# for the rpi child regardless of the sudoers env policy; SUDO_USER=deploy is
# preserved by sudo so npm-channel detection resolves (npm absent -> github).
run_capture update.log "${SSH[@]}" \
  sudo env RPI_RELEASE_BASE_URL=file:///opt/e2e-release \
  rpi agent update --version 0.17.1
assert_log update.log 'installed'
assert_log update.log 'v0.17.1'

# The canonical binary is now the legacy build.
after=$("${SSH[@]}" rpi --version)
echo "$after" | grep -q '0.17.1' || fail "rpi --version after update: $after (expected 0.17.1)"

echo 'rpi e2e: PASS'
```

- [ ] **Step 3: Make the script executable / normalized**

The e2e Dockerfile already runs `find /opt/e2e -name '*.sh' -exec sed -i 's/\r$//' {} + -exec chmod 0755 {} +`, so no manual chmod is needed — but ensure the file uses LF line endings when created.

- [ ] **Step 4: Run the new scenario locally (Docker required)**

Prepare the legacy source once (if not already present), then run just this scenario:

```bash
git fetch --no-tags --depth=1 origin +refs/tags/v0.17.1:refs/tags/v0.17.1
node tests/e2e/prepare-legacy.mjs
node tests/e2e/run.mjs agent-update
```
Expected: `rpi e2e: 1/1 scenarios passed` with `agent-update  PASS`.

If Docker is unavailable in this environment, skip local execution and rely on the CI `e2e` job; note this explicitly in the task's completion report rather than claiming it passed.

- [ ] **Step 5: Commit**

```bash
rtk git add tests/e2e/Dockerfile tests/e2e/scenarios/agent-update/scenario.sh
rtk git commit -m "test(e2e): add board-side agent update scenario"
```

---

### Task 9: Documentation + skill update

Document the two commands, `install.sh`, and the narrow sudoers rule; add them to the rpi-cli skill so future sessions know they exist.

**Files:**
- Modify: `README.md`
- Modify: `plugins/rpi/skills/rpi-cli/SKILL.md`

**Interfaces:** none (docs only).

- [ ] **Step 1: Find the right anchors**

```bash
rtk grep -n "agent setup" README.md
rtk grep -n "rpi agent" plugins/rpi/skills/rpi-cli/SKILL.md
```
Use the results to place the new content next to the existing agent-management / install sections (do not duplicate an existing heading).

- [ ] **Step 2: Add a README section**

Add a section to `README.md` near the agent/install documentation. Content (adjust heading level to match surrounding sections):

````markdown
### Updating the agent

Update the rpi binary on a board to a chosen version from your laptop:

```bash
rpi upgrade                 # bring the board up to this CLI's version
rpi upgrade --version 0.22.0
rpi upgrade --version latest --yes
```

`rpi upgrade` opens `ssh -t <user>@<host> sudo rpi agent update --version <X>`,
so a board whose sudo needs a password will prompt in your own terminal. It
reuses your existing SSH profile (`--server` / `PI_SERVER` / default), shows
`current → target`, and re-reads `/v1/version` afterwards to confirm.

On the board, `rpi agent update` downloads the release archive
(`rpi-v<version>-<triple>.tar.gz`) from GitHub Releases, verifies its SHA256
against the release `SHA256SUMS`, swaps `/usr/local/bin/rpi`, re-runs the
idempotent `rpi agent setup`, and restarts `rpi-agent`. If the board was
installed via npm, it refreshes the global `rpi-deploy@<version>` instead.

For unattended updates, add a narrow sudoers rule (not blanket NOPASSWD):

```
<login-user> ALL=(root) NOPASSWD: /usr/local/bin/rpi agent update *
```

### Installing without npm

```bash
curl -fsSL https://raw.githubusercontent.com/khmilevoi/rpi-deploy/master/scripts/install.sh | sh
```

Downloads and verifies the prebuilt binary and installs it to
`/usr/local/bin` (override with `RPI_INSTALL_DIR`); `RPI_VERSION` pins a
version (default: latest). It does not run setup — follow with
`sudo rpi agent setup` on a Pi, or `rpi setup` on a dev machine.
````

- [ ] **Step 3: Add the commands to the rpi-cli skill**

In `plugins/rpi/skills/rpi-cli/SKILL.md`, add `rpi upgrade` and `rpi agent update` to the command reference next to the other `rpi agent …` entries, matching the file's existing formatting. Include: `rpi upgrade [--version <X|latest>] [--yes]` (client-side board update over SSH + sudo) and `rpi agent update [--version <X>] [--user <u>] [--dry-run]` (board-side; run with sudo). Mention `install.sh` as the no-npm bootstrap.

- [ ] **Step 4: Commit**

```bash
rtk git add README.md plugins/rpi/skills/rpi-cli/SKILL.md
rtk git commit -m "docs: document rpi upgrade, agent update, and install.sh"
```

---

## Final verification (after all tasks)

Run the full CI-equivalent gate and confirm the whole feature builds and tests green:

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
npm run test:node
```

If Docker is available, also run the e2e scenario (`node tests/e2e/run.mjs agent-update`). Then the plan is complete and ready for `/release` (a minor bump: 0.21.1 → 0.22.0).

## Self-review notes (spec coverage)

- `rpi agent update [--version] [--dry-run]` (spec §Commands) → Tasks 3, 6.
- Version resolution: explicit / default-to-client-version / `latest` (spec §Testing) → Task 5 (client default/explicit unit-tested; `latest` via GitHub API), Task 2 (`resolve_latest_version` board-side).
- Channel detection npm vs GitHub-direct (spec §Commands step 2) → Task 3 (`obtain_source_binary`, both branches unit-tested).
- SHA256 verify: match / mismatch / missing-from-SHA256SUMS (spec §Testing) → Task 2 tests.
- arch → triple incl. unsupported (spec §Testing) → Tasks 1, 2 tests.
- Reuse `self_install`/`restart_agent_if_active`/`resolve_npm_dist_binary`/`setup` (spec §Reuse) → Task 3.
- `rpi upgrade [--version <X|latest>] [--yes]` + connection flags (spec §Commands) → Tasks 5, 6.
- SSH-exec TTY runner, sibling of `SshTunnel` (spec §Commands) → Task 4.
- `PI_AGENT_URL`-set → clear error (spec §Commands) → Task 5 (`run` guard).
- `install.sh` with `RPI_VERSION` / `RPI_INSTALL_DIR` / injectable base (spec §install.sh, §Testing) → Task 7.
- Injectable release base URL for board + install.sh (spec §Testing) → `RPI_RELEASE_BASE_URL` (Tasks 2, 7).
- e2e against a local fixture release server via env-overridable base (spec §Testing) → Task 8.
- sudoers documentation, interactive `ssh -t` default (spec §sudo) → Tasks 4 (run_tty), 9 (README).
