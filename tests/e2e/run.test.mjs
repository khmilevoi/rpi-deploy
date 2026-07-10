import test from 'node:test';
import assert from 'node:assert/strict';
import os from 'node:os';
import path from 'node:path';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';

import {
  discoverScenarios,
  formatSummary,
  makeProjectName,
  parseComposeVersion,
  parseMetaEnv,
  parseRunArgs,
  runDev,
  runE2E,
  runPool,
  scenarioTimeoutMs,
} from './run.mjs';

const ok = (stdout = '') => ({ code: 0, stdout, stderr: '', timedOut: false });

function fakeRunner(responses) {
  const calls = [];
  const runner = async (args, options = {}) => {
    calls.push({ args, options });
    return responses.length ? responses.shift() : ok();
  };
  return { calls, runner };
}

async function withArtifacts(fn) {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'rpi-e2e-runner-'));
  try {
    await fn(dir);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

test('parseComposeVersion accepts v2 output and rejects malformed text', () => {
  assert.deepEqual(parseComposeVersion('Docker Compose version v2.33.1'), [2, 33, 1]);
  assert.deepEqual(parseComposeVersion('2.40.0\n'), [2, 40, 0]);
  assert.deepEqual(parseComposeVersion('v2.40.0-desktop.1'), [2, 40, 0]);
  assert.throws(() => parseComposeVersion('compose unknown'), /cannot parse/i);
});

test('makeProjectName is deterministic with injected values', () => {
  assert.equal(
    makeProjectName({ pid: 42, now: 123456, suffix: 'abc123' }),
    'rpi-e2e-42-2n9c-abc123',
  );
});

test('local success builds, starts dependencies, runs client, then tears down', async () => {
  await withArtifacts(async (artifactDir) => {
    const { calls, runner } = fakeRunner([
      ok('2.33.1'), // version
      ok(),         // config
      ok(),         // build client image
      ok(),         // up --wait dependencies
      ok(),         // run client
      ok(),         // down
      ok(),         // image rm
    ]);
    const code = await runE2E({
      runner,
      artifactDir,
      projectName: 'rpi-e2e-test',
      env: {},
    });
    assert.equal(code, 0);
    const commands = calls.map((call) => call.args.join(' '));
    assert.match(commands[0], /^compose version --short$/);
    assert.match(commands[1], /--file tests[/\\]e2e[/\\]base\.compose\.yaml config --quiet$/);
    assert.match(commands[2], /--file tests[/\\]e2e[/\\]base\.compose\.yaml build client$/);
    assert.match(commands[3], /up -d --no-build --wait --wait-timeout 120 dind target git-fixture$/);
    assert.match(commands[4], /run --rm --no-deps client$/);
    assert.match(commands[5], /down -v --remove-orphans$/);
    assert.match(commands[6], /image rm rpi-e2e-runtime:rpi-e2e-test$/);
  });
});

test('client failure collects diagnostics and keeps the client exit code', async () => {
  await withArtifacts(async (artifactDir) => {
    const { calls, runner } = fakeRunner([
      ok('2.33.1'),
      ok(),
      ok(),
      ok(),
      { code: 23, stdout: '', stderr: 'deploy failed', timedOut: false },
      ok(), // ps
      ok(), // outer logs
      ok(), // nested diagnostics
      ok(), // down
      ok(), // image rm
    ]);
    const code = await runE2E({
      runner,
      artifactDir,
      projectName: 'rpi-e2e-test',
      env: {},
    });
    assert.equal(code, 23);
    const joined = calls.map((call) => call.args.join(' ')).join('\n');
    assert.match(joined, /ps --all/);
    assert.match(joined, /logs --no-color --timestamps/);
    assert.match(joined, /exec -T target bash -lc/);
    assert.match(joined, /down -v --remove-orphans/);
    const diagnosticIndex = calls.findIndex((call) => call.args.includes('ps'));
    const downIndex = calls.findIndex((call) => call.args.includes('down'));
    assert.ok(diagnosticIndex >= 0 && diagnosticIndex < downIndex);
  });
});

test('cleanup failure fails a successful scenario but cannot mask a scenario failure', async () => {
  await withArtifacts(async (artifactDir) => {
    const successCleanupFailure = fakeRunner([
      ok('2.33.1'), ok(), ok(), ok(), ok(),
      { code: 9, stdout: '', stderr: 'down failed', timedOut: false },
      ok(),
    ]);
    assert.equal(await runE2E({
      runner: successCleanupFailure.runner,
      artifactDir,
      projectName: 'rpi-e2e-a',
      env: {},
    }), 9);

    const primaryFailure = fakeRunner([
      ok('2.33.1'), ok(), ok(), ok(),
      { code: 17, stdout: '', stderr: '', timedOut: false },
      ok(), ok(), ok(),
      { code: 9, stdout: '', stderr: 'down failed', timedOut: false },
      ok(),
    ]);
    assert.equal(await runE2E({
      runner: primaryFailure.runner,
      artifactDir,
      projectName: 'rpi-e2e-b',
      env: {},
    }), 17);
  });
});

test('prebuilt mode skips local build and image removal', async () => {
  await withArtifacts(async (artifactDir) => {
    const { calls, runner } = fakeRunner([ok('2.40.0'), ok(), ok(), ok(), ok()]);
    assert.equal(await runE2E({
      runner,
      artifactDir,
      projectName: 'rpi-e2e-ci',
      env: {
        RPI_E2E_PREBUILT: '1',
        RPI_E2E_RUNTIME_IMAGE: 'rpi-e2e-runtime:ci',
      },
    }), 0);
    const joined = calls.map((call) => call.args.join(' ')).join('\n');
    assert.doesNotMatch(joined, / build client/);
    assert.doesNotMatch(joined, /image rm/);
  });
});

test('Compose older than 2.33.1 fails before any service starts', async () => {
  await withArtifacts(async (artifactDir) => {
    const { calls, runner } = fakeRunner([ok('2.32.4')]);
    assert.equal(await runE2E({
      runner,
      artifactDir,
      projectName: 'rpi-e2e-old',
      env: {},
    }), 1);
    assert.equal(calls.length, 1);
  });
});

test('a timed-out client returns 124 and tears down exactly once', async () => {
  await withArtifacts(async (artifactDir) => {
    const timeout = { code: 143, stdout: '', stderr: '', timedOut: true };
    const { calls, runner } = fakeRunner([
      ok('2.33.1'), ok(), ok(), ok(), timeout,
      ok(), ok(), ok(), // diagnostics
      ok(), ok(),       // down, image rm
    ]);
    assert.equal(await runE2E({
      runner,
      artifactDir,
      projectName: 'rpi-e2e-timeout',
      env: {},
    }), 124);
    assert.equal(calls.filter((call) => call.args.includes('down')).length, 1);
  });
});

test('an already-aborted run exits 130 without touching Docker', async () => {
  await withArtifacts(async (artifactDir) => {
    const controller = new AbortController();
    controller.abort();
    const { calls, runner } = fakeRunner([]);
    assert.equal(await runE2E({
      runner,
      artifactDir,
      projectName: 'rpi-e2e-aborted',
      env: {},
      signal: controller.signal,
    }), 130);
    assert.equal(calls.length, 0);
  });
});

test('RPI_E2E_KEEP=1 skips teardown and image removal', async () => {
  await withArtifacts(async (artifactDir) => {
    const { calls, runner } = fakeRunner([
      ok('2.33.1'), ok(), ok(), ok(),
      { code: 23, stdout: '', stderr: '', timedOut: false },
      ok(), ok(), ok(), // diagnostics
    ]);
    assert.equal(await runE2E({
      runner,
      artifactDir,
      projectName: 'rpi-e2e-keep',
      env: { RPI_E2E_KEEP: '1' },
    }), 23);
    const joined = calls.map((call) => call.args.join(' ')).join('\n');
    assert.doesNotMatch(joined, /down -v/);
    assert.doesNotMatch(joined, /image rm/);
  });
});

test('dev up builds the shared image once and waits for client-dev', async () => {
  await withArtifacts(async (artifactDir) => {
    const { calls, runner } = fakeRunner([ok(), ok()]);
    assert.equal(await runDev('up', { runner, artifactDir, env: {} }), 0);
    const commands = calls.map((call) => call.args.join(' '));
    assert.match(commands[0], /--project-name rpi-e2e-dev/);
    assert.match(commands[0], /--profile dev build client$/);
    assert.match(
      commands[1],
      /up -d --no-build --wait --wait-timeout 120 dind target git-fixture client-dev$/,
    );
  });
});

test('dev down tears down the fixed dev project and removes its image', async () => {
  await withArtifacts(async (artifactDir) => {
    const { calls, runner } = fakeRunner([ok(), ok()]);
    assert.equal(await runDev('down', { runner, artifactDir, env: {} }), 0);
    const joined = calls.map((call) => call.args.join(' ')).join('\n');
    assert.match(joined, /--project-name rpi-e2e-dev/);
    assert.match(joined, /down -v --remove-orphans/);
    assert.match(joined, /image rm rpi-e2e-runtime:rpi-e2e-dev/);
  });
});

test('discoverScenarios lists folders that contain scenario.sh, sorted', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'rpi-e2e-scenarios-'));
  try {
    await mkdir(path.join(dir, 'zeta'), { recursive: true });
    await writeFile(path.join(dir, 'zeta', 'scenario.sh'), '#!/usr/bin/env bash\n');
    await mkdir(path.join(dir, 'alpha'), { recursive: true });
    await writeFile(path.join(dir, 'alpha', 'scenario.sh'), '#!/usr/bin/env bash\n');
    await mkdir(path.join(dir, 'not-a-scenario'), { recursive: true });
    await writeFile(path.join(dir, 'stray-file'), 'x');
    assert.deepEqual(await discoverScenarios(dir), ['alpha', 'zeta']);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('discoverScenarios returns empty for a missing directory', async () => {
  assert.deepEqual(
    await discoverScenarios(path.join(os.tmpdir(), 'rpi-e2e-none-such')),
    [],
  );
});

test('runPool caps concurrency and preserves result order', async () => {
  let active = 0;
  let peak = 0;
  const results = await runPool([10, 20, 30, 40, 50], 2, async (item) => {
    active += 1;
    peak = Math.max(peak, active);
    await new Promise((resolve) => setTimeout(resolve, 5));
    active -= 1;
    return item + 1;
  });
  assert.deepEqual(results, [11, 21, 31, 41, 51]);
  assert.equal(peak, 2);
});

test('runPool stops dispatching after stopOn matches, leaving the rest undefined', async () => {
  const seen = [];
  const results = await runPool(
    ['a', 'b', 'c', 'd'],
    1,
    async (item) => {
      seen.push(item);
      return { item, code: item === 'b' ? 1 : 0 };
    },
    { stopOn: (result) => result.code !== 0 },
  );
  assert.deepEqual(seen, ['a', 'b']);
  assert.equal(results[2], undefined);
  assert.equal(results[3], undefined);
});

test('formatSummary renders pass/fail/skip lines and a totals header', () => {
  const text = formatSummary([
    { scenario: 'happy-path', code: 0, durationMs: 61_000 },
    { scenario: 'rm-root-owned', code: 23, durationMs: 5_000, timedOut: false },
    { scenario: 'slowpoke', code: 124, durationMs: 900_000, timedOut: true },
    { scenario: 'never-ran', skipped: true },
  ]);
  assert.match(text, /^rpi e2e: 1\/4 scenarios passed, 1 skipped$/m);
  assert.match(text, /happy-path\s+PASS 61s/);
  assert.match(text, /rm-root-owned\s+FAIL \(exit 23\) 5s/);
  assert.match(text, /slowpoke\s+FAIL \(timeout\) 900s/);
  assert.match(text, /never-ran\s+SKIP$/m);
});

test('parseMetaEnv reads KEY=VALUE lines and ignores comments and blanks', () => {
  assert.deepEqual(parseMetaEnv('# note\nRPI_E2E_TIMEOUT=1200\n\nX = y\n'), {
    RPI_E2E_TIMEOUT: '1200',
    X: 'y',
  });
});

test('scenarioTimeoutMs honors meta.env and falls back to the default', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'rpi-e2e-meta-'));
  try {
    await mkdir(path.join(dir, 'slow'), { recursive: true });
    await writeFile(path.join(dir, 'slow', 'meta.env'), 'RPI_E2E_TIMEOUT=1200\n');
    await mkdir(path.join(dir, 'plain'), { recursive: true });
    await mkdir(path.join(dir, 'bad'), { recursive: true });
    await writeFile(path.join(dir, 'bad', 'meta.env'), 'RPI_E2E_TIMEOUT=soon\n');
    assert.equal(await scenarioTimeoutMs('slow', dir), 1_200_000);
    assert.equal(await scenarioTimeoutMs('plain', dir), 15 * 60 * 1000);
    assert.equal(await scenarioTimeoutMs('bad', dir), 15 * 60 * 1000);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('parseRunArgs collects scenario filters and flags', () => {
  assert.deepEqual(parseRunArgs([]), { scenarios: [], failFast: false });
  assert.deepEqual(parseRunArgs(['happy-path', '--fail-fast']), {
    scenarios: ['happy-path'],
    failFast: true,
  });
  assert.deepEqual(parseRunArgs(['--concurrency', '3', 'a', 'b']), {
    scenarios: ['a', 'b'],
    failFast: false,
    concurrency: 3,
  });
  assert.throws(() => parseRunArgs(['--concurrency', 'many']), /positive integer/);
  assert.throws(() => parseRunArgs(['--wat']), /unknown flag/);
});
