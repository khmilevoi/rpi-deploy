# Security Review: Prebuilt Binaries (v0.7) Branch

- **Scope**: `worktree-feat-prebuilt-binaries` branch diff vs `master`
- **Date**: 2026-07-07
- **Reviewed changes**: `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `scripts/postinstall.js` (prebuilt-binary download path), `package.json`, and formatting-only changes across `crates/**`

## Result: No findings

No vulnerabilities met the high-confidence / concrete-exploit bar. Two changes introduce new attack surface and were reviewed in depth; both hold up.

### `scripts/postinstall.js` — `downloadPrebuilt()`

- Download URL is built from a hardcoded repo (`khmilevoi/rpi-deploy`) and the installed package's own `version` — not attacker-influenceable at install time.
- The archive is SHA-256 checksum-verified against a `SHA256SUMS` file (also fetched from the same release) **before** extraction; a mismatch aborts and falls back to source build.
- Extraction uses `spawnSync('tar', ['-xf', archive, '-C', tmp], ...)` — an argv array, not a shell string, so no command injection via the filename/path components.
- Archive contents are produced by this repo's own `release.yml` packaging step (a single `rpi`/`rpi.exe` binary, no nested paths), so zip-slip / path-traversal-via-archive-entries is not a realistic vector here.
- Temp directory via `mkdtempSync`, permissions set explicitly (`0o755`), and cleanup happens in a `finally` block.
- No shell is invoked for the download/verify/extract/install path.

### `.github/workflows/release.yml` / `ci.yml`

- Workflow-level `permissions: contents: read`; elevated permissions (`contents: write`, `id-token: write`) are scoped only to the specific jobs that need them (`release`, `npm-publish`).
- Triggers are `push: tags`, `workflow_dispatch`, and `pull_request` — never `pull_request_target` — so forked/untrusted PRs never execute with secrets or write tokens.
- No GitHub Actions context expressions (PR titles, branch names, etc.) are interpolated directly into `run:` shell blocks; the one ref usage (`$GITHUB_REF_NAME`) flows through an environment variable, not template expansion, so it isn't shell-injectable.
- `NPM_TOKEN` / `GH_TOKEN` are referenced only via `env:`, never echoed or interpolated into command strings.

### Rust source changes (`deploy.rs`, `env.rs`, `lifecycle.rs`, `list.rs`, `mask.rs`, `agent/http.rs`, `agent/setup.rs`, `agent/uninstall.rs`, `cli/*.rs`, `docker.rs`, `overrides.rs`, `probe.rs`, `sqlite.rs`, `entities.rs`, `main.rs`)

- Diffed byte-for-byte against `master`: every change is a `cargo fmt` reformat or a clippy fix (`sort_by` → `sort_by_key`, `PathBuf::from` → `Path::new`). No logic changes. Input validation, secret-masking, and SQL query construction are unchanged from `master`.

## Methodology

1. Sub-agent pass over the full diff across the categories in the standard security-review checklist (injection, path traversal, insecure download/supply-chain, auth, secrets, SQLi).
2. Direct verification of the two files with genuine new attack surface (`scripts/postinstall.js`, `release.yml`) by reading the actual source rather than relying on the sub-agent's summary alone.
3. No candidate findings existed to run through false-positive filtering.
