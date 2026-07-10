import { randomBytes } from 'node:crypto';
import { spawn } from 'node:child_process';
import { createWriteStream } from 'node:fs';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, '..', '..');
const COMPOSE_FILE = path.join('tests', 'e2e', 'compose.yaml');
const MIN_COMPOSE = [2, 33, 1];
const BUILD_TIMEOUT_MS = 30 * 60 * 1000;
const SCENARIO_TIMEOUT_MS = 15 * 60 * 1000;

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

export async function runE2E({
  runner = spawnDocker,
  env = process.env,
  projectName = makeProjectName(),
  artifactDir = env.RPI_E2E_ARTIFACT_DIR
    ? path.resolve(env.RPI_E2E_ARTIFACT_DIR)
    : path.join(ROOT, 'target', 'e2e-artifacts', projectName),
  signal,
} = {}) {
  await mkdir(artifactDir, { recursive: true });
  if (signal?.aborted) return 130;
  const prebuilt = env.RPI_E2E_PREBUILT === '1';
  const runtimeImage = env.RPI_E2E_RUNTIME_IMAGE || `rpi-e2e-runtime:${projectName}`;
  const composeEnv = {
    ...env,
    RPI_E2E_ARTIFACT_DIR: artifactDir,
    RPI_E2E_RUNTIME_IMAGE: runtimeImage,
  };
  const base = [
    'compose', '--ansi', 'never', '--project-name', projectName,
    '--file', COMPOSE_FILE,
  ];
  const compose = (tail, options = {}) => runner([...base, ...tail], {
    env: composeEnv,
    signal,
    ...options,
  });

  let primaryCode = 1;
  let attemptedStart = false;
  let localImageTouched = false;
  try {
    const versionResult = await runner(['compose', 'version', '--short'], {
      env: composeEnv,
      quiet: true,
      signal,
    });
    if (versionResult.code !== 0) throw new Error(versionResult.stderr || 'docker compose unavailable');
    const version = parseComposeVersion(versionResult.stdout);
    if (!versionAtLeast(version, MIN_COMPOSE)) {
      throw new Error(`Docker Compose >= ${MIN_COMPOSE.join('.')} required; found ${version.join('.')}`);
    }

    const config = await compose(['config', '--quiet'], { quiet: true });
    if (config.code !== 0) throw new Error(config.stderr || 'docker compose config failed');

    attemptedStart = true;
    if (!prebuilt) {
      localImageTouched = true;
      const build = await compose(['build', 'client'], {
        logPath: path.join(artifactDir, 'build.log'),
        timeoutMs: BUILD_TIMEOUT_MS,
      });
      if (build.code !== 0) throw new Error('e2e runtime image build failed');
    }

    const dependencies = await compose([
      'up', '-d', '--no-build', '--wait', '--wait-timeout', '120',
      'dind', 'target', 'git-fixture',
    ], { logPath: path.join(artifactDir, 'dependencies.log') });
    if (dependencies.code !== 0) {
      primaryCode = signal?.aborted
        ? 130
        : (dependencies.timedOut ? 124 : (dependencies.code || 1));
      await collectDiagnostics(compose, artifactDir);
      return primaryCode;
    }

    const client = await compose(['run', '--rm', '--no-deps', 'client'], {
      logPath: path.join(artifactDir, 'scenario.log'),
      timeoutMs: SCENARIO_TIMEOUT_MS,
    });
    primaryCode = signal?.aborted
      ? 130
      : (client.timedOut ? 124 : client.code);
    if (primaryCode !== 0) await collectDiagnostics(compose, artifactDir);
  } catch (error) {
    primaryCode = signal?.aborted ? 130 : 1;
    try {
      await writeFile(
        path.join(artifactDir, 'launcher-error.log'),
        `${error instanceof Error ? error.stack || error.message : String(error)}\n`,
        'utf8',
      );
    } catch (writeError) {
      console.error(`rpi e2e: cannot write launcher error: ${writeError}`);
    }
    if (attemptedStart) await collectDiagnostics(compose, artifactDir);
  } finally {
    if (attemptedStart) {
      const down = await compose(['down', '-v', '--remove-orphans'], {
        logPath: path.join(artifactDir, 'cleanup.log'),
        timeoutMs: 120_000,
      });
      if (primaryCode === 0 && down.code !== 0) primaryCode = down.code || 1;
    }
    if (localImageTouched) {
      await runner(['image', 'rm', runtimeImage], {
        env: composeEnv,
        quiet: true,
        signal,
      });
    }
  }
  return primaryCode;
}

if (process.argv[1] && pathToFileURL(path.resolve(process.argv[1])).href === import.meta.url) {
  console.warn('rpi e2e: starting an isolated privileged Docker-in-Docker daemon');
  const controller = new AbortController();
  process.once('SIGINT', () => controller.abort());
  process.once('SIGTERM', () => controller.abort());
  process.exitCode = await runE2E({ signal: controller.signal });
}
