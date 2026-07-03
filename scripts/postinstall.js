#!/usr/bin/env node
// rpi-deploy postinstall: builds the rpi binary from the bundled Rust sources.
// Never runs apt/brew/winget and never checks Docker (that is `rpi agent
// setup`'s job). May auto-install rustup when cargo is missing.
'use strict';

const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const pkgDir = path.resolve(__dirname, '..');
const exe = process.platform === 'win32' ? '.exe' : '';
const cargoBinDir = path.join(os.homedir(), '.cargo', 'bin');

function log(msg) {
  console.log(`rpi-deploy: ${msg}`);
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

  log('installed. Next steps:');
  log('  developer machine:  rpi setup   then, inside your project:  rpi init');
  log('  Raspberry Pi agent: sudo rpi agent setup   (Docker must already be installed)');
}

main().catch((e) => fail(String(e && e.stack ? e.stack : e)));
