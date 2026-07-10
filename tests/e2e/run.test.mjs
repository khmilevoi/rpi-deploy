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
  runScenario,
  scenarioTimeoutMs,
} from './run.mjs';

const ok = (stdout = '') => ({ code: 0, stdout, stderr: '', timedOut: false });

function fakeRunner(responses) {
  const calls = [];
  const runner = async (args, options = {}) => {
    calls.push({ args, options });
    const next = responses.length ? responses.shift() : ok();
    if (next instanceof Error) throw next;
    return next;
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

test('single scenario success: version, build, config, up, run client, down, image rm', async () => {
  await withArtifacts(async (artifactDir) => {
    const { calls, runner } = fakeRunner([
      ok('2.33.1'), // version
      ok(),         // build client (once, shared image)
      ok(),         // config --quiet (per scenario)
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
      available: ['happy-path'],
    });
    assert.equal(code, 0);
    const commands = calls.map((call) => call.args.join(' '));
    assert.match(commands[0], /^compose version --short$/);
    assert.match(commands[1], /--file tests[/\\]e2e[/\\]base\.compose\.yaml build client$/);
    assert.match(commands[2], /--project-name rpi-e2e-test-happy-path .*config --quiet$/);
    assert.match(commands[3], /up -d --no-build --wait --wait-timeout 120 dind target git-fixture$/);
    assert.match(commands[4], /run --rm --no-deps client$/);
    assert.match(commands[5], /down -v --remove-orphans$/);
    assert.match(commands[6], /image rm rpi-e2e-runtime:rpi-e2e-test$/);
    const clientCall = calls[4];
    assert.equal(clientCall.options.env.RPI_E2E_SCENARIO, 'happy-path');
    assert.ok(clientCall.options.env.RPI_E2E_ARTIFACT_DIR.endsWith('happy-path'));
  });
});

test('multiple scenarios reuse one image build and aggregate the exit code', async () => {
  await withArtifacts(async (artifactDir) => {
    const { calls, runner } = fakeRunner([
      ok('2.33.1'), // version
      ok(),         // build client (once)
      ok(), ok(), ok(), ok(),                                   // alpha: config, up, run, down
      ok(), ok(),                                               // beta: config, up
      { code: 7, stdout: '', stderr: '', timedOut: false },     // beta: run client fails
      ok(), ok(), ok(),                                         // beta: diagnostics (ps, logs, exec)
      ok(),                                                     // beta: down
      ok(),                                                     // image rm
    ]);
    const code = await runE2E({
      runner,
      artifactDir,
      projectName: 'rpi-e2e-multi',
      env: {},
      available: ['alpha', 'beta'],
      concurrency: 1,
    });
    assert.equal(code, 7);
    const runCalls = calls.filter((c) => c.args.includes('run'));
    assert.equal(runCalls.length, 2);
    assert.equal(runCalls[0].options.env.RPI_E2E_SCENARIO, 'alpha');
    assert.equal(runCalls[1].options.env.RPI_E2E_SCENARIO, 'beta');
    const joined = calls.map((c) => c.args.join(' ')).join('\n');
    assert.equal((joined.match(/build client/g) || []).length, 1);
    assert.match(joined, /--project-name rpi-e2e-multi-alpha/);
    assert.match(joined, /--project-name rpi-e2e-multi-beta/);
  });
});

test('--fail-fast stops dispatching after the first failure', async () => {
  await withArtifacts(async (artifactDir) => {
    const { calls, runner } = fakeRunner([
      ok('2.33.1'), ok(),                                       // version, build
      ok(), ok(),                                               // alpha: config, up
      { code: 23, stdout: '', stderr: '', timedOut: false },    // alpha: run fails
      ok(), ok(), ok(),                                         // alpha: diagnostics
      ok(),                                                     // alpha: down
      ok(),                                                     // image rm
    ]);
    const code = await runE2E({
      runner,
      artifactDir,
      projectName: 'rpi-e2e-ff',
      env: {},
      available: ['alpha', 'beta', 'gamma'],
      concurrency: 1,
      failFast: true,
    });
    assert.equal(code, 23);
    assert.equal(calls.filter((c) => c.args.includes('run')).length, 1);
    const joined = calls.map((c) => c.args.join(' ')).join('\n');
    assert.doesNotMatch(joined, /-beta/);
    assert.doesNotMatch(joined, /-gamma/);
  });
});

test('a scenario filter runs only the requested scenario', async () => {
  await withArtifacts(async (artifactDir) => {
    const { calls, runner } = fakeRunner([
      ok('2.33.1'), ok(), ok(), ok(), ok(), ok(), ok(),
    ]);
    const code = await runE2E({
      runner,
      artifactDir,
      projectName: 'rpi-e2e-pick',
      env: {},
      available: ['alpha', 'beta'],
      scenarios: ['beta'],
    });
    assert.equal(code, 0);
    const runCall = calls.find((c) => c.args.includes('run'));
    assert.equal(runCall.options.env.RPI_E2E_SCENARIO, 'beta');
    const joined = calls.map((c) => c.args.join(' ')).join('\n');
    assert.doesNotMatch(joined, /-alpha/);
  });
});

test('an unknown scenario fails with exit 2 before touching Docker', async () => {
  await withArtifacts(async (artifactDir) => {
    const { calls, runner } = fakeRunner([]);
    const code = await runE2E({
      runner,
      artifactDir,
      env: {},
      available: ['alpha'],
      scenarios: ['nope'],
    });
    assert.equal(code, 2);
    assert.equal(calls.length, 0);
  });
});

test('client failure collects diagnostics before teardown and keeps the exit code', async () => {
  await withArtifacts(async (artifactDir) => {
    const { calls, runner } = fakeRunner([
      ok('2.33.1'), ok(), ok(), ok(),
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
      available: ['happy-path'],
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
      available: ['happy-path'],
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
      available: ['happy-path'],
    }), 17);
  });
});

test('runScenario returns a nonzero-code result instead of throwing when the runner rejects', async () => {
  await withArtifacts(async (artifactDir) => {
    const { calls, runner } = fakeRunner([
      ok(),                             // config --quiet
      ok(),                             // up dependencies
      new Error('run step exploded'),   // run --rm --no-deps client rejects
      ok(), ok(), ok(),                 // diagnostics (ps, logs, exec)
      ok(),                             // down
    ]);
    const result = await runScenario({
      scenario: 'happy-path',
      projectName: 'rpi-e2e-reject-run',
      runtimeImage: 'img:x',
      artifactDir,
      runner,
      env: {},
    });
    assert.equal(typeof result.code, 'number');
    assert.notEqual(result.code, 0);
    assert.equal(result.scenario, 'happy-path');
    const joined = calls.map((c) => c.args.join(' ')).join('\n');
    assert.match(joined, /down -v --remove-orphans/);
  });
});

test('runScenario swallows a rejecting teardown, still fails green and never masks red', async () => {
  await withArtifacts(async (artifactDir) => {
    // Green scenario: run succeeds, but `down` rejects — teardown failure
    // must fail the scenario (matches the nonzero-down-code behavior above)
    // without throwing out of runScenario.
    const greenWithBadTeardown = fakeRunner([
      ok(), ok(), ok(),                 // config, up, run (all succeed)
      new Error('down exploded'),       // down rejects
    ]);
    const green = await runScenario({
      scenario: 'happy-path',
      projectName: 'rpi-e2e-green-bad-down',
      runtimeImage: 'img:x',
      artifactDir,
      runner: greenWithBadTeardown.runner,
      env: {},
    });
    assert.equal(typeof green.code, 'number');
    assert.notEqual(green.code, 0);

    // Red scenario: run fails, and down also rejects — the run's exit code
    // must win, not get replaced by the teardown failure.
    const redWithBadTeardown = fakeRunner([
      ok(), ok(),                                              // config, up
      { code: 17, stdout: '', stderr: '', timedOut: false },   // run fails
      ok(), ok(), ok(),                                        // diagnostics
      new Error('down exploded'),                               // down rejects
    ]);
    const red = await runScenario({
      scenario: 'happy-path',
      projectName: 'rpi-e2e-red-bad-down',
      runtimeImage: 'img:x',
      artifactDir,
      runner: redWithBadTeardown.runner,
      env: {},
    });
    assert.equal(red.code, 17);
  });
});

test('a teardown rejection does not throw out of the pool, mask a red scenario, or orphan siblings', async () => {
  await withArtifacts(async (artifactDir) => {
    const { calls, runner } = fakeRunner([
      ok('2.33.1'), ok(),                                       // version, build
      ok(), ok(),                                               // alpha: config, up
      { code: 17, stdout: '', stderr: '', timedOut: false },    // alpha: run fails
      ok(), ok(), ok(),                                         // alpha: diagnostics
      new Error('teardown exploded'),                           // alpha: down rejects
      ok(), ok(),                                               // beta: config, up
      ok(),                                                     // beta: run succeeds
      ok(),                                                     // beta: down
      ok(),                                                     // image rm
    ]);
    const code = await runE2E({
      runner,
      artifactDir,
      projectName: 'rpi-e2e-teardown-reject',
      env: {},
      available: ['alpha', 'beta'],
      concurrency: 1,
    });
    // The first failing scenario's exit code wins — a rejecting teardown on
    // alpha must not mask it, and beta must still have run (not orphaned).
    assert.equal(code, 17);
    const runCalls = calls.filter((c) => c.args.includes('run') && c.args.includes('client'));
    assert.equal(runCalls.length, 2);
    assert.equal(runCalls[0].options.env.RPI_E2E_SCENARIO, 'alpha');
    assert.equal(runCalls[1].options.env.RPI_E2E_SCENARIO, 'beta');
  });
});

test('runE2E converts an artifact-directory mkdir failure into a controlled exit code', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'rpi-e2e-mkdir-'));
  try {
    const blocker = path.join(dir, 'blocker');
    await writeFile(blocker, 'x');
    const { calls, runner } = fakeRunner([]);
    const code = await runE2E({
      runner,
      artifactDir: path.join(blocker, 'nested'),
      env: {},
      available: ['happy-path'],
    });
    assert.equal(code, 1);
    assert.equal(calls.length, 0);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
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
      available: ['happy-path'],
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
      available: ['happy-path'],
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
      available: ['happy-path'],
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
      available: ['happy-path'],
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
      available: ['happy-path'],
    }), 23);
    const joined = calls.map((call) => call.args.join(' ')).join('\n');
    assert.doesNotMatch(joined, /down -v/);
    assert.doesNotMatch(joined, /image rm/);
  });
});

test('runScenario appends a scenario compose override when present', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'rpi-e2e-override-'));
  try {
    await mkdir(path.join(dir, 'custom'), { recursive: true });
    await writeFile(path.join(dir, 'custom', 'compose.override.yaml'), 'services: {}\n');
    await withArtifacts(async (artifactDir) => {
      const { calls, runner } = fakeRunner([ok(), ok(), ok(), ok()]);
      const result = await runScenario({
        scenario: 'custom',
        projectName: 'rpi-e2e-x-custom',
        runtimeImage: 'img:x',
        artifactDir,
        runner,
        env: {},
        scenariosDir: dir,
      });
      assert.equal(result.code, 0);
      const joined = calls.map((c) => c.args.join(' ')).join('\n');
      assert.match(joined, /--file .*custom[/\\]compose\.override\.yaml/);
    });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
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
    assert.equal(calls[0].options.env.RPI_E2E_SCENARIO, 'happy-path');
  });
});

test('dev up accepts a scenario argument', async () => {
  await withArtifacts(async (artifactDir) => {
    const { calls, runner } = fakeRunner([ok(), ok()]);
    assert.equal(await runDev('up', { runner, artifactDir, env: {}, scenario: 'custom' }), 0);
    assert.equal(calls[0].options.env.RPI_E2E_SCENARIO, 'custom');
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
    assert.equal(calls[0].options.env.RPI_E2E_SCENARIO, 'happy-path');
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
