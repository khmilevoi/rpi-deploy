import { spawn } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, '..', '..');

/** Pinned legacy agent for cross-version compat e2e (spec 2026-07-12):
 * the newest tag that predates the `features` handshake field, lacks
 * source-check (< 0.18.0), and still runs under this harness. */
export const LEGACY_TAG = 'v0.17.1';
export const LEGACY_TAR = path.join(HERE, '.legacy-src.tar');

/** `git archive` the pinned tag into the build context. Deterministic for a
 * given tag, so the docker layer that ADDs it stays cached. */
export function prepareLegacyTar({ tag = LEGACY_TAG, out = LEGACY_TAR, cwd = ROOT } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn('git', ['archive', '--format=tar', '-o', out, tag], {
      cwd,
      stdio: ['ignore', 'inherit', 'pipe'],
      windowsHide: true,
    });
    let stderr = '';
    child.stderr.on('data', (chunk) => { stderr += chunk; });
    child.on('error', reject);
    child.on('close', (code) => {
      if (code === 0) {
        resolve(out);
      } else {
        reject(new Error(
          `git archive ${tag} failed (exit ${code}): ${stderr.trim()} ` +
          `— shallow clone? fetch the tag: git fetch --no-tags origin +refs/tags/${tag}:refs/tags/${tag}`,
        ));
      }
    });
  });
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  prepareLegacyTar().then(
    (out) => console.log(`rpi e2e: legacy source ready: ${out} (${LEGACY_TAG})`),
    (error) => {
      console.error(`rpi e2e: ${error.message}`);
      process.exit(1);
    },
  );
}
