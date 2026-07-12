import { randomBytes } from 'node:crypto';
import { spawn } from 'node:child_process';
import { createWriteStream, existsSync } from 'node:fs';
import { access, mkdir, readdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { prepareLegacyTar } from './prepare-legacy.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, '..', '..');
const COMPOSE_FILE = path.join('tests', 'e2e', 'base.compose.yaml');
const SCENARIOS_DIR = path.join(HERE, 'scenarios');
const MIN_COMPOSE = [2, 33, 1];
const BUILD_TIMEOUT_MS = 30 * 60 * 1000;
const SCENARIO_TIMEOUT_MS = 15 * 60 * 1000;
const DEFAULT_CONCURRENCY = 2;
/** Valid Docker Compose project-name component: also the scenario-folder-name contract. */
const SCENARIO_NAME_RE = /^[a-z0-9][a-z0-9-]*$/;

export function parseComposeVersion(text) {
  const match = /(?:^|\s)v?(\d+)\.(\d+)\.(\d+)(?:[-+][^\s]+)?(?:\s|$)/.exec(text.trim());
  if (!match) throw new Error(`cannot parse Docker Compose version: ${text.trim()}`);
  return match.slice(1).map(Number);
}

function versionAtLeast(actual, minimum) {
  for (let i = 0; i < minimum.length; i += 1) {
    if (actual[i] > minimum[i]) return true;
    if (actual[i] < minimum[i]) return false;
  }
  return true;
}

export function makeProjectName({
  pid = process.pid,
  now = Date.now(),
  suffix = randomBytes(3).toString('hex'),
} = {}) {
  return `rpi-e2e-${pid}-${now.toString(36)}-${suffix}`.toLowerCase();
}

/**
 * Scenario folders under tests/e2e/scenarios that contain a scenario.sh.
 * Only folder names matching SCENARIO_NAME_RE (a valid Docker Compose
 * project-name component) are returned; a scenario folder with an invalid
 * name is skipped but reported via console.warn so a typo is discoverable
 * instead of silently dropped.
 */
export async function discoverScenarios(dir = SCENARIOS_DIR) {
  let entries;
  try {
    entries = await readdir(dir, { withFileTypes: true });
  } catch {
    return [];
  }
  const names = [];
  for (const entry of entries) {
    if (!entry.isDirectory()) continue;
    try {
      await access(path.join(dir, entry.name, 'scenario.sh'));
    } catch {
      continue;
    }
    if (SCENARIO_NAME_RE.test(entry.name)) {
      names.push(entry.name);
    } else {
      console.warn(
        `rpi e2e: ignoring scenario folder with invalid name: ${entry.name} ` +
          `(must match ${SCENARIO_NAME_RE})`,
      );
    }
  }
  return names.sort();
}

/**
 * Bounded worker pool. results[i] corresponds to items[i]. Once stopOn(result)
 * returns true, no new items are dispatched; in-flight workers finish and the
 * undispatched slots stay undefined.
 */
export async function runPool(items, limit, worker, { stopOn } = {}) {
  const results = new Array(items.length);
  let next = 0;
  let stopped = false;
  const lanes = Array.from(
    { length: Math.max(1, Math.min(limit, items.length)) },
    async () => {
      while (!stopped) {
        const i = next;
        if (i >= items.length) return;
        next += 1;
        results[i] = await worker(items[i], i);
        if (stopOn?.(results[i])) stopped = true;
      }
    },
  );
  await Promise.all(lanes);
  return results;
}

export function formatSummary(results) {
  const width = Math.max(...results.map((r) => r.scenario.length));
  const lines = results.map((r) => {
    const status = r.skipped
      ? 'SKIP'
      : r.code === 0
        ? 'PASS'
        : r.timedOut
          ? 'FAIL (timeout)'
          : `FAIL (exit ${r.code})`;
    const duration = r.skipped ? '' : ` ${Math.round((r.durationMs ?? 0) / 1000)}s`;
    return `  ${r.scenario.padEnd(width)}  ${status}${duration}`;
  });
  const failed = results.filter((r) => !r.skipped && r.code !== 0).length;
  const skipped = results.filter((r) => r.skipped).length;
  const passed = results.length - failed - skipped;
  const tail = skipped ? `, ${skipped} skipped` : '';
  return [`rpi e2e: ${passed}/${results.length} scenarios passed${tail}`, ...lines].join('\n');
}

/** KEY=VALUE lines; '#' comments and blanks ignored. */
export function parseMetaEnv(text) {
  const out = {};
  for (const raw of text.split('\n')) {
    const line = raw.trim();
    if (!line || line.startsWith('#')) continue;
    const eq = line.indexOf('=');
    if (eq <= 0) continue;
    out[line.slice(0, eq).trim()] = line.slice(eq + 1).trim();
  }
  return out;
}

/** Per-scenario client timeout: RPI_E2E_TIMEOUT (seconds) from meta.env. */
export async function scenarioTimeoutMs(scenario, dir = SCENARIOS_DIR) {
  try {
    const meta = parseMetaEnv(await readFile(path.join(dir, scenario, 'meta.env'), 'utf8'));
    const secs = Number(meta.RPI_E2E_TIMEOUT);
    if (Number.isInteger(secs) && secs > 0) return secs * 1000;
  } catch {
    /* no meta.env — use the default */
  }
  return SCENARIO_TIMEOUT_MS;
}

/** CLI: positional scenario filters, --fail-fast, --concurrency N. */
export function parseRunArgs(args) {
  const options = { scenarios: [], failFast: false };
  for (let i = 0; i < args.length; i += 1) {
    const arg = args[i];
    if (arg === '--fail-fast') {
      options.failFast = true;
    } else if (arg === '--concurrency') {
      const value = Number(args[i + 1]);
      if (!Number.isInteger(value) || value < 1) {
        throw new Error(`--concurrency needs a positive integer, got: ${args[i + 1] ?? '(nothing)'}`);
      }
      options.concurrency = value;
      i += 1;
    } else if (arg.startsWith('--')) {
      throw new Error(`unknown flag: ${arg}`);
    } else {
      options.scenarios.push(arg);
    }
  }
  return options;
}

export async function spawnDocker(args, {
  cwd = ROOT,
  env = process.env,
  logPath,
  quiet = false,
  timeoutMs = 0,
  signal,
} = {}) {
  if (logPath) await mkdir(path.dirname(logPath), { recursive: true });
  return new Promise((resolve) => {
    const child = spawn('docker', args, {
      cwd,
      env,
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    });
    const log = logPath ? createWriteStream(logPath, { flags: 'a' }) : null;
    let stdout = '';
    let stderr = '';
    let timedOut = false;
    let settled = false;
    let forceTimer;

    const finish = (result) => {
      if (settled) return;
      settled = true;
      if (forceTimer) clearTimeout(forceTimer);
      log?.end();
      signal?.removeEventListener('abort', abort);
      resolve(result);
    };
    const abort = () => {
      if (timedOut) return;
      timedOut = true;
      child.kill('SIGTERM');
      forceTimer = setTimeout(() => child.kill('SIGKILL'), 5_000);
      forceTimer.unref();
    };
    const timer = timeoutMs ? setTimeout(abort, timeoutMs) : null;
    timer?.unref();
    signal?.addEventListener('abort', abort, { once: true });

    child.stdout.on('data', (chunk) => {
      const text = chunk.toString();
      stdout += text;
      if (!quiet) process.stdout.write(chunk);
      log?.write(chunk);
    });
    child.stderr.on('data', (chunk) => {
      const text = chunk.toString();
      stderr += text;
      if (!quiet) process.stderr.write(chunk);
      log?.write(chunk);
    });
    child.on('error', (error) => {
      if (timer) clearTimeout(timer);
      finish({ code: 1, stdout, stderr: `${stderr}${error.message}\n`, timedOut });
    });
    child.on('close', (code) => {
      if (timer) clearTimeout(timer);
      finish({ code: code ?? 1, stdout, stderr, timedOut });
    });
  });
}

async function collectDiagnostics(compose, artifactDir) {
  const nested = [
    'set +e',
    'docker info',
    'docker ps -a',
    'docker images',
    'for id in $(docker ps -aq); do',
    '  echo "===== docker logs $id ====="',
    '  docker logs --tail 200 "$id" 2>&1',
    'done',
  ].join('\n');
  await compose(['ps', '--all'], {
    logPath: path.join(artifactDir, 'outer-ps.log'),
    timeoutMs: 60_000,
  });
  await compose(['logs', '--no-color', '--timestamps'], {
    logPath: path.join(artifactDir, 'outer.log'),
    timeoutMs: 60_000,
  });
  await compose(['exec', '-T', 'target', 'bash', '-lc', nested], {
    logPath: path.join(artifactDir, 'nested.log'),
    timeoutMs: 60_000,
  });
}

/**
 * One isolated scenario stack: its own compose project, DinD engine, agent,
 * git fixture, and client run. Returns a result row for the summary.
 */
export async function runScenario({
  scenario,
  projectName,
  runtimeImage,
  artifactDir,
  runner = spawnDocker,
  env = process.env,
  keep = env.RPI_E2E_KEEP === '1',
  timeoutMs = SCENARIO_TIMEOUT_MS,
  scenariosDir = SCENARIOS_DIR,
  signal,
}) {
  const startedAt = Date.now();
  const composeEnv = {
    ...env,
    RPI_E2E_ARTIFACT_DIR: artifactDir,
    RPI_E2E_RUNTIME_IMAGE: runtimeImage,
    RPI_E2E_SCENARIO: scenario,
  };
  const files = ['--file', COMPOSE_FILE];
  const override = path.join(scenariosDir, scenario, 'compose.override.yaml');
  if (existsSync(override)) files.push('--file', override);
  const base = ['compose', '--ansi', 'never', '--project-name', projectName, ...files];
  const compose = (tail, options = {}) => runner([...base, ...tail], {
    env: composeEnv,
    signal,
    ...options,
  });
  // Best-effort: diagnostics are a nice-to-have on failure, never a reason to
  // let runScenario reject and orphan sibling pool lanes.
  const diagnostics = async () => {
    try {
      await collectDiagnostics(compose, artifactDir);
    } catch (diagError) {
      console.error(
        `rpi e2e: diagnostics collection failed (${scenario}): ` +
          `${diagError instanceof Error ? diagError.message : diagError}`,
      );
    }
  };

  let code = 1;
  let timedOut = false;
  let attemptedStart = false;
  try {
    await mkdir(artifactDir, { recursive: true });
    const config = await compose(['config', '--quiet'], { quiet: true });
    if (config.code !== 0) throw new Error(config.stderr || `compose config failed (${scenario})`);

    attemptedStart = true;
    const dependencies = await compose([
      'up', '-d', '--no-build', '--wait', '--wait-timeout', '120',
      'dind', 'target', 'git-fixture',
    ], { logPath: path.join(artifactDir, 'dependencies.log') });
    if (dependencies.code !== 0) {
      timedOut = dependencies.timedOut;
      code = signal?.aborted ? 130 : (dependencies.timedOut ? 124 : (dependencies.code || 1));
      await diagnostics();
    } else {
      const client = await compose(['run', '--rm', '--no-deps', 'client'], {
        logPath: path.join(artifactDir, 'scenario.log'),
        timeoutMs,
      });
      timedOut = client.timedOut;
      code = signal?.aborted ? 130 : (client.timedOut ? 124 : client.code);
      if (code !== 0) await diagnostics();
    }
  } catch (error) {
    code = signal?.aborted ? 130 : 1;
    try {
      await writeFile(
        path.join(artifactDir, 'launcher-error.log'),
        `${error instanceof Error ? error.stack || error.message : String(error)}\n`,
        'utf8',
      );
    } catch (writeError) {
      console.error(`rpi e2e: cannot write launcher error: ${writeError}`);
    }
    if (attemptedStart) await diagnostics();
  } finally {
    if (attemptedStart && keep) {
      console.warn(`rpi e2e: RPI_E2E_KEEP=1 — stack kept (project ${projectName})`);
      console.warn(`rpi e2e:   docker exec -it ${projectName}-target-1 bash`);
      console.warn(`rpi e2e:   clean up: node tests/e2e/run.mjs --down ${projectName}`);
    }
    if (attemptedStart && !keep) {
      // Teardown failure fails a green scenario but never masks a red one —
      // and, same as diagnostics, must never escape as a rejection.
      let down;
      try {
        down = await compose(['down', '-v', '--remove-orphans'], {
          logPath: path.join(artifactDir, 'cleanup.log'),
          timeoutMs: 120_000,
        });
      } catch (downError) {
        down = { code: 1 };
        console.error(
          `rpi e2e: teardown failed (${scenario}): ` +
            `${downError instanceof Error ? downError.message : downError}`,
        );
      }
      if (code === 0 && down.code !== 0) code = down.code || 1;
    }
  }
  return { scenario, code, timedOut, durationMs: Date.now() - startedAt, artifactDir };
}

/**
 * Orchestrator: validate scenario names, check the compose version, build the
 * shared runtime image once, run scenario stacks through a bounded pool, print
 * the summary, and remove the locally built image.
 */
export async function runE2E({
  runner = spawnDocker,
  env = process.env,
  projectName = makeProjectName(),
  scenarios = [],
  available,
  concurrency,
  failFast = false,
  artifactDir = env.RPI_E2E_ARTIFACT_DIR
    ? path.resolve(env.RPI_E2E_ARTIFACT_DIR)
    : path.join(ROOT, 'target', 'e2e-artifacts', projectName),
  signal,
  prepareLegacy = prepareLegacyTar,
} = {}) {
  try {
    await mkdir(artifactDir, { recursive: true });
  } catch (error) {
    console.error(
      `rpi e2e: cannot create artifact directory ${artifactDir}: ` +
        `${error instanceof Error ? error.message : error}`,
    );
    return signal?.aborted ? 130 : 1;
  }
  if (signal?.aborted) return 130;

  const all = available ?? await discoverScenarios();
  if (!all.length) {
    console.error('rpi e2e: no scenarios found under tests/e2e/scenarios');
    return 2;
  }
  const unknown = scenarios.filter((name) => !all.includes(name));
  if (unknown.length) {
    console.error(
      `rpi e2e: unknown scenario(s): ${unknown.join(', ')} (available: ${all.join(', ')})`,
    );
    return 2;
  }
  const selected = scenarios.length ? scenarios : all;

  const prebuilt = env.RPI_E2E_PREBUILT === '1';
  const keep = env.RPI_E2E_KEEP === '1';
  const runtimeImage = env.RPI_E2E_RUNTIME_IMAGE || `rpi-e2e-runtime:${projectName}`;
  const envConcurrency = Number.parseInt(env.RPI_E2E_CONCURRENCY ?? '', 10);
  const poolSize = concurrency
    ?? (Number.isInteger(envConcurrency) && envConcurrency >= 1
      ? envConcurrency
      : DEFAULT_CONCURRENCY);
  const buildEnv = {
    ...env,
    RPI_E2E_ARTIFACT_DIR: artifactDir,
    RPI_E2E_RUNTIME_IMAGE: runtimeImage,
    RPI_E2E_SCENARIO: selected[0],
  };
  const base = ['compose', '--ansi', 'never', '--project-name', projectName, '--file', COMPOSE_FILE];

  let localImageTouched = false;
  let results = [];
  try {
    const versionResult = await runner(['compose', 'version', '--short'], {
      env: buildEnv,
      quiet: true,
      signal,
    });
    if (versionResult.code !== 0) throw new Error(versionResult.stderr || 'docker compose unavailable');
    const version = parseComposeVersion(versionResult.stdout);
    if (!versionAtLeast(version, MIN_COMPOSE)) {
      throw new Error(`Docker Compose >= ${MIN_COMPOSE.join('.')} required; found ${version.join('.')}`);
    }

    if (!prebuilt) {
      await prepareLegacy();
      localImageTouched = true;
      const build = await runner([...base, 'build', 'client'], {
        env: buildEnv,
        signal,
        logPath: path.join(artifactDir, 'build.log'),
        timeoutMs: BUILD_TIMEOUT_MS,
      });
      if (build.code !== 0) throw new Error('e2e runtime image build failed');
    }

    const pooled = await runPool(
      selected,
      poolSize,
      async (scenario) => {
        if (signal?.aborted) return { scenario, skipped: true };
        return runScenario({
          scenario,
          projectName: `${projectName}-${scenario}`,
          runtimeImage,
          artifactDir: path.join(artifactDir, scenario),
          runner,
          env,
          keep,
          timeoutMs: await scenarioTimeoutMs(scenario),
          signal,
        });
      },
      { stopOn: failFast ? (result) => !result.skipped && result.code !== 0 : undefined },
    );
    results = selected.map((scenario, i) => pooled[i] ?? { scenario, skipped: true });
    console.log(formatSummary(results));
  } catch (error) {
    try {
      await writeFile(
        path.join(artifactDir, 'launcher-error.log'),
        `${error instanceof Error ? error.stack || error.message : String(error)}\n`,
        'utf8',
      );
    } catch (writeError) {
      console.error(`rpi e2e: cannot write launcher error: ${writeError}`);
    }
    return signal?.aborted ? 130 : 1;
  } finally {
    if (localImageTouched && !keep) {
      // Best-effort, same as the diagnostics/teardown paths above: a rejecting
      // runner here must never override the code computed above by throwing
      // out of this finally block.
      try {
        await runner(['image', 'rm', runtimeImage], { env: buildEnv, quiet: true, signal });
      } catch (imageRmError) {
        console.error(
          `rpi e2e: image rm failed (${runtimeImage}): ` +
            `${imageRmError instanceof Error ? imageRmError.message : imageRmError}`,
        );
      }
    }
  }
  if (signal?.aborted) return 130;
  const failed = results.find((r) => !r.skipped && r.code !== 0);
  return failed ? (failed.code || 1) : 0;
}

export async function runDev(action, {
  runner = spawnDocker,
  env = process.env,
  scenario = 'happy-path',
  projectName = 'rpi-e2e-dev',
  artifactDir = path.join(ROOT, 'target', 'e2e-artifacts', projectName),
  available,
  signal,
} = {}) {
  await mkdir(artifactDir, { recursive: true });

  const knownScenarios = available ?? await discoverScenarios();
  if (!SCENARIO_NAME_RE.test(scenario) || !knownScenarios.includes(scenario)) {
    console.error(
      `rpi e2e dev: unknown scenario: ${scenario} (available: ${knownScenarios.join(', ')})`,
    );
    return 2;
  }

  const runtimeImage = env.RPI_E2E_RUNTIME_IMAGE || `rpi-e2e-runtime:${projectName}`;
  const composeEnv = {
    ...env,
    RPI_E2E_ARTIFACT_DIR: artifactDir,
    RPI_E2E_RUNTIME_IMAGE: runtimeImage,
    RPI_E2E_SCENARIO: scenario,
  };
  const files = ['--file', COMPOSE_FILE];
  const override = path.join(SCENARIOS_DIR, scenario, 'compose.override.yaml');
  if (existsSync(override)) files.push('--file', override);
  const base = [
    'compose', '--ansi', 'never', '--project-name', projectName,
    ...files, '--profile', 'dev',
  ];
  const compose = (tail, options = {}) => runner([...base, ...tail], {
    env: composeEnv,
    signal,
    ...options,
  });
  if (action === 'up') {
    const build = await compose(['build', 'client'], { timeoutMs: BUILD_TIMEOUT_MS });
    if (build.code !== 0) return build.code || 1;
    const up = await compose([
      'up', '-d', '--no-build', '--wait', '--wait-timeout', '120',
      'dind', 'target', 'git-fixture', 'client-dev',
    ]);
    if (up.code !== 0) return up.code || 1;
    console.log(`rpi e2e dev: stack is up (project ${projectName}, scenario ${scenario})`);
    console.log(`  docker exec -it ${projectName}-client-dev-1 bash   # rpi CLI + SSH key`);
    console.log(`  docker exec -it ${projectName}-target-1 bash       # agent + sshd + nested Docker`);
    console.log('  in client-dev, once: source /opt/e2e/lib.sh && e2e_bootstrap');
    return 0;
  }
  if (action === 'down') {
    // Same defensive symmetry as runScenario/runE2E's teardown: a rejecting
    // runner on either call must not throw out of runDev.
    let down;
    try {
      down = await compose(['down', '-v', '--remove-orphans'], { timeoutMs: 120_000 });
    } catch (downError) {
      down = { code: 1 };
      console.error(
        `rpi e2e dev: teardown failed: ` +
          `${downError instanceof Error ? downError.message : downError}`,
      );
    }
    try {
      await runner(['image', 'rm', runtimeImage], { env: composeEnv, quiet: true, signal });
    } catch (imageRmError) {
      console.error(
        `rpi e2e dev: image rm failed (${runtimeImage}): ` +
          `${imageRmError instanceof Error ? imageRmError.message : imageRmError}`,
      );
    }
    return down.code || 0;
  }
  throw new Error(`unknown dev action: ${action}`);
}

if (process.argv[1] && pathToFileURL(path.resolve(process.argv[1])).href === import.meta.url) {
  const controller = new AbortController();
  process.once('SIGINT', () => controller.abort());
  process.once('SIGTERM', () => controller.abort());
  const args = process.argv.slice(2);
  if (args[0] === '--dev-up') {
    process.exitCode = await runDev('up', {
      signal: controller.signal,
      ...(args[1] ? { scenario: args[1] } : {}),
    });
  } else if (args[0] === '--dev-down' || args[0] === '--down') {
    process.exitCode = await runDev('down', {
      signal: controller.signal,
      ...(args[0] === '--down' && args[1] ? { projectName: args[1] } : {}),
    });
  } else {
    let options;
    try {
      options = parseRunArgs(args);
    } catch (error) {
      console.error(`rpi e2e: ${error instanceof Error ? error.message : error}`);
      process.exitCode = 2;
    }
    if (options) {
      console.warn('rpi e2e: starting isolated privileged Docker-in-Docker daemons');
      process.exitCode = await runE2E({ ...options, signal: controller.signal });
    }
  }
}
