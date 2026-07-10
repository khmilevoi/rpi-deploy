import test from 'node:test';
import assert from 'node:assert/strict';
import os from 'node:os';
import path from 'node:path';
import { mkdtemp, rm } from 'node:fs/promises';

import { makeProjectName, parseComposeVersion, runDev, runE2E } from './run.mjs';

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
    assert.match(commands[1], /--file tests[/\\]e2e[/\\]compose\.yaml config --quiet$/);
    assert.match(commands[2], /--file tests[/\\]e2e[/\\]compose\.yaml build client$/);
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
