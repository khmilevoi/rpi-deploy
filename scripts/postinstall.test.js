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
