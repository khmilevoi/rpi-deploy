# Prebuilt Binaries (v0.7) — Design

Date: 2026-07-07
Status: approved

## Context

Since v0.6, `npm install -g rpi-deploy` builds the `rpi` binary from the
bundled Rust sources in `scripts/postinstall.js`. That takes about 10 minutes
on a Raspberry Pi and requires a Rust toolchain (plus MSVC Build Tools on
Windows). The repository has no CI at all; `docs/ci-github-actions.md` still
carries a stale note that binary releases "arrive in v0.5".

The repository (`github.com/khmilevoi/rpi-deploy`) is public, so GitHub's
ARM runners are free and release assets download without authentication.

## Goals

- Prebuilt `rpi` binaries for Windows x64, Linux aarch64 (the Pi), and
  Linux x64, built by GitHub Actions on a version tag and attached to a
  GitHub Release.
- `npm install -g rpi-deploy` downloads the matching binary in seconds
  instead of building from source; source build remains as fallback.
- Basic CI (tests, clippy, fmt) on pushes and pull requests.

## Non-Goals

- macOS binaries (macOS users keep the source-build fallback).
- 32-bit ARM (armv7) binaries.
- Standalone `install.sh` / `install.ps1` scripts (may come later; npm is
  the installation path today).

## Build Strategy (approved: Approach A)

Native runners, musl static linking for Linux — no cross-compilation:

| Runner | Target | Asset |
|---|---|---|
| `windows-latest` | `x86_64-pc-windows-msvc` | `rpi-vX.Y.Z-x86_64-pc-windows-msvc.zip` |
| `ubuntu-24.04-arm` | `aarch64-unknown-linux-musl` | `rpi-vX.Y.Z-aarch64-unknown-linux-musl.tar.gz` |
| `ubuntu-latest` | `x86_64-unknown-linux-musl` | `rpi-vX.Y.Z-x86_64-unknown-linux-musl.tar.gz` |

Musl produces fully static binaries that run on any glibc version
(Bookworm 2.36, Bullseye 2.31, Alpine). The dependency tree is
musl-compatible: rusqlite uses the `bundled` SQLite, reqwest uses rustls
(no OpenSSL), and ring/age/sysinfo build cleanly. Linux builds need only
`rustup target add <triple>` and the `musl-tools` apt package.

Rejected alternatives: glibc builds on older runner images (binary
portability tied to the runner image's glibc floor; breaks on Bullseye) and
cross-compilation via `cross`/`cargo-zigbuild` (extra tooling with no
benefit while ARM runners are free).

## Release Workflow — `.github/workflows/release.yml`

Trigger: push of a `v*` tag, plus `workflow_dispatch` as a dry run (builds
artifacts, skips the release and npm-publish jobs).

Jobs, in order:

1. **check** (`ubuntu-latest`): verify the tag version equals the version
   in `Cargo.toml` (`workspace.package.version`) and `package.json`; run
   `cargo test --locked`. On `workflow_dispatch` the tag comparison is
   skipped (no tag), the version-consistency check between the two files
   still runs.
2. **build** (matrix above): `rustup target add`, `musl-tools` on Linux,
   `cargo build --release --locked --target <triple>`, package the single
   binary into the archive (zip on Windows, tar.gz on Linux), upload as a
   workflow artifact.
3. **release** (tag only, needs build): download all artifacts, generate a
   single `SHA256SUMS` file covering the three archives, create the GitHub
   Release with `gh release create` attaching the archives and `SHA256SUMS`.
4. **npm-publish** (tag only, needs release): `npm publish` using the
   `NPM_TOKEN` repository secret. Ordering guarantees the release assets
   exist before the package version is visible on npm, so the very first
   installer's postinstall finds them.

## postinstall.js Changes

New flow when installed as a package (the `isPackageInstall()` guard and
the source-checkout no-op stay as they are):

1. Map `process.platform`/`process.arch` to a target triple:
   `win32`/`x64` → `x86_64-pc-windows-msvc`, `linux`/`arm64` →
   `aarch64-unknown-linux-musl`, `linux`/`x64` →
   `x86_64-unknown-linux-musl`. Any other combination (macOS, armv7, …)
   goes straight to the existing source-build path.
2. Download the archive from
   `https://github.com/khmilevoi/rpi-deploy/releases/download/v{version}/rpi-v{version}-{triple}.{ext}`
   and `SHA256SUMS` from the same release, using Node's global `fetch`
   (Node >= 18 is already required by `engines`). The version comes
   strictly from `package.json` — no "latest" resolution.
3. Verify the archive's sha256 against `SHA256SUMS` (`node:crypto`).
4. Extract with the system `tar -xf` (present on Linux; Windows 10+ ships
   bsdtar, which also reads zip). Place the binary at `dist/rpi{.exe}`,
   chmod 755.
5. On any failure — HTTP error, network/proxy failure, checksum mismatch,
   missing `tar` — print a clear warning and fall back to the existing
   source build. The fallback is safe: the bundled sources are protected
   by npm's own package integrity checks.
6. `RPI_DEPLOY_BUILD_FROM_SOURCE=1` forces the source-build path (escape
   hatch for unsupported environments or debugging).

`package.json` keeps shipping `crates/`, `Cargo.toml`, and `Cargo.lock`
for the fallback. `bin/rpi.js` is unchanged.

## CI Workflow — `.github/workflows/ci.yml`

Trigger: push to `master` and pull requests.

- **linux** (`ubuntu-latest`): `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test --locked`.
- **windows** (`windows-latest`): `cargo test --locked` — catches
  Windows-specific breakage between releases.

## Documentation Updates

- README: the install section states that npm downloads a prebuilt binary
  for Windows x64 / Linux x64 / Linux aarch64 and falls back to building
  from source elsewhere (macOS) or on download failure.
- `docs/ci-github-actions.md`: drop the stale "Binary releases +
  install.sh arrive in v0.5" note.

## Release Process (manual part)

1. One-time: add the `NPM_TOKEN` secret to the GitHub repository.
2. Per release: bump the version in `Cargo.toml` and `package.json`
   (and `Cargo.lock`), commit, `git tag vX.Y.Z`, `git push --tags`.
   CI does the rest.

## Testing

- Rust code is untouched; the existing test suite now runs in CI.
- The release pipeline is exercised with a `workflow_dispatch` dry run
  before the first tag.
- After the first real tag: verify `npm install -g rpi-deploy` on the
  Windows dev machine and on a Pi — the binary must arrive in seconds
  with no cargo involved.
- The fallback path is verified locally by running postinstall with
  `RPI_DEPLOY_BUILD_FROM_SOURCE=1`.
