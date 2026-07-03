#!/usr/bin/env node
// Shim installed as the global `rpi` command: runs the native binary that
// scripts/postinstall.js built into dist/.
'use strict';

const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const exe = process.platform === 'win32' ? '.exe' : '';
const bin = path.join(__dirname, '..', 'dist', `rpi${exe}`);

if (!fs.existsSync(bin)) {
  console.error('rpi: binary not built; reinstall without --ignore-scripts: npm install -g rpi-deploy');
  process.exit(1);
}

const r = spawnSync(bin, process.argv.slice(2), { stdio: 'inherit' });
if (r.error) {
  console.error(`rpi: failed to run ${bin}: ${r.error.message}`);
  process.exit(1);
}
if (r.signal) {
  // POSIX convention: terminated by signal N -> exit code 128+N.
  process.exit(128 + (os.constants.signals[r.signal] || 0));
}
process.exit(r.status ?? 1);
