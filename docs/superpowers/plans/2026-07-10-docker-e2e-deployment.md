# Docker Production-Path E2E Deployment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Docker-isolated merge gate that runs the current `rpi` CLI through real SSH and the agent Unix socket, deploys a Git fixture into DinD, verifies health and stable redeploy, removes it, and cleans up deterministically.

**Architecture:** A host-side Node launcher controls an outer Docker Compose project with a one-shot key generator, a local Git server, a target (`sshd` + `rpi-agent`), and an isolated privileged DinD daemon. Target and DinD share a network namespace so the agent's `127.0.0.1:<allocated-port>` healthcheck behaves exactly like a local Docker Engine; the terminal client runs separately with `docker compose run --rm --no-deps` so dependencies remain alive for diagnostics.

**Tech Stack:** Rust/Cargo workspace, Node.js 18+ (`node:test`, ESM), Docker Engine/Docker Desktop, Docker Compose v2.33.1+, Docker Buildx, Bash, OpenSSH, Git daemon, GitHub Actions.

## Global Constraints

- Source of truth: `docs/superpowers/specs/2026-07-10-docker-e2e-deployment-design.md` at or after commit `39e32ce`.
- Start execution in a clean isolated worktree created with `superpowers:using-git-worktrees`, from the user-selected base after the current dirty `master` checkout is reconciled. Never stage unrelated version/release edits from the current checkout.
- Suggested branch: `codex/docker-e2e-deployment`.
- Do not change production Rust code, public CLI/API behavior, schemas, or protocol types. If implementation appears to require that, stop and revise the design first.
- Local command remains exactly `npm run test:e2e`.
- The production-path scenario must never set `PI_AGENT_URL`.
- Only the `dind` service may use `privileged: true`; never mount `/var/run/docker.sock` from the host.
- Bind the DinD TCP API only to `127.0.0.1:2375` inside the shared target/DinD network namespace.
- Publish no outer or nested ports to the host.
- Use project name `e2e-fixture`, nested port range `18080-18089`, and require stable port `18080` across two deploys.
- Keep the full e2e job on `ubuntu-latest`; do not enable it on a self-hosted runner.
- GitHub job timeout: 30 minutes. Dependency readiness: 120 seconds maximum. Client scenario: 15 minutes maximum after image construction.
- Preserve the first meaningful failure code. Diagnostics and teardown failures are secondary; a teardown failure after an otherwise successful scenario must make the launcher fail.
- Use `apply_patch` for edits. Run focused tests red then green, and commit after every task.

---

## File Structure

| Path | Responsibility |
| --- | --- |
| `tests/e2e/run.mjs` | Cross-platform process runner, Compose lifecycle, timeouts, diagnostics, cleanup, CLI entrypoint. |
| `tests/e2e/run.test.mjs` | Docker-free unit tests for lifecycle ordering, exit-code preservation, prebuilt mode, cleanup, and version validation. |
| `tests/e2e/contracts.test.mjs` | Docker-free structural tests for fixture, Dockerfile, Compose isolation, scenario commands, and CI wiring. |
| `tests/e2e/Dockerfile` | Build the current `rpi` once and create the shared Linux runtime image. |
| `.dockerignore` | Keep repository-local build context deterministic and small. |
| `.gitattributes` | Force LF for executable e2e shell scripts and the exact health response. |
| `tests/e2e/agent.toml` | Unix-socket agent config and deterministic nested port/timeouts. |
| `tests/e2e/target-entrypoint.sh` | Prepare users/permissions, start `rpi-agent` and `sshd`, propagate process failure. |
| `tests/e2e/git-entrypoint.sh` | Turn the versioned fixture into a bare `main` repository and serve it with `git daemon`. |
| `tests/e2e/compose.yaml` | Outer keygen/Git/target/DinD/client topology and health dependencies. |
| `tests/e2e/scenario.sh` | SSH host-key setup, two deploys, CLI/HTTP assertions, and `rpi rm` verification. |
| `tests/e2e/fixtures/app/*` | Minimal deployable project that forces the agent's HTTP host-port health probe. |
| `package.json` | `test:node` and `test:e2e` commands; preserve all current package/version metadata. |
| `.github/workflows/ci.yml` | Cross-platform Node tests plus cached Ubuntu full e2e merge gate. |
| `README.md` | Local e2e command, prerequisites, privilege warning, scope, and artifact location. |
| `docs/ci-github-actions.md` | Merge-gate topology, cache, security boundary, and failure artifacts. |

`.gitignore` already contains `.superpowers`; verify it remains present, but do not modify it as part of this feature.

---

### Task 1: Cross-platform E2E launcher lifecycle

**Files:**
- Create: `tests/e2e/run.mjs`
- Create: `tests/e2e/run.test.mjs`
- Modify: `package.json`

**Interfaces:**
- Produces: `parseComposeVersion(text): number[]`, `makeProjectName(options?): string`, `spawnDocker(args, options?): Promise<RunResult>`, and `runE2E(options?): Promise<number>`.
- `RunResult` shape: `{ code: number, stdout: string, stderr: string, timedOut: boolean }`.
- `runE2E` injected runner signature: `(args: string[], options: RunOptions) => Promise<RunResult>`.
- Later tasks provide `tests/e2e/compose.yaml` with services `keygen`, `dind`, `target`, `git-fixture`, and `client`.
- Environment contract: optional `RPI_E2E_PREBUILT=1`, optional `RPI_E2E_RUNTIME_IMAGE`, optional `RPI_E2E_ARTIFACT_DIR`.

- [ ] **Step 1: Add the failing launcher tests**

Create `tests/e2e/run.test.mjs`:

```js
import test from 'node:test';
import assert from 'node:assert/strict';
import os from 'node:os';
import path from 'node:path';
import { mkdtemp, rm } from 'node:fs/promises';

import { makeProjectName, parseComposeVersion, runE2E } from './run.mjs';

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
```

- [ ] **Step 2: Run the test and confirm the red state**

Run:

```text
node --test tests/e2e/run.test.mjs
```

Expected: FAIL with `ERR_MODULE_NOT_FOUND` for `tests/e2e/run.mjs`.

- [ ] **Step 3: Implement the launcher**

Create `tests/e2e/run.mjs` with these complete interfaces and lifecycle. Keep command construction exactly as shown so the tests and later Compose service names agree:

```js
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
```

When implementing, keep the `return` inside the dependency-failure branch safe: JavaScript still executes `finally`, so teardown remains guaranteed.

- [ ] **Step 4: Add package scripts without changing version or metadata**

Modify only the `scripts` object in `package.json`:

```json
"scripts": {
  "postinstall": "node scripts/postinstall.js",
  "prepublishOnly": "node scripts/check-version.js",
  "test:node": "node --test",
  "test:e2e": "node tests/e2e/run.mjs"
}
```

- [ ] **Step 5: Run the focused tests and make them green**

Run:

```text
npm run test:node
```

Expected: all existing postinstall tests and all nine launcher tests PASS. No Docker process starts because `run.test.mjs` injects a fake runner.

- [ ] **Step 6: Commit the launcher**

```bash
git add package.json tests/e2e/run.mjs tests/e2e/run.test.mjs
git commit -m "test(e2e): add cross-platform Docker lifecycle launcher"
```

---

### Task 2: Deterministic deployable Git fixture

**Files:**
- Create: `.gitattributes`
- Create: `tests/e2e/contracts.test.mjs`
- Create: `tests/e2e/fixtures/app/rpi.toml`
- Create: `tests/e2e/fixtures/app/compose.yaml`
- Create: `tests/e2e/fixtures/app/Dockerfile`
- Create: `tests/e2e/fixtures/app/health`

**Interfaces:**
- Produces Git tree content served later as `git://git-fixture/fixture.git`, branch `main`.
- Project contract: name `e2e-fixture`, Compose service `web`, container port `8080`, HTTP path `/health`, exact status `200`, no fixed host port, no Dockerfile `HEALTHCHECK`.

- [ ] **Step 1: Write the failing fixture contract test**

Create `tests/e2e/contracts.test.mjs`:

```js
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, '..', '..');
const read = (relative) => readFile(path.join(ROOT, relative), 'utf8');

test('fixture uses local Git, managed port allocation, HTTP fallback, and LF content', async () => {
  const [attributes, config, compose, dockerfile, health] = await Promise.all([
    read('.gitattributes'),
    read('tests/e2e/fixtures/app/rpi.toml'),
    read('tests/e2e/fixtures/app/compose.yaml'),
    read('tests/e2e/fixtures/app/Dockerfile'),
    read('tests/e2e/fixtures/app/health'),
  ]);
  assert.match(attributes, /tests\/e2e\/\*\.sh text eol=lf/);
  assert.match(attributes, /tests\/e2e\/fixtures\/app\/health text eol=lf/);
  assert.match(config, /name = "e2e-fixture"/);
  assert.match(config, /repo = "git:\/\/git-fixture\/fixture\.git"/);
  assert.match(config, /service = "web"/);
  assert.match(config, /port = 8080/);
  assert.match(config, /path = "\/health"/);
  assert.match(config, /expect = "200"/);
  assert.doesNotMatch(compose, /^\s*ports:/m);
  assert.match(compose, /^\s*expose:/m);
  assert.doesNotMatch(dockerfile, /^HEALTHCHECK/m);
  assert.equal(health.trim(), 'ok');
});
```

- [ ] **Step 2: Run the contract test and verify it fails**

Run:

```text
npm run test:node
```

Expected: FAIL with `ENOENT` for `.gitattributes` or
`tests/e2e/fixtures/app/rpi.toml`.

- [ ] **Step 3: Create the fixture configuration**

Create root `.gitattributes` before adding the fixture files:

```gitattributes
tests/e2e/*.sh text eol=lf
tests/e2e/fixtures/app/health text eol=lf
```

Create `tests/e2e/fixtures/app/rpi.toml`:

```toml
schema = 1

[project]
name = "e2e-fixture"

[source]
repo = "git://git-fixture/fixture.git"
branch = "main"

[build]
compose = "compose.yaml"

[ingress]
service = "web"
port = 8080

[healthcheck]
path = "/health"
expect = "200"
timeout = "30s"
```

Create `tests/e2e/fixtures/app/compose.yaml`:

```yaml
services:
  web:
    build: .
    expose:
      - "8080"
```

Create `tests/e2e/fixtures/app/Dockerfile`:

```dockerfile
FROM busybox:1.37
WORKDIR /www
COPY health /www/health
EXPOSE 8080
CMD ["httpd", "-f", "-p", "8080", "-h", "/www"]
```

Create `tests/e2e/fixtures/app/health` with exactly:

```text
ok
```

- [ ] **Step 4: Run the fixture contract test and make it green**

Run:

```text
npm run test:node
```

Expected: all Node tests PASS; specifically the fixture contract proves there is no fixed `ports:` mapping and no Dockerfile `HEALTHCHECK`.

- [ ] **Step 5: Commit the fixture**

```bash
git add .gitattributes tests/e2e/contracts.test.mjs tests/e2e/fixtures/app
git commit -m "test(e2e): add deterministic deploy fixture"
```

---

### Task 3: Shared Linux runtime, target agent, and Git server

**Files:**
- Create: `.dockerignore`
- Create: `tests/e2e/Dockerfile`
- Create: `tests/e2e/agent.toml`
- Create: `tests/e2e/target-entrypoint.sh`
- Create: `tests/e2e/git-entrypoint.sh`
- Modify: `tests/e2e/contracts.test.mjs`

**Interfaces:**
- Produces image tag supplied as `RPI_E2E_RUNTIME_IMAGE` with `/usr/local/bin/rpi`, Docker CLI + Compose plugin, Bash, OpenSSH, Git daemon, and curl.
- `target-entrypoint.sh` consumes `/run/e2e-public/id_ed25519.pub`, `/etc/rpi/agent.toml`, and `DOCKER_HOST=tcp://127.0.0.1:2375`.
- `git-entrypoint.sh` consumes `/opt/e2e/fixtures/app` and serves `/srv/git/fixture.git`.

- [ ] **Step 1: Extend the contract tests for runtime invariants**

Append to `tests/e2e/contracts.test.mjs`:

```js
test('runtime builds one current rpi binary and contains required target tools', async () => {
  const [dockerfile, agent, target, git] = await Promise.all([
    read('tests/e2e/Dockerfile'),
    read('tests/e2e/agent.toml'),
    read('tests/e2e/target-entrypoint.sh'),
    read('tests/e2e/git-entrypoint.sh'),
  ]);
  assert.match(dockerfile, /cargo build --locked -p pi/);
  assert.match(dockerfile, /COPY --from=builder \/out\/rpi \/usr\/local\/bin\/rpi/);
  assert.match(dockerfile, /FROM docker:28-cli AS docker_cli/);
  assert.match(dockerfile, /docker-compose/);
  assert.match(agent, /socket = "\/run\/rpi\/agent\.sock"/);
  assert.match(agent, /port_min = 18080/);
  assert.match(agent, /port_max = 18089/);
  assert.match(target, /runuser -u rpi-agent/);
  assert.match(target, /AllowStreamLocalForwarding=yes/);
  assert.match(git, /git daemon/);
  assert.match(git, /fixture\.git/);
});
```

- [ ] **Step 2: Run the tests and confirm the runtime files are missing**

Run:

```text
npm run test:node
```

Expected: FAIL with `ENOENT` for `tests/e2e/Dockerfile`.

- [ ] **Step 3: Add the Docker build context exclusions**

Create root `.dockerignore`:

```text
.git
.github
.superpowers
.worktrees
.claude/worktrees
target
dist
node_modules
docs
*.db
*.db-wal
*.db-shm
```

- [ ] **Step 4: Create the shared runtime image**

Create `tests/e2e/Dockerfile`:

```dockerfile
# syntax=docker/dockerfile:1.7
FROM rust:1.88-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --locked -p pi && \
    install -D -m 0755 target/debug/rpi /out/rpi

FROM docker:28-cli AS docker_cli

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
      bash ca-certificates curl git libgcc-s1 openssh-client openssh-server passwd util-linux && \
    rm -rf /var/lib/apt/lists/*
COPY --from=docker_cli /usr/local/bin/docker /usr/local/bin/docker
COPY --from=docker_cli /usr/local/libexec/docker/cli-plugins/docker-compose \
  /usr/local/libexec/docker/cli-plugins/docker-compose
COPY --from=builder /out/rpi /usr/local/bin/rpi

RUN groupadd --system rpi-agent && \
    useradd --system --gid rpi-agent --no-create-home --shell /usr/sbin/nologin rpi-agent && \
    useradd --create-home --shell /bin/bash deploy && \
    passwd --delete deploy && \
    usermod --append --groups rpi-agent deploy && \
    install -d -m 0755 /run/sshd /opt/e2e && \
    docker --version && docker compose version && rpi --version

COPY tests/e2e /opt/e2e
RUN sed -i 's/\r$//' /opt/e2e/*.sh && chmod 0755 /opt/e2e/*.sh
```

If the official `docker:28-cli` image moves the Compose plugin path, stop and verify the current official image layout with `docker run --rm docker:28-cli sh -lc 'find /usr/local -name docker-compose -type f'`; then update both the Dockerfile and its contract assertion to the returned path. Do not install a Python Compose v1 package.

- [ ] **Step 5: Create the deterministic agent configuration**

Create `tests/e2e/agent.toml`:

```toml
data_dir = "/var/lib/rpi"
socket = "/run/rpi/agent.sock"
port_min = 18080
port_max = 18089
build_concurrency = 1
history_keep = 10

[timeouts]
fetch = "60s"
build = "5m"
up = "60s"

[gc]
disk_threshold_percent = 95

[logs]
dir = "/var/log/rpi"
retention_days = 1
```

- [ ] **Step 6: Create the target process supervisor**

Create `tests/e2e/target-entrypoint.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

install -d -o rpi-agent -g rpi-agent -m 0750 /var/lib/rpi /var/log/rpi
install -d -o rpi-agent -g rpi-agent -m 0770 /run/rpi
install -d -o deploy -g deploy -m 0700 /home/deploy/.ssh
install -o deploy -g deploy -m 0600 \
  /run/e2e-public/id_ed25519.pub /home/deploy/.ssh/authorized_keys
ssh-keygen -A

runuser -u rpi-agent -- env \
  HOME=/var/lib/rpi \
  XDG_CONFIG_HOME=/var/lib/rpi/.config \
  XDG_CACHE_HOME=/var/lib/rpi/.cache \
  DOCKER_HOST=tcp://127.0.0.1:2375 \
  /usr/local/bin/rpi agent run --config /etc/rpi/agent.toml &
agent_pid=$!

/usr/sbin/sshd -D -e \
  -o PasswordAuthentication=no \
  -o PermitEmptyPasswords=no \
  -o KbdInteractiveAuthentication=no \
  -o PermitRootLogin=no \
  -o PubkeyAuthentication=yes \
  -o AllowTcpForwarding=yes \
  -o AllowStreamLocalForwarding=yes &
sshd_pid=$!

shutdown_children() {
  kill "$agent_pid" "$sshd_pid" 2>/dev/null || true
  wait "$agent_pid" "$sshd_pid" 2>/dev/null || true
}
trap shutdown_children EXIT
trap 'exit 143' TERM INT

set +e
wait -n "$agent_pid" "$sshd_pid"
status=$?
set -e
exit "$status"
```

- [ ] **Step 7: Create the Git fixture server**

Create `tests/e2e/git-entrypoint.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

rm -rf /srv/git/fixture.git /tmp/fixture-work
mkdir -p /srv/git
cp -a /opt/e2e/fixtures/app /tmp/fixture-work
cd /tmp/fixture-work
git init --initial-branch=main
git config user.name 'rpi e2e'
git config user.email 'rpi-e2e@example.invalid'
git add .
git commit -m 'fixture: initial app'
git clone --bare . /srv/git/fixture.git
exec git daemon --reuseaddr --verbose --export-all --base-path=/srv/git /srv/git
```

- [ ] **Step 8: Run Node contracts and build-smoke the runtime image**

Run:

```text
npm run test:node
docker build --file tests/e2e/Dockerfile --tag rpi-e2e-runtime:plan .
docker run --rm rpi-e2e-runtime:plan bash -lc "rpi --version && docker compose version && ssh -V && git --version"
```

Expected: Node tests PASS; image build succeeds; the smoke command prints the current `rpi` version, Docker Compose v2, OpenSSH, and Git versions.

- [ ] **Step 9: Commit the runtime**

```bash
git add .dockerignore tests/e2e/Dockerfile tests/e2e/agent.toml tests/e2e/target-entrypoint.sh tests/e2e/git-entrypoint.sh tests/e2e/contracts.test.mjs
git commit -m "test(e2e): add isolated client and target runtime"
```

---

### Task 4: Outer Compose topology and isolation contracts

**Files:**
- Create: `tests/e2e/compose.yaml`
- Modify: `tests/e2e/contracts.test.mjs`

**Interfaces:**
- Consumes runtime image contract from Task 3 and launcher service names from Task 1.
- Produces services `keygen`, `dind`, `target`, `git-fixture`, `client`.
- `target` shares `network_mode: service:dind`; the `dind` network alias `target` is the SSH hostname.
- `keygen` exits successfully before target starts; `docker compose up -d --wait dind target git-fixture` runs it through `service_completed_successfully`.

- [ ] **Step 1: Write failing isolation and topology contracts**

Append to `tests/e2e/contracts.test.mjs`:

```js
test('outer Compose isolates DinD and preserves the target loopback model', async () => {
  const compose = await read('tests/e2e/compose.yaml');
  assert.match(compose, /privileged: true/);
  assert.equal((compose.match(/privileged: true/g) || []).length, 1);
  assert.match(compose, /127\.0\.0\.1:2375/);
  assert.match(compose, /network_mode: service:dind/);
  assert.match(compose, /aliases:\s*\n\s*- target/);
  assert.match(compose, /condition: service_completed_successfully/);
  assert.match(compose, /condition: service_healthy/);
  assert.doesNotMatch(compose, /\/var\/run\/docker\.sock/);
  assert.doesNotMatch(compose, /^\s{4}ports:/m);
  const targetBlock = /^  target:\s*$([\s\S]*?)^  git-fixture:\s*$/m.exec(compose)?.[1] || '';
  assert.match(targetBlock, /ssh-public:\/run\/e2e-public:ro/);
  assert.doesNotMatch(targetBlock, /ssh-private/);
});

test('Compose service names match the launcher contract', async () => {
  const compose = await read('tests/e2e/compose.yaml');
  for (const service of ['keygen', 'dind', 'target', 'git-fixture', 'client']) {
    assert.match(compose, new RegExp(`^  ${service}:`, 'm'));
  }
});
```

- [ ] **Step 2: Run tests and confirm the Compose file is missing**

Run:

```text
npm run test:node
```

Expected: FAIL with `ENOENT` for `tests/e2e/compose.yaml`.

- [ ] **Step 3: Create the outer Compose graph**

Create `tests/e2e/compose.yaml`:

```yaml
x-runtime: &runtime
  image: ${RPI_E2E_RUNTIME_IMAGE:?RPI_E2E_RUNTIME_IMAGE must be set by run.mjs}
  build:
    context: ../..
    dockerfile: tests/e2e/Dockerfile

services:
  keygen:
    <<: *runtime
    network_mode: none
    command:
      - bash
      - -ceu
      - |
        rm -f /run/e2e-keys/id_ed25519 /run/e2e-keys/id_ed25519.pub
        rm -f /run/e2e-public/id_ed25519.pub
        ssh-keygen -q -t ed25519 -N '' -f /run/e2e-keys/id_ed25519
        chmod 0600 /run/e2e-keys/id_ed25519
        install -m 0644 /run/e2e-keys/id_ed25519.pub /run/e2e-public/id_ed25519.pub
    volumes:
      - ssh-private:/run/e2e-keys
      - ssh-public:/run/e2e-public

  dind:
    image: docker:28-dind
    privileged: true
    environment:
      DOCKER_TLS_CERTDIR: ""
      DOCKER_HOST: tcp://127.0.0.1:2375
    command:
      - --host=tcp://127.0.0.1:2375
    volumes:
      - dind-data:/var/lib/docker
    networks:
      e2e:
        aliases:
          - target
    healthcheck:
      test: ["CMD-SHELL", "docker info >/dev/null 2>&1"]
      interval: 2s
      timeout: 5s
      retries: 30
      start_period: 5s
    stop_grace_period: 10s

  target:
    <<: *runtime
    network_mode: service:dind
    depends_on:
      keygen:
        condition: service_completed_successfully
      dind:
        condition: service_healthy
    environment:
      DOCKER_HOST: tcp://127.0.0.1:2375
      RUST_LOG: info
    command: ["/opt/e2e/target-entrypoint.sh"]
    volumes:
      - ssh-public:/run/e2e-public:ro
      - agent-data:/var/lib/rpi
      - ./agent.toml:/etc/rpi/agent.toml:ro
    healthcheck:
      test:
        - CMD
        - bash
        - -ec
        - >-
          curl --fail --silent --unix-socket /run/rpi/agent.sock
          http://localhost/v1/version >/dev/null &&
          docker info >/dev/null &&
          exec 3<>/dev/tcp/127.0.0.1/22
      interval: 2s
      timeout: 5s
      retries: 30
      start_period: 5s
    stop_grace_period: 10s

  git-fixture:
    <<: *runtime
    command: ["/opt/e2e/git-entrypoint.sh"]
    networks:
      - e2e
    healthcheck:
      test: ["CMD-SHELL", "git ls-remote git://127.0.0.1/fixture.git HEAD >/dev/null 2>&1"]
      interval: 2s
      timeout: 5s
      retries: 30

  client:
    <<: *runtime
    depends_on:
      keygen:
        condition: service_completed_successfully
      target:
        condition: service_healthy
      git-fixture:
        condition: service_healthy
    environment:
      HOME: /tmp/rpi-e2e-home
      NO_COLOR: "1"
    working_dir: /opt/e2e/fixtures/app
    command: ["/opt/e2e/scenario.sh"]
    networks:
      - e2e
    volumes:
      - ssh-private:/run/e2e-keys:ro
      - type: bind
        source: ${RPI_E2E_ARTIFACT_DIR:?RPI_E2E_ARTIFACT_DIR must be set by run.mjs}
        target: /artifacts

networks:
  e2e:

volumes:
  agent-data:
  dind-data:
  ssh-private:
  ssh-public:
```

The e2e bridge intentionally keeps normal outbound connectivity so DinD can
pull `busybox:1.37`. Isolation comes from having no published ports, a
loopback-only Docker API, no production credentials, and no host Docker socket.

- [ ] **Step 4: Validate contracts and Compose rendering**

Run in PowerShell from the repository root:

```powershell
npm run test:node
$env:RPI_E2E_RUNTIME_IMAGE = 'rpi-e2e-runtime:plan'
$env:RPI_E2E_ARTIFACT_DIR = Join-Path (Get-Location) 'target\e2e-artifacts\config'
docker compose --file tests/e2e/compose.yaml config --quiet
```

Expected: Node tests PASS and `docker compose config --quiet` exits 0 with no output. Search the rendered config:

```powershell
docker compose --file tests/e2e/compose.yaml config | Select-String '/var/run/docker.sock|published:'
```

Expected: no matches.

- [ ] **Step 5: Commit the Compose topology**

```bash
git add tests/e2e/compose.yaml tests/e2e/contracts.test.mjs
git commit -m "test(e2e): define isolated DinD deployment topology"
```

---

### Task 5: Production-path client scenario and full local e2e

**Files:**
- Create: `tests/e2e/scenario.sh`
- Modify: `tests/e2e/contracts.test.mjs`

**Interfaces:**
- Consumes `target` hostname, `deploy` user, key `/run/e2e-keys/id_ed25519`, fixture working directory, and artifacts bind `/artifacts`.
- Produces `deploy-1.log`, `ls-1.log`, `deploy-2.log`, `ls-2.log`, `rm.log`, and `ls-after-rm.log`.
- Success exit code is 0 only after two deploys, stable port assertion, independent HTTP check, `rpi rm`, and no nested fixture containers.

- [ ] **Step 1: Write the failing scenario contract**

Append to `tests/e2e/contracts.test.mjs`:

```js
test('scenario uses the production SSH path and covers deploy, redeploy, and remove', async () => {
  const scenario = await read('tests/e2e/scenario.sh');
  assert.match(scenario, /unset PI_AGENT_URL/);
  assert.doesNotMatch(scenario, /PI_AGENT_URL=/);
  assert.equal((scenario.match(/rpi deploy/g) || []).length, 2);
  assert.match(scenario, /rpi ls/);
  assert.match(scenario, /127\.0\.0\.1:18080\/health/);
  assert.match(scenario, /rpi rm e2e-fixture --yes/);
  assert.match(scenario, /com\.docker\.compose\.project=e2e-fixture/);
  assert.match(scenario, /env DOCKER_HOST=tcp:\/\/127\.0\.0\.1:2375 docker ps/);
  for (const milestone of [
    'fetched ',
    'docker compose build ...',
    'docker compose up -d ...',
    'healthcheck: passed',
  ]) {
    assert.match(scenario, new RegExp(milestone.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
  }
});
```

- [ ] **Step 2: Run tests and confirm the scenario file is missing**

Run:

```text
npm run test:node
```

Expected: FAIL with `ENOENT` for `tests/e2e/scenario.sh`.

- [ ] **Step 3: Implement the full scenario**

Create `tests/e2e/scenario.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

ARTIFACTS=/artifacts
KEY=/run/e2e-keys/id_ed25519
CONNECT=(--host target --user deploy --key "$KEY")
SSH=(ssh -i "$KEY" -o BatchMode=yes -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes deploy@target)

fail() {
  echo "rpi e2e: $*" >&2
  exit 1
}

run_capture() {
  local file=$1
  shift
  set +e
  "$@" 2>&1 | tee "$ARTIFACTS/$file"
  local status=${PIPESTATUS[0]}
  set -e
  [[ $status -eq 0 ]] || fail "$file command exited with $status"
}

assert_log() {
  local file=$1
  local text=$2
  grep -F -- "$text" "$ARTIFACTS/$file" >/dev/null || \
    fail "$file does not contain: $text"
}

assert_deploy_log() {
  local file=$1
  assert_log "$file" 'fetched '
  assert_log "$file" 'docker compose build ...'
  assert_log "$file" 'docker compose up -d ...'
  assert_log "$file" 'healthcheck: passed'
}

mkdir -p "$ARTIFACTS" "$HOME/.ssh"
chmod 0700 "$HOME/.ssh"
[[ $(stat -c '%a' "$KEY") == '600' ]] || fail 'private key mode is not 0600'
unset PI_AGENT_URL

for _ in $(seq 1 30); do
  if ssh-keyscan -H target >"$HOME/.ssh/known_hosts.tmp" 2>/dev/null; then
    mv "$HOME/.ssh/known_hosts.tmp" "$HOME/.ssh/known_hosts"
    break
  fi
  sleep 1
done
[[ -s "$HOME/.ssh/known_hosts" ]] || fail 'could not record target SSH host key'
"${SSH[@]}" true

rpi --version
run_capture deploy-1.log rpi deploy "${CONNECT[@]}"
assert_deploy_log deploy-1.log

run_capture ls-1.log rpi ls "${CONNECT[@]}"
assert_log ls-1.log 'e2e-fixture'
assert_log ls-1.log '18080'
assert_log ls-1.log 'web:running'
health=$("${SSH[@]}" curl -fsS http://127.0.0.1:18080/health)
[[ $health == 'ok' ]] || fail "unexpected first health body: $health"

run_capture deploy-2.log rpi deploy "${CONNECT[@]}"
assert_deploy_log deploy-2.log

run_capture ls-2.log rpi ls "${CONNECT[@]}"
assert_log ls-2.log 'e2e-fixture'
assert_log ls-2.log '18080'
assert_log ls-2.log 'web:running'
health=$("${SSH[@]}" curl -fsS http://127.0.0.1:18080/health)
[[ $health == 'ok' ]] || fail "unexpected second health body: $health"

run_capture rm.log rpi rm e2e-fixture --yes "${CONNECT[@]}"
assert_log rm.log "project 'e2e-fixture' removed"

run_capture ls-after-rm.log rpi ls "${CONNECT[@]}"
assert_log ls-after-rm.log 'no projects deployed yet'
if "${SSH[@]}" curl -fsS http://127.0.0.1:18080/health >/dev/null 2>&1; then
  fail 'health endpoint still reachable after rpi rm'
fi
leftovers=$("${SSH[@]}" env DOCKER_HOST=tcp://127.0.0.1:2375 docker ps -aq \
  --filter label=com.docker.compose.project=e2e-fixture)
[[ -z $leftovers ]] || fail "fixture containers remain after rpi rm: $leftovers"

echo 'rpi e2e: PASS'
```

- [ ] **Step 4: Rebuild-smoke Bash syntax inside the runtime image**

Run:

```text
docker build --file tests/e2e/Dockerfile --tag rpi-e2e-runtime:plan .
docker run --rm rpi-e2e-runtime:plan bash -n /opt/e2e/scenario.sh
npm run test:node
```

Expected: Docker build exits 0; `bash -n` exits 0 with no output; all Node tests PASS.

- [ ] **Step 5: Run the complete local e2e**

Run:

```text
npm run test:e2e
```

Expected final scenario output includes:

```text
rpi e2e: PASS
```

Expected exit code: 0. The artifact directory created under
`target/e2e-artifacts/<run-id>` contains the six scenario logs plus
build/dependency/cleanup logs.

- [ ] **Step 6: Verify outer cleanup**

Run in PowerShell:

```powershell
docker ps -a --filter 'name=rpi-e2e-' --format '{{.Names}}'
docker volume ls --filter 'name=rpi-e2e-' --format '{{.Name}}'
docker network ls --filter 'name=rpi-e2e-' --format '{{.Name}}'
```

Expected: all three commands produce no rows.

- [ ] **Step 7: Commit the scenario**

```bash
git add tests/e2e/scenario.sh tests/e2e/contracts.test.mjs
git commit -m "test(e2e): exercise SSH deploy redeploy and removal"
```

---

### Task 6: GitHub Actions merge gate and BuildKit cache

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `tests/e2e/contracts.test.mjs`

**Interfaces:**
- Consumes package scripts from Task 1 and `tests/e2e/Dockerfile` from Task 3.
- Produces an `e2e` job on `ubuntu-latest`, after `linux`, with prebuilt image `rpi-e2e-runtime:ci` and artifacts under `${{ runner.temp }}/rpi-e2e`.

- [ ] **Step 1: Write the failing CI contract test**

Append to `tests/e2e/contracts.test.mjs`:

```js
test('CI runs the e2e gate with Buildx cache and failure-only artifacts', async () => {
  const workflow = await read('.github/workflows/ci.yml');
  assert.match(workflow, /branches: \[master\]/);
  assert.match(workflow, /^  pull_request:\s*$/m);
  assert.match(workflow, /permissions:\s*\n  contents: read/);
  assert.match(workflow, /^  e2e:\s*$/m);
  assert.match(workflow, /needs: linux/);
  assert.match(workflow, /runs-on: ubuntu-latest/);
  assert.match(workflow, /timeout-minutes: 30/);
  assert.match(workflow, /docker\/setup-buildx-action@v3/);
  assert.match(workflow, /docker\/build-push-action@v6/);
  assert.match(workflow, /cache-from: type=gha,scope=rpi-e2e/);
  assert.match(workflow, /cache-to: type=gha,mode=max,scope=rpi-e2e,ignore-error=true/);
  assert.match(workflow, /RPI_E2E_PREBUILT: "1"/);
  assert.match(workflow, /npm run test:e2e/);
  assert.match(workflow, /if: failure\(\)/);
  assert.match(workflow, /actions\/upload-artifact@v6/);
  assert.doesNotMatch(workflow, /runs-on: self-hosted/);
});
```

- [ ] **Step 2: Run the test and verify the e2e job is absent**

Run:

```text
npm run test:node
```

Expected: FAIL because `.github/workflows/ci.yml` has no `e2e` job.

- [ ] **Step 3: Route existing Node tests through the package script**

In both existing `linux` and `windows` jobs, replace:

```yaml
- run: node --test "scripts/**/*.test.js"
```

with:

```yaml
- run: npm run test:node
```

- [ ] **Step 4: Add the cached Ubuntu e2e job**

Append under `jobs:` in `.github/workflows/ci.yml`:

```yaml
  e2e:
    needs: linux
    runs-on: ubuntu-latest
    timeout-minutes: 30
    env:
      RPI_E2E_RUNTIME_IMAGE: rpi-e2e-runtime:ci
      RPI_E2E_PREBUILT: "1"
      RPI_E2E_ARTIFACT_DIR: ${{ runner.temp }}/rpi-e2e
    steps:
      - uses: actions/checkout@v6
      - uses: actions/setup-node@v6
        with:
          node-version: 22
      - uses: docker/setup-buildx-action@v3
      - name: Build cached e2e runtime
        uses: docker/build-push-action@v6
        with:
          context: .
          file: tests/e2e/Dockerfile
          load: true
          tags: rpi-e2e-runtime:ci
          cache-from: type=gha,scope=rpi-e2e
          cache-to: type=gha,mode=max,scope=rpi-e2e,ignore-error=true
      - name: Production-path Docker e2e
        run: npm run test:e2e
      - name: Upload e2e diagnostics
        if: failure()
        uses: actions/upload-artifact@v6
        with:
          name: rpi-e2e-${{ github.run_id }}-${{ github.run_attempt }}
          path: ${{ runner.temp }}/rpi-e2e
          if-no-files-found: ignore
```

Keep the workflow-level permissions exactly:

```yaml
permissions:
  contents: read
```

- [ ] **Step 5: Run local workflow contracts and existing checks**

Run:

```text
npm run test:node
cargo fmt --all -- --check
cargo test --locked
```

Expected: all commands PASS. Confirm the full e2e still passes with the locally built path:

```text
npm run test:e2e
```

Expected: `rpi e2e: PASS` and exit 0.

- [ ] **Step 6: Commit the CI gate**

```bash
git add .github/workflows/ci.yml tests/e2e/contracts.test.mjs
git commit -m "ci: gate pull requests on production-path Docker e2e"
```

---

### Task 7: Developer documentation and final verification

**Files:**
- Modify: `README.md`
- Modify: `docs/ci-github-actions.md`
- Verify only: `.gitignore`

**Interfaces:**
- Documents the exact command `npm run test:e2e`, Compose floor `2.33.1`, privileged DinD warning, GitHub-hosted-only CI boundary, and artifact paths.

- [ ] **Step 1: Add the README development section**

Under `## Development`, immediately after the existing `cargo test --workspace` block, add:

````markdown
### Full Docker end-to-end test

The production-path e2e test builds the current `rpi` once, starts an isolated
target with real SSH and `/run/rpi/agent.sock`, and deploys a local Git fixture
into a dedicated Docker-in-Docker daemon:

```bash
npm run test:e2e
```

Requirements: Node.js 18+, Docker Desktop using Linux containers (or Docker
Engine on Linux), Docker Compose 2.33.1+, and support for privileged Linux
containers. The command starts a **privileged** `docker:28-dind` service, but it
does not mount the host Docker socket or publish test ports to the host.

The scenario covers `rpi deploy` over SSH, the agent Unix socket, real Compose
build/up, HTTP health, a stable second deploy, `rpi ls`, and `rpi rm`. It does
not cover systemd installation, Cloudflare, secrets, private Git, or ARM.

On failure, inspect `target/e2e-artifacts/<run-id>`. The launcher records build,
outer Compose, agent, nested Docker, scenario, and cleanup diagnostics before it
removes the run's containers, networks, and volumes.
````

Use four backticks around the outer plan snippet when applying it so the inner
fenced command renders correctly; the resulting README must use normal Markdown
with one heading and one `bash` code fence.

- [ ] **Step 2: Document the repository merge gate**

Add this section near the top of `docs/ci-github-actions.md`, before
`## Repository Secrets`:

```markdown
## This repository's Docker e2e merge gate

The `e2e` job in `.github/workflows/ci.yml` runs after the Linux format, clippy,
and unit-test job on every pull request and push to `master`. It prebuilds the
shared runtime with Docker Buildx, uses the GitHub Actions cache backend v2, and
runs `npm run test:e2e` on `ubuntu-latest` with a 30-minute job timeout.

The job intentionally has only `contents: read`, consumes no repository or
deployment secrets, and uploads `${{ runner.temp }}/rpi-e2e` only when the job
fails. The DinD service is privileged, so this job must remain on a disposable
GitHub-hosted runner; do not copy it to `self-hosted` without a separate threat
review.

The cache is an optimization, not a correctness dependency. Cache export uses
`ignore-error=true`, and a cold runner must still build and pass.
```

- [ ] **Step 3: Verify ignores and documentation strings**

Run:

```text
rg -n "npm run test:e2e|Compose 2.33.1|privileged|e2e-artifacts" README.md docs/ci-github-actions.md
rg -n "^\.superpowers$" .gitignore
```

Expected: README and CI docs contain all four required concepts; `.gitignore`
contains exactly the `.superpowers` ignore entry already present before this
feature.

- [ ] **Step 4: Run the complete verification matrix**

Run focused/static checks first:

```text
npm run test:node
node scripts/check-version.js
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
git diff --check
```

Expected: every command exits 0.

Then run the real container gate:

```text
npm run test:e2e
```

Expected: `rpi e2e: PASS`, exit 0, no `rpi-e2e-*` containers, networks, or
volumes, and artifacts under `target/e2e-artifacts/<run-id>`.

- [ ] **Step 5: Review the final diff against the design**

Run:

```text
git status --short
git diff --stat
git diff -- . ':!docs/superpowers/plans/2026-07-10-docker-e2e-deployment.md'
```

Expected changed implementation files are only the paths listed in this plan.
There must be no production Rust diff and no host Docker socket reference:

```text
rg -n "/var/run/docker.sock|runs-on: self-hosted" tests/e2e .github/workflows/ci.yml
```

Expected: no matches.

- [ ] **Step 6: Commit documentation**

```bash
git add README.md docs/ci-github-actions.md
git commit -m "docs: explain production-path Docker e2e"
```

- [ ] **Step 7: Request review before integration**

Invoke `superpowers:requesting-code-review` with the design, this plan, the full
diff, and the verification evidence. Address review findings through
`superpowers:receiving-code-review`, rerun the affected focused tests, then rerun
the full verification matrix before claiming completion.
