#!/usr/bin/env node
// prepublishOnly guard: package.json version must equal the Cargo workspace
// version, so a published tarball always builds the matching Rust version.
'use strict';

const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const pkg = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
const cargo = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');

const m = cargo.match(/^\[workspace\.package\][^[]*?^version\s*=\s*"([^"]+)"/ms);
if (!m) {
  console.error('check-version: cannot find [workspace.package] version in Cargo.toml');
  process.exit(1);
}
if (m[1] !== pkg.version) {
  console.error(`check-version: package.json is ${pkg.version} but Cargo.toml workspace is ${m[1]}`);
  process.exit(1);
}
console.log(`check-version: ok (${pkg.version})`);
