#!/usr/bin/env node
// rpi-deploy postinstall: builds the rpi binary from the bundled Rust sources.
// Only runs when installed as a package (under node_modules); a bare
// `npm install` in a source checkout is a no-op. Never runs apt/brew/winget and
// never checks Docker (that is `rpi agent setup`'s job). May auto-install
// rustup when cargo is missing.
'use strict';

const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const pkgDir = path.resolve(__dirname, '..');
const exe = process.platform === 'win32' ? '.exe' : '';
const cargoBinDir = path.join(os.homedir(), '.cargo', 'bin');

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
  let tmp;
  try {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'rpi-deploy-'));
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
    if (tmp) fs.rmSync(tmp, { recursive: true, force: true });
  }
}

function printNextSteps() {
  log('installed. Next steps:');
  log('  developer machine:  rpi setup   then, inside your project:  rpi init');
  log('  Raspberry Pi agent: sudo rpi agent setup   (Docker must already be installed)');
}

function log(msg) {
  console.log(`rpi-deploy: ${msg}`);
}

// True only when npm is installing this package into a node_modules tree
// (global install or as a dependency). In a source checkout the script lives at
// the repo root instead, where `npm install`/`npm ci` must NOT build — and must
// never delete the developer's target/ directory.
function isPackageInstall() {
  return /[\\/]node_modules[\\/]/.test(pkgDir);
}

function fail(msg) {
  console.error(`rpi-deploy: error: ${msg}`);
  process.exit(1);
}

function which(cmd) {
  const probe = process.platform === 'win32' ? 'where' : 'which';
  return spawnSync(probe, [cmd], { stdio: 'ignore' }).status === 0;
}

// Resolve cargo deterministically: a fresh rustup install is not in PATH yet.
function cargoCmd() {
  const local = path.join(cargoBinDir, `cargo${exe}`);
  return fs.existsSync(local) ? local : 'cargo';
}

function hasCargo() {
  return which('cargo') || fs.existsSync(path.join(cargoBinDir, `cargo${exe}`));
}

async function installRustup() {
  log('cargo not found; installing Rust via rustup (https://rustup.rs)...');
  if (process.platform === 'win32') {
    const arch = process.arch === 'arm64' ? 'aarch64' : 'x86_64';
    const url = `https://win.rustup.rs/${arch}`;
    const tmp = path.join(os.tmpdir(), 'rustup-init.exe');
    const res = await fetch(url);
    if (!res.ok) fail(`download ${url}: HTTP ${res.status}`);
    fs.writeFileSync(tmp, Buffer.from(await res.arrayBuffer()));
    const r = spawnSync(tmp, ['-y'], { stdio: 'inherit' });
    if (r.status !== 0) fail('rustup-init failed');
  } else {
    if (!which('curl')) {
      fail('curl is required to install rustup; install curl, then rerun: npm install -g rpi-deploy');
    }
    const r = spawnSync(
      'sh',
      ['-c', "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"],
      { stdio: 'inherit' }
    );
    if (r.status !== 0) fail('rustup install failed');
  }
  if (!hasCargo()) fail(`cargo still not found after rustup install (expected in ${cargoBinDir})`);
}

function checkCToolchain() {
  if (process.platform === 'win32') return; // cargo reports MSVC problems; hint printed on failure
  if (!which('cc')) {
    if (process.platform === 'darwin') {
      fail('a C compiler is required; install Xcode Command Line Tools: xcode-select --install');
    }
    fail('a C toolchain is required; on Debian/Raspberry Pi OS run: sudo apt-get install -y build-essential pkg-config, then rerun: npm install -g rpi-deploy');
  }
  if (process.platform === 'linux' && !which('pkg-config')) {
    fail('pkg-config is required; on Debian/Raspberry Pi OS run: sudo apt-get install -y build-essential pkg-config, then rerun: npm install -g rpi-deploy');
  }
}

async function main() {
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
  checkCToolchain();

  log('building rpi from source (cargo build --release --locked); this takes a few minutes (about 10 on a Raspberry Pi)...');
  const build = spawnSync(cargoCmd(), ['build', '--release', '--locked'], {
    cwd: pkgDir,
    stdio: 'inherit',
  });
  if (build.status !== 0) {
    if (process.platform === 'win32') {
      console.error('rpi-deploy: hint: building on Windows needs the Visual Studio Build Tools C++ workload.');
    }
    fail('cargo build failed (see output above)');
  }

  const built = path.join(pkgDir, 'target', 'release', `rpi${exe}`);
  const distDir = path.join(pkgDir, 'dist');
  fs.mkdirSync(distDir, { recursive: true });
  fs.copyFileSync(built, path.join(distDir, `rpi${exe}`));
  fs.chmodSync(path.join(distDir, `rpi${exe}`), 0o755);

  log('removing the build directory to save disk space...');
  fs.rmSync(path.join(pkgDir, 'target'), { recursive: true, force: true });

  printNextSteps();
}

module.exports = { targetTriple, assetName, parseSha256Sums, sha256File, downloadPrebuilt };

if (require.main === module) {
  main().catch((e) => fail(String(e && e.stack ? e.stack : e)));
}
