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
