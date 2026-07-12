import assert from 'node:assert/strict';
import { test } from 'node:test';
import path from 'node:path';
import { tmpdir } from 'node:os';
import { LEGACY_TAG, LEGACY_TAR, prepareLegacyTar } from './prepare-legacy.mjs';

test('legacy tag is a pinned release tag', () => {
  assert.match(LEGACY_TAG, /^v\d+\.\d+\.\d+$/);
});

test('tarball filename contract is stable', () => {
  assert.ok(LEGACY_TAR.endsWith('.legacy-src.tar'));
});

test('rejects with a fetch hint when the tag is missing', async () => {
  await assert.rejects(
    prepareLegacyTar({ tag: 'v999.999.999', out: path.join(tmpdir(), 'rpi-e2e-no.tar') }),
    /git archive v999\.999\.999 failed/,
  );
});
