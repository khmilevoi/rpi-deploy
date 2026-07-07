# Prebuilt Binaries (v0.7) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** GitHub Actions builds `rpi` binaries for Windows x64 / Linux x64 / Linux aarch64 on a `v*` tag, attaches them to a GitHub Release, publishes to npm; `postinstall.js` downloads the matching binary instead of building from source (source build stays as fallback).

**Architecture:** Two new workflows (`ci.yml` for tests on push/PR, `release.yml` for tag-triggered build→release→npm-publish) plus a download-and-verify path in `scripts/postinstall.js`. Linux binaries are musl-static (portable across glibc versions), built natively — no cross-compilation (ARM runners are free for this public repo).

**Tech Stack:** GitHub Actions (`windows-latest`, `ubuntu-latest`, `ubuntu-24.04-arm`, `dtolnay/rust-toolchain`, `Swatinem/rust-cache`), Node 18+ built-ins only (`fetch`, `node:crypto`, `node:test`), system `tar` for extraction.

**Spec:** `docs/superpowers/specs/2026-07-07-prebuilt-binaries-design.md`

## Global Constraints

- Repository: `khmilevoi/rpi-deploy` (public). Release download base URL: `https://github.com/khmilevoi/rpi-deploy/releases/download/v{version}/`.
- Asset names, exactly: `rpi-v{version}-x86_64-pc-windows-msvc.zip`, `rpi-v{version}-aarch64-unknown-linux-musl.tar.gz`, `rpi-v{version}-x86_64-unknown-linux-musl.tar.gz`, plus one `SHA256SUMS` file covering all three.
- Each archive contains exactly one file at its root: `rpi.exe` (zip) or `rpi` (tar.gz).
- No new npm dependencies and no new Rust dependencies. Node >= 18 only (global `fetch`, `node:test`).
- `package.json` `files` whitelist must keep `crates/`, `Cargo.toml`, `Cargo.lock` (source-build fallback) and must NOT gain the new test file (`files` lists `scripts/postinstall.js` and `scripts/check-version.js` explicitly, so this holds automatically — do not switch to `scripts/`).
- Env escape hatch, exact name: `RPI_DEPLOY_BUILD_FROM_SOURCE=1` forces the source build.
- Per project CLAUDE.md: prefix shell commands with `rtk` (e.g. `rtk cargo test`, `rtk git add`). `node ...` runs bare.
- Commit style follows repo history: `feat:`, `fix:`, `docs:`, `ci:`, `chore:` prefixes.
- All paths below are relative to the worktree root (`.claude/worktrees/feat-prebuilt-binaries` checkout of the repo).

---

### Task 1: Pure helpers in postinstall.js, with tests

`scripts/postinstall.js` currently runs `main()` unconditionally on require. Add pure helper functions (target-triple mapping, asset naming, SHA256SUMS parsing, file hashing), export them for tests, and guard `main()` behind `require.main === module`.

**Files:**
- Modify: `scripts/postinstall.js`
- Test (create): `scripts/postinstall.test.js`

**Interfaces:**
- Produces (consumed by Task 2 and by the test file):
  - `targetTriple(platform: string, arch: string): string | null` — `'win32','x64'` → `'x86_64-pc-windows-msvc'`; `'linux','arm64'` → `'aarch64-unknown-linux-musl'`; `'linux','x64'` → `'x86_64-unknown-linux-musl'`; anything else → `null`.
  - `assetName(version: string, triple: string): string` — e.g. `rpi-v0.7.0-x86_64-pc-windows-msvc.zip` (zip when triple contains `windows`, else tar.gz).
  - `parseSha256Sums(text: string): Record<string, string>` — filename → 64-hex hash, accepts `sha256sum` output (`<hash>  <name>` or `<hash> *<name>`).
  - `sha256File(file: string): Promise<string>` — lowercase hex sha256 of file contents.

- [ ] **Step 1: Write the failing test**

Create `scripts/postinstall.test.js` with exactly:

```js
'use strict';

const test = require('node:test');
const assert = require('node:assert');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const { targetTriple, assetName, parseSha256Sums, sha256File } = require('./postinstall.js');

test('targetTriple maps supported platforms', () => {
  assert.equal(targetTriple('win32', 'x64'), 'x86_64-pc-windows-msvc');
  assert.equal(targetTriple('linux', 'arm64'), 'aarch64-unknown-linux-musl');
  assert.equal(targetTriple('linux', 'x64'), 'x86_64-unknown-linux-musl');
});

test('targetTriple returns null for unsupported platforms', () => {
  assert.equal(targetTriple('darwin', 'arm64'), null);
  assert.equal(targetTriple('darwin', 'x64'), null);
  assert.equal(targetTriple('linux', 'arm'), null);
  assert.equal(targetTriple('win32', 'arm64'), null);
});

test('assetName picks zip on windows, tar.gz elsewhere', () => {
  assert.equal(
    assetName('0.7.0', 'x86_64-pc-windows-msvc'),
    'rpi-v0.7.0-x86_64-pc-windows-msvc.zip'
  );
  assert.equal(
    assetName('0.7.0', 'aarch64-unknown-linux-musl'),
    'rpi-v0.7.0-aarch64-unknown-linux-musl.tar.gz'
  );
  assert.equal(
    assetName('0.7.0', 'x86_64-unknown-linux-musl'),
    'rpi-v0.7.0-x86_64-unknown-linux-musl.tar.gz'
  );
});

test('parseSha256Sums parses sha256sum output', () => {
  const h1 = 'a'.repeat(64);
  const h2 = 'b'.repeat(64);
  const text = `${h1}  rpi-v0.7.0-x86_64-unknown-linux-musl.tar.gz\n${h2} *rpi-v0.7.0-x86_64-pc-windows-msvc.zip\n\nnot a sums line\n`;
  const sums = parseSha256Sums(text);
  assert.equal(sums['rpi-v0.7.0-x86_64-unknown-linux-musl.tar.gz'], h1);
  assert.equal(sums['rpi-v0.7.0-x86_64-pc-windows-msvc.zip'], h2);
  assert.equal(Object.keys(sums).length, 2);
});

test('sha256File hashes file contents', async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'rpi-test-'));
  try {
    const f = path.join(tmp, 'x');
    fs.writeFileSync(f, 'hello');
    assert.equal(
      await sha256File(f),
      '2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824'
    );
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test('requiring postinstall.js does not run main()', () => {
  // If main() had run on require, the log above ("source checkout detected...")
  // would have printed and, worse, a package-install path would have started a
  // build. The require at the top of this file succeeding without side effects
  // is the assertion; here we just sanity-check the exports exist.
  assert.equal(typeof targetTriple, 'function');
  assert.equal(typeof assetName, 'function');
  assert.equal(typeof parseSha256Sums, 'function');
  assert.equal(typeof sha256File, 'function');
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `node --test scripts/`
Expected: FAIL — `targetTriple is not a function` (or destructuring of `undefined`), because `postinstall.js` exports nothing yet. Note: the require will also print `rpi-deploy: source checkout detected...` because `main()` currently runs on require — that disappears in Step 3.

- [ ] **Step 3: Implement the helpers and the require guard**

In `scripts/postinstall.js`:

3a. After the `const cargoBinDir = ...` line (line 16), insert:

```js
const REPO = 'khmilevoi/rpi-deploy';

// Rust target triples with prebuilt binaries on GitHub Releases, keyed by
// `${process.platform}-${process.arch}`. Anything else builds from source.
const TARGET_TRIPLES = {
  'win32-x64': 'x86_64-pc-windows-msvc',
  'linux-x64': 'x86_64-unknown-linux-musl',
  'linux-arm64': 'aarch64-unknown-linux-musl',
};

function targetTriple(platform, arch) {
  return TARGET_TRIPLES[`${platform}-${arch}`] || null;
}

function assetName(version, triple) {
  const ext = triple.includes('windows') ? 'zip' : 'tar.gz';
  return `rpi-v${version}-${triple}.${ext}`;
}

// Accepts `sha256sum` output: "<hash>  <name>" (text) or "<hash> *<name>" (binary).
function parseSha256Sums(text) {
  const sums = {};
  for (const line of text.split('\n')) {
    const m = /^([0-9a-f]{64})[ *]+(.+)$/.exec(line.trim());
    if (m) sums[m[2]] = m[1];
  }
  return sums;
}

async function sha256File(file) {
  const crypto = require('node:crypto');
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}
```

3b. Replace the last line of the file:

```js
main().catch((e) => fail(String(e && e.stack ? e.stack : e)));
```

with:

```js
module.exports = { targetTriple, assetName, parseSha256Sums, sha256File };

if (require.main === module) {
  main().catch((e) => fail(String(e && e.stack ? e.stack : e)));
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `node --test scripts/`
Expected: PASS, 6 tests, and no `rpi-deploy: ...` log output during the run.

Also run: `node scripts/postinstall.js`
Expected: still prints `rpi-deploy: source checkout detected (not installed under node_modules); skipping build and leaving target/ untouched. ...` and exits 0 (the CLI entry path still works).

- [ ] **Step 5: Commit**

```bash
rtk git add scripts/postinstall.js scripts/postinstall.test.js
rtk git commit -m "feat: add prebuilt-binary helpers to postinstall

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Download-and-verify flow in postinstall.js

Wire a `downloadPrebuilt(version)` function into `main()`: try the prebuilt binary first, fall back to the existing source build on any failure. Extract the duplicated "Next steps" log into a helper.

**Files:**
- Modify: `scripts/postinstall.js`

**Interfaces:**
- Consumes (from Task 1): `targetTriple`, `assetName`, `parseSha256Sums`, `sha256File`, `REPO`.
- Produces: `downloadPrebuilt(version: string): Promise<boolean>` — `true` = binary installed into `dist/`, `false` = caller must build from source (reason already logged). Exported for smoke testing.

- [ ] **Step 1: Implement downloadPrebuilt and printNextSteps**

In `scripts/postinstall.js`, insert after the `sha256File` function from Task 1:

```js
async function fetchTo(url, dest) {
  const res = await fetch(url, { redirect: 'follow' });
  if (!res.ok) throw new Error(`download ${url}: HTTP ${res.status}`);
  fs.writeFileSync(dest, Buffer.from(await res.arrayBuffer()));
}

// Try to install a prebuilt binary from the GitHub release matching this
// package version exactly (no "latest" resolution). Returns false — after
// logging why — whenever the caller should fall back to the source build.
// The fallback is safe: the bundled sources are integrity-checked by npm.
async function downloadPrebuilt(version) {
  const triple = targetTriple(process.platform, process.arch);
  if (!triple) {
    log(`no prebuilt binary for ${process.platform}/${process.arch}; building from source`);
    return false;
  }
  const asset = assetName(version, triple);
  const base = `https://github.com/${REPO}/releases/download/v${version}`;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'rpi-deploy-'));
  try {
    log(`downloading prebuilt binary ${asset}...`);
    const archive = path.join(tmp, asset);
    await fetchTo(`${base}/${asset}`, archive);

    const sumsRes = await fetch(`${base}/SHA256SUMS`, { redirect: 'follow' });
    if (!sumsRes.ok) throw new Error(`download ${base}/SHA256SUMS: HTTP ${sumsRes.status}`);
    const expected = parseSha256Sums(await sumsRes.text())[asset];
    if (!expected) throw new Error(`${asset} not listed in SHA256SUMS`);
    const actual = await sha256File(archive);
    if (actual !== expected) {
      throw new Error(`sha256 mismatch for ${asset}: expected ${expected}, got ${actual}`);
    }

    // bsdtar ships with Windows 10+ and reads zip; GNU tar handles tar.gz.
    const r = spawnSync('tar', ['-xf', archive, '-C', tmp], { stdio: 'inherit' });
    if (r.error || r.status !== 0) throw new Error('tar extraction failed');
    const bin = path.join(tmp, `rpi${exe}`);
    if (!fs.existsSync(bin)) throw new Error(`archive did not contain rpi${exe}`);

    const distDir = path.join(pkgDir, 'dist');
    fs.mkdirSync(distDir, { recursive: true });
    fs.copyFileSync(bin, path.join(distDir, `rpi${exe}`));
    fs.chmodSync(path.join(distDir, `rpi${exe}`), 0o755);
    log('prebuilt binary installed.');
    return true;
  } catch (e) {
    log(`warning: prebuilt binary unavailable (${e && e.message ? e.message : e}); falling back to source build`);
    return false;
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
}

function printNextSteps() {
  log('installed. Next steps:');
  log('  developer machine:  rpi setup   then, inside your project:  rpi init');
  log('  Raspberry Pi agent: sudo rpi agent setup   (Docker must already be installed)');
}
```

- [ ] **Step 2: Wire it into main() and dedupe the outro**

2a. In `main()`, replace:

```js
  if (!isPackageInstall()) {
    log('source checkout detected (not installed under node_modules); skipping build and leaving target/ untouched. Build directly with `cargo build --release`.');
    return;
  }

  if (!hasCargo()) await installRustup();
```

with:

```js
  if (!isPackageInstall()) {
    log('source checkout detected (not installed under node_modules); skipping build and leaving target/ untouched. Build directly with `cargo build --release`.');
    return;
  }

  const version = JSON.parse(fs.readFileSync(path.join(pkgDir, 'package.json'), 'utf8')).version;
  if (process.env.RPI_DEPLOY_BUILD_FROM_SOURCE === '1') {
    log('RPI_DEPLOY_BUILD_FROM_SOURCE=1 set; building from source');
  } else if (await downloadPrebuilt(version)) {
    printNextSteps();
    return;
  }

  if (!hasCargo()) await installRustup();
```

2b. At the end of `main()`, replace the trailing log block:

```js
  log('installed. Next steps:');
  log('  developer machine:  rpi setup   then, inside your project:  rpi init');
  log('  Raspberry Pi agent: sudo rpi agent setup   (Docker must already be installed)');
```

with:

```js
  printNextSteps();
```

2c. Extend the exports line from Task 1 to:

```js
module.exports = { targetTriple, assetName, parseSha256Sums, sha256File, downloadPrebuilt };
```

- [ ] **Step 3: Run the tests and a live fallback smoke**

Run: `node --test scripts/`
Expected: PASS (6 tests, unchanged).

Run (live smoke of the failure path — release v0.6.0 has no binary assets, so this must log a warning and return false without touching `dist/`):

```bash
node -e "require('./scripts/postinstall.js').downloadPrebuilt('0.6.0').then(r => { console.log('returned:', r); process.exit(r ? 1 : 0); })"
```

Expected output: `rpi-deploy: downloading prebuilt binary rpi-v0.6.0-...`, then `rpi-deploy: warning: prebuilt binary unavailable (download ...: HTTP 404); falling back to source build`, then `returned: false`, exit 0. Verify no `dist/` directory was created: `rtk ls dist` should report it does not exist.

Run: `node scripts/postinstall.js`
Expected: unchanged source-checkout skip message, exit 0.

- [ ] **Step 4: Commit**

```bash
rtk git add scripts/postinstall.js
rtk git commit -m "feat: download prebuilt binary in postinstall, fall back to source build

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: CI workflow (ci.yml)

Basic CI on push/PR. First make sure the checks it will run pass locally, so CI is not born red.

**Files:**
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `node --test scripts/` from Tasks 1–2.
- Produces: repo-level CI gate; no code interfaces.

- [ ] **Step 1: Verify the checks pass locally (Windows dev machine)**

Run, one at a time:

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
node --test scripts/
```

Expected: all pass. If `fmt` fails: run `rtk cargo fmt --all`, re-run the check, and include the formatting in this task's commit. If `clippy` fails with non-trivial lints (anything beyond an obvious mechanical fix), STOP and report to the user instead of "fixing" behavior.

- [ ] **Step 2: Write the workflow**

Create `.github/workflows/ci.yml` with exactly:

```yaml
name: ci

on:
  push:
    branches: [master]
  pull_request:

permissions:
  contents: read

jobs:
  linux:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all -- --check
      - run: cargo clippy --all-targets --locked -- -D warnings
      - run: cargo test --locked
      - run: node --test scripts/

  windows:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --locked
      - run: node --test scripts/
```

- [ ] **Step 3: Validate the YAML parses**

Run: `npx --yes js-yaml .github/workflows/ci.yml`
Expected: prints the parsed JSON, no error. (Full behavior is verified on GitHub when the PR opens — see Task 7.)

- [ ] **Step 4: Commit**

```bash
rtk git add .github/workflows/ci.yml
rtk git commit -m "ci: add fmt/clippy/test workflow for linux and windows

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: Release workflow (release.yml)

Tag-triggered pipeline: check → build (3-target matrix) → GitHub Release → npm publish. `workflow_dispatch` runs check+build only (dry run).

**Files:**
- Create: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `scripts/check-version.js` (existing; exits non-zero when `package.json` and `Cargo.toml` workspace versions differ). Asset names per Global Constraints — `postinstall.js` (Task 2) downloads exactly these names.
- Produces: GitHub Release `v{version}` with 3 archives + `SHA256SUMS`; npm package `rpi-deploy@{version}`.

- [ ] **Step 1: Write the workflow**

Create `.github/workflows/release.yml` with exactly:

```yaml
name: release

on:
  push:
    tags: ['v*']
  workflow_dispatch: # dry run: builds artifacts, skips release and npm publish

permissions:
  contents: read

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: package.json and Cargo.toml versions match
        run: node scripts/check-version.js
      - name: tag matches package version
        if: startsWith(github.ref, 'refs/tags/')
        run: |
          PKG="v$(node -p "require('./package.json').version")"
          if [ "$PKG" != "$GITHUB_REF_NAME" ]; then
            echo "tag $GITHUB_REF_NAME does not match package.json version $PKG" >&2
            exit 1
          fi
      - run: cargo test --locked

  build:
    needs: check
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: windows-latest
            target: x86_64-pc-windows-msvc
          - os: ubuntu-24.04-arm
            target: aarch64-unknown-linux-musl
          - os: ubuntu-latest
            target: x86_64-unknown-linux-musl
    runs-on: ${{ matrix.os }}
    env:
      CC_aarch64_unknown_linux_musl: musl-gcc
      CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER: musl-gcc
      CC_x86_64_unknown_linux_musl: musl-gcc
      CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER: musl-gcc
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - name: install musl-tools
        if: runner.os == 'Linux'
        run: sudo apt-get update && sudo apt-get install -y musl-tools
      - run: cargo build --release --locked --target ${{ matrix.target }}
      - name: package (linux)
        if: runner.os == 'Linux'
        run: |
          VERSION=$(node -p "require('./package.json').version")
          tar -C "target/${{ matrix.target }}/release" -czf "rpi-v${VERSION}-${{ matrix.target }}.tar.gz" rpi
      - name: package (windows)
        if: runner.os == 'Windows'
        shell: pwsh
        run: |
          $version = node -p "require('./package.json').version"
          Compress-Archive -Path "target/${{ matrix.target }}/release/rpi.exe" -DestinationPath "rpi-v$version-${{ matrix.target }}.zip"
      - uses: actions/upload-artifact@v4
        with:
          name: rpi-${{ matrix.target }}
          path: |
            rpi-v*.tar.gz
            rpi-v*.zip
          if-no-files-found: error

  release:
    if: startsWith(github.ref, 'refs/tags/')
    needs: build
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/download-artifact@v4
        with:
          path: assets
          merge-multiple: true
      - name: generate SHA256SUMS
        run: cd assets && sha256sum rpi-v* > SHA256SUMS
      - name: create GitHub release
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          gh release create "$GITHUB_REF_NAME" --repo "$GITHUB_REPOSITORY" \
            --title "$GITHUB_REF_NAME" --generate-notes assets/*

  npm-publish:
    if: startsWith(github.ref, 'refs/tags/')
    needs: release
    runs-on: ubuntu-latest
    permissions:
      contents: read
      id-token: write # npm --provenance
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 20
          registry-url: https://registry.npmjs.org
      - run: npm publish --provenance --access public
        env:
          NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}
```

Why these details (do not "simplify" them away):
- `CC_*`/`CARGO_TARGET_*_LINKER: musl-gcc` — `musl-tools` provides `musl-gcc` for the host arch; the `cc` crate (ring, bundled sqlite3) and the final link both need it for `*-linux-musl` targets. Musl targets are `+crt-static` by default → fully static binaries.
- `ubuntu-24.04-arm` builds aarch64 natively (free for public repos) — no QEMU/cross.
- `npm publish` ordering: the job needs `release` so binaries exist before the version is visible on npm; `prepublishOnly` re-runs `check-version.js` as a final guard.
- `workflow_dispatch` runs only `check`+`build` because `release`/`npm-publish` are gated on `startsWith(github.ref, 'refs/tags/')`.

- [ ] **Step 2: Validate the YAML parses**

Run: `npx --yes js-yaml .github/workflows/release.yml`
Expected: parsed JSON, no error.

- [ ] **Step 3: Cross-check asset names against postinstall**

Run: `node -e "const p=require('./scripts/postinstall.js'); console.log(p.assetName('9.9.9','x86_64-pc-windows-msvc')); console.log(p.assetName('9.9.9','aarch64-unknown-linux-musl')); console.log(p.assetName('9.9.9','x86_64-unknown-linux-musl'));"`
Expected output, exactly matching what the workflow's package steps produce for version 9.9.9:

```
rpi-v9.9.9-x86_64-pc-windows-msvc.zip
rpi-v9.9.9-aarch64-unknown-linux-musl.tar.gz
rpi-v9.9.9-x86_64-unknown-linux-musl.tar.gz
```

- [ ] **Step 4: Commit**

```bash
rtk git add .github/workflows/release.yml
rtk git commit -m "ci: add tag-triggered release workflow with prebuilt binaries and npm publish

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Documentation updates

**Files:**
- Modify: `README.md` (status paragraph ~lines 9–16, npm-install comment ~line 187, "ships the Rust sources" paragraph ~lines 234–240)
- Modify: `docs/ci-github-actions.md` (~lines 41–44, stale "Install pi CLI" step)

**Interfaces:** none (prose only).

- [ ] **Step 1: Update README status paragraph**

Replace (README.md lines 9–16):

```markdown
Status: v0.6 (npm install) — everything from v0.1–v0.5 (deploy/env/ingress/CI,
`rpi logs`, `rpi stats`, `rpi start|stop|restart`, `rpi rm`, `rpi status`,
`rpi doctor`, `rpi agent status|logs`, one-command setup) plus
`npm install -g rpi-deploy` for both roles: the CLI command is now `rpi`, the
package builds it from source on install, and `sudo rpi agent setup` installs
the running binary to `/usr/local/bin/rpi` and restarts the agent on updates.
Manual install from source remains as a fallback (see "Build And Install The
Binary" below).
```

with:

```markdown
Status: v0.7 (prebuilt binaries) — everything from v0.1–v0.6 (deploy/env/
ingress/CI, `rpi logs`, `rpi stats`, `rpi start|stop|restart`, `rpi rm`,
`rpi status`, `rpi doctor`, `rpi agent status|logs`, one-command setup,
`npm install -g rpi-deploy` for both roles) plus prebuilt binaries:
GitHub Actions builds `rpi` for Windows x64, Linux x64, and Linux aarch64 on
every release tag, and `npm install` downloads the matching binary in seconds
instead of compiling for ~10 minutes. Building from source remains the
fallback everywhere else (see "Build And Install The Binary" below).
```

- [ ] **Step 2: Update the Pi install comment**

Replace (README.md line 187):

```bash
sudo npm install -g rpi-deploy    # builds from source, ~10 minutes on a Pi
```

with:

```bash
sudo npm install -g rpi-deploy    # downloads a prebuilt arm64 binary, seconds
```

- [ ] **Step 3: Replace the "ships the Rust sources" paragraph**

Replace (README.md lines 234–240):

```markdown
The npm package ships the Rust sources and builds them on install
(`cargo build --release --locked`); rustup is installed automatically when
cargo is missing, and the build directory is removed afterwards to save disk
space. Building on Windows needs the Visual Studio Build Tools C++ workload.
Installing with `--ignore-scripts` leaves the CLI unusable (`rpi` will report
that the binary was not built) — as does npm's `allow-scripts` gate on recent
versions, see above.
```

with:

```markdown
On install the package downloads a prebuilt binary from the matching GitHub
Release (Windows x64, Linux x64, Linux aarch64) and verifies its SHA-256
checksum. On other platforms (macOS, 32-bit ARM), or when the download fails
(offline, proxy, checksum mismatch), it falls back to building the bundled
Rust sources (`cargo build --release --locked`); rustup is installed
automatically when cargo is missing, and the build directory is removed
afterwards to save disk space. Building on Windows needs the Visual Studio
Build Tools C++ workload. Set `RPI_DEPLOY_BUILD_FROM_SOURCE=1` to skip the
download and force the source build. Installing with `--ignore-scripts`
leaves the CLI unusable (`rpi` will report that the binary was not built) —
as does npm's `allow-scripts` gate on recent versions, see above.
```

- [ ] **Step 4: Fix the stale CI doc**

Replace (docs/ci-github-actions.md lines 41–44):

```yaml
      - name: Install pi CLI
        # Binary releases + install.sh arrive in v0.5; for now, install from source.
        # Speed-up: use actions/cache for ~/.cargo/bin keyed by the tool revision hash.
        run: cargo install --git https://github.com/khmilevoi/pi --locked pi
```

with:

```yaml
      - name: Install rpi CLI
        # postinstall downloads the prebuilt x86_64 binary from GitHub Releases.
        run: npm install -g rpi-deploy
```

- [ ] **Step 5: Commit**

```bash
rtk git add README.md docs/ci-github-actions.md
rtk git commit -m "docs: document prebuilt binary install and refresh CI example

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: Version bump to 0.7.0

**Files:**
- Modify: `Cargo.toml` (workspace version), `package.json` (version), `Cargo.lock` (via cargo)

**Interfaces:**
- Produces: version `0.7.0` everywhere — the value the release tag `v0.7.0` will be checked against by `release.yml`'s check job.

- [ ] **Step 1: Bump versions**

In `Cargo.toml`, under `[workspace.package]`, change `version = "0.6.0"` to `version = "0.7.0"`.
In `package.json`, change `"version": "0.6.0"` to `"version": "0.7.0"`.

- [ ] **Step 2: Sync Cargo.lock and verify consistency**

Run: `rtk cargo update --workspace`
Expected: only workspace members (`pi`, `pi-domain`, `pi-application`, `pi-infrastructure`) change version in `Cargo.lock`; no external dependency updates.

Run: `node scripts/check-version.js`
Expected: `check-version: ok (0.7.0)`

Run: `rtk cargo check --locked`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
rtk git add Cargo.toml Cargo.lock package.json
rtk git commit -m "chore: bump version to 0.7.0

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: Final verification and release handoff

**Files:** none created; verification + instructions only.

- [ ] **Step 1: Full local check**

Run, one at a time; all must pass:

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
node --test scripts/
node scripts/postinstall.js
```

The last command must print the source-checkout skip message and leave `target/` untouched.

- [ ] **Step 2: Review the branch**

Run: `rtk git log --oneline master..HEAD` and `rtk git diff master --stat`
Expected: the commits from Tasks 1–6 plus the spec/plan docs; no stray files (no `dist/`, no `node_modules/`, no `target/` changes).

- [ ] **Step 3: Hand off with the post-merge release checklist**

The following can only happen on GitHub after this branch is pushed/merged — report it to the user verbatim as their checklist, do not attempt the tag yourself:

1. Push the branch and open a PR → `ci.yml` runs for the first time (linux + windows jobs must pass).
2. Merge to `master`.
3. One-time: add the `NPM_TOKEN` secret (npm automation token for `rpi-deploy`) in repo Settings → Secrets → Actions.
4. Dry run: `rtk gh workflow run release.yml --ref master`, then `rtk gh run watch` — check and all three build jobs must succeed and upload 3 artifacts.
5. Release: `rtk git tag v0.7.0 && rtk git push origin v0.7.0` — the full pipeline runs: GitHub Release `v0.7.0` appears with 3 archives + `SHA256SUMS`, then `rpi-deploy@0.7.0` appears on npm.
6. Verify installs: `npm install -g rpi-deploy@0.7.0` on the Windows machine (binary in seconds, no cargo) and on the Pi (`sudo npm install -g rpi-deploy@0.7.0`); on both run `rpi --help`. Fallback check: reinstall with `RPI_DEPLOY_BUILD_FROM_SOURCE=1` set to confirm the source path still works.
