# E2E Parallel Scenarios Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restructure the Docker e2e harness into a `scenarios/<name>/` layout where each scenario is a self-contained folder running as its own isolated compose stack, executed in parallel under a bounded worker pool.

**Architecture:** One shared `base.compose.yaml` (keygen, dind, target, git-fixture, client) is instantiated N times as independent compose projects; `RPI_E2E_SCENARIO` selects which baked-in scenario folder each stack serves/runs. `run.mjs` discovers scenario folders, builds the runtime image once, and drives stacks through a worker pool with a summary report. Client-side assertion helpers live in a shared `lib.sh`.

**Tech Stack:** Node >= 18 (`node --test`, no deps), Docker Compose >= 2.33.1, bash, Docker-in-Docker, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-07-10-e2e-parallel-scenarios-harness-design.md`

## Global Constraints

- Runner code must stay Node 18-compatible (package.json `engines: >=18`); no new npm dependencies.
- Docker Compose version floor stays `2.33.1` (`MIN_COMPOSE`).
- Default concurrency **2**; CI sets `RPI_E2E_CONCURRENCY: "3"`.
- Scenario contract: `scenario.sh` + `app/` required; `agent.toml`, `compose.override.yaml`, `meta.env` optional. Scenario folder names must be valid in compose project names: `^[a-z0-9][a-z0-9-]*$`.
- Fixture repo URL is stable across scenarios: `git://git-fixture/fixture.git`.
- A per-scenario `agent.toml` must keep `socket = "/run/rpi/agent.sock"` (the target healthcheck pins it) and sshd on port 22.
- Env contract preserved: `RPI_E2E_PREBUILT`, `RPI_E2E_RUNTIME_IMAGE`, `RPI_E2E_ARTIFACT_DIR`, `RPI_E2E_KEEP`. New: `RPI_E2E_CONCURRENCY`, `RPI_E2E_SCENARIO` (set by the runner, not by users).
- npm script names unchanged: `test:e2e`, `e2e:dev:up`, `e2e:dev:down`.
- All shell files must be LF (`.gitattributes`); the dev machine is Windows — never rely on checkout EOL.
- Shell commands in this repo are prefixed with `rtk` (CLAUDE.md golden rule).
- Exit-code semantics preserved per scenario: 124 timeout, 130 aborted, teardown failure fails a green scenario but never masks a red one. New: exit 2 for unknown scenario names / bad CLI flags.

## File Structure (end state)

```
tests/e2e/
  base.compose.yaml            # renamed from compose.yaml; scenario-parametrized
  agent.default.toml           # renamed from agent.toml; shared default agent config
  Dockerfile                   # *.sh normalization made recursive
  run.mjs                      # + discovery, pool, summary, per-scenario stacks
  run.test.mjs                 # unit tests for the runner (fake docker)
  contracts.test.mjs           # structural contract tests (rewritten)
  lib.sh                       # shared client bootstrap + assertion library
  entrypoints/
    git-entrypoint.sh          # serves scenarios/$RPI_E2E_SCENARIO/app
    target-entrypoint.sh       # resolves per-scenario agent config
  scenarios/
    happy-path/
      app/                     # moved from tests/e2e/fixtures/app
        Dockerfile
        compose.yaml
        rpi.toml
        health
      scenario.sh              # moved from tests/e2e/scenario.sh, trimmed
.gitattributes                 # LF rules for new paths
.github/workflows/ci.yml       # + RPI_E2E_CONCURRENCY: "3"
```

Task order keeps every commit green: Task 1 adds pure runner utilities (additive), Task 2 extracts the client library (behavior-neutral), Task 3 flips the layout while keeping the runner single-scenario, Task 4 turns the runner multi-scenario/parallel, Task 5 wires CI and verifies end-to-end.

---

### Task 1: Runner pure utilities (discovery, pool, summary, meta, args)

**Files:**
- Modify: `tests/e2e/run.mjs` (add imports + constants + 6 exported functions; nothing existing changes)
- Test: `tests/e2e/run.test.mjs` (append tests; extend imports)

**Interfaces:**
- Consumes: nothing new (pure additions).
- Produces (Task 4 consumes exactly these):
  - `discoverScenarios(dir?: string): Promise<string[]>` — sorted folder names containing `scenario.sh`; `[]` for missing dir.
  - `runPool(items, limit, worker, { stopOn }?): Promise<results[]>` — bounded pool; `results[i]` is `worker(items[i], i)`'s value; after `stopOn(result)` returns true no new items are dispatched (in-flight finish; undispatched slots stay `undefined`).
  - `formatSummary(results: {scenario, code?, durationMs?, timedOut?, skipped?}[]): string`.
  - `parseMetaEnv(text: string): Record<string,string>`.
  - `scenarioTimeoutMs(scenario: string, dir?: string): Promise<number>` — `RPI_E2E_TIMEOUT` (integer seconds) from `meta.env`, else `15 * 60 * 1000`.
  - `parseRunArgs(args: string[]): { scenarios: string[], failFast: boolean, concurrency?: number }` — throws on unknown flags / bad `--concurrency`.

- [ ] **Step 1: Write the failing tests**

In `tests/e2e/run.test.mjs`, replace the import block at the top with:

```js
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
```

Append at the end of the file:

```js
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk npm run test:node -- tests/e2e/run.test.mjs`
Expected: FAIL — `SyntaxError: The requested module './run.mjs' does not provide an export named 'discoverScenarios'`.

- [ ] **Step 3: Implement the utilities in `tests/e2e/run.mjs`**

Replace the import block and constants at the top of `tests/e2e/run.mjs` with:

```js
import { randomBytes } from 'node:crypto';
import { spawn } from 'node:child_process';
import { createWriteStream, existsSync } from 'node:fs';
import { mkdir, readdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, '..', '..');
const COMPOSE_FILE = path.join('tests', 'e2e', 'compose.yaml');
const SCENARIOS_DIR = path.join(HERE, 'scenarios');
const MIN_COMPOSE = [2, 33, 1];
const BUILD_TIMEOUT_MS = 30 * 60 * 1000;
const SCENARIO_TIMEOUT_MS = 15 * 60 * 1000;
const DEFAULT_CONCURRENCY = 2;
```

(Note: `COMPOSE_FILE` still points at `compose.yaml` — Task 3 renames it. `DEFAULT_CONCURRENCY` is used in Task 4.)

Insert after `makeProjectName` (before `spawnDocker`):

```js
/** Scenario folders under tests/e2e/scenarios that contain a scenario.sh. */
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
    if (existsSync(path.join(dir, entry.name, 'scenario.sh'))) names.push(entry.name);
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
```

`DEFAULT_CONCURRENCY` and `SCENARIOS_DIR` are intentionally unreferenced by old code until Task 4; that is fine for ESM (no lint step on JS in this repo).

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk npm run test:node -- tests/e2e/run.test.mjs`
Expected: PASS — all existing tests plus the 8 new ones.

- [ ] **Step 5: Commit**

```bash
rtk git add tests/e2e/run.mjs tests/e2e/run.test.mjs
rtk git commit -m "test(e2e): add scenario discovery, worker pool, and summary utilities"
```

---

### Task 2: Shared client library (`lib.sh`) + trimmed scenario script

**Files:**
- Modify: `tests/e2e/lib.sh` (add `fail`, `e2e_bootstrap`, `run_capture`, `assert_log`, `assert_deploy_log`)
- Modify: `tests/e2e/scenario.sh` (still at the old path; prologue replaced by `e2e_bootstrap`)
- Test: `tests/e2e/contracts.test.mjs` (update the scenario/lib test)

**Interfaces:**
- Consumes: existing `e2e_client_init` in `lib.sh`.
- Produces (every scenario script consumes): bash functions `fail <msg>`, `e2e_bootstrap` (sets globals `ARTIFACTS=/artifacts`, `KEY`, arrays `CONNECT`, `SSH`), `run_capture <artifact-file> <cmd...>`, `assert_log <file> <text>`, `assert_deploy_log <file>`.

- [ ] **Step 1: Update the contract test to demand the shared library**

In `tests/e2e/contracts.test.mjs`, replace the whole test `'scenario uses the production SSH path and covers deploy, redeploy, and remove'` with:

```js
test('scenario drives deploy, redeploy, and remove through the shared library', async () => {
  const [scenario, lib] = await Promise.all([
    read('tests/e2e/scenario.sh'),
    read('tests/e2e/lib.sh'),
  ]);
  assert.match(scenario, /^source \/opt\/e2e\/lib\.sh$/m);
  assert.match(scenario, /^e2e_bootstrap$/m);
  for (const helper of [
    'fail()',
    'e2e_client_init()',
    'e2e_bootstrap()',
    'run_capture()',
    'assert_log()',
    'assert_deploy_log()',
  ]) {
    assert.ok(lib.includes(helper), `lib.sh defines ${helper}`);
  }
  assert.match(lib, /unset PI_AGENT_URL/);
  assert.match(lib, /ssh-keyscan -H target/);
  assert.match(lib, /\/etc\/ssh\/ssh_known_hosts/);
  assert.doesNotMatch(scenario, /PI_AGENT_URL=/);
  assert.doesNotMatch(scenario, /\$HOME\/\.ssh/);
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
    assert.match(lib, new RegExp(milestone.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
  }
});
```

- [ ] **Step 2: Run the contract tests to verify they fail**

Run: `rtk npm run test:node -- tests/e2e/contracts.test.mjs`
Expected: FAIL — `lib.sh defines fail()` (helpers are still inlined in scenario.sh).

- [ ] **Step 3: Rewrite `tests/e2e/lib.sh` (full content)**

```bash
#!/usr/bin/env bash
# Shared client library, sourced by scenario scripts and by interactive dev
# shells. OpenSSH resolves `~` through the passwd database (pw_dir), not
# $HOME, and the rpi CLI spawns plain `ssh` with no way to pass
# -o UserKnownHostsFile — so the target host key must be recorded in the
# system-wide /etc/ssh/ssh_known_hosts to cover both ssh paths.

E2E_KEY=/run/e2e-keys/id_ed25519

fail() {
  echo "rpi e2e: $*" >&2
  exit 1
}

e2e_client_init() {
  if [[ $(stat -c '%a' "$E2E_KEY") != '600' ]]; then
    echo 'rpi e2e: private key mode is not 0600' >&2
    return 1
  fi
  unset PI_AGENT_URL
  local tmp=/etc/ssh/ssh_known_hosts.tmp
  for _ in $(seq 1 30); do
    if ssh-keyscan -H target >"$tmp" 2>/dev/null && [[ -s $tmp ]]; then
      mv "$tmp" /etc/ssh/ssh_known_hosts
      return 0
    fi
    sleep 1
  done
  echo 'rpi e2e: could not record target SSH host key' >&2
  return 1
}

# Standard prologue for every scenario: shared globals (ARTIFACTS, KEY,
# CONNECT, SSH), the artifacts dir, the recorded target host key, and a
# proven SSH path. Arrays assigned here are global to the sourcing script.
e2e_bootstrap() {
  ARTIFACTS=/artifacts
  KEY=$E2E_KEY
  CONNECT=(--host target --user deploy --key "$KEY")
  SSH=(ssh -i "$KEY" -o BatchMode=yes -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes deploy@target)
  mkdir -p "$ARTIFACTS"
  e2e_client_init || fail 'client init failed'
  "${SSH[@]}" true
}

# run_capture <artifact-file> <cmd...> — run, tee output into the artifact,
# fail the scenario when the command exits nonzero.
run_capture() {
  local file=$1
  shift
  set +e
  "$@" 2>&1 | tee "$ARTIFACTS/$file"
  local status=${PIPESTATUS[0]}
  set -e
  [[ $status -eq 0 ]] || fail "$file command exited with $status"
}

# assert_log <artifact-file> <text> — literal substring match.
assert_log() {
  local file=$1
  local text=$2
  grep -F -- "$text" "$ARTIFACTS/$file" >/dev/null || \
    fail "$file does not contain: $text"
}

# assert_deploy_log <artifact-file> — the four deploy milestones every
# successful `rpi deploy` prints.
assert_deploy_log() {
  local file=$1
  assert_log "$file" 'fetched '
  assert_log "$file" 'docker compose build ...'
  assert_log "$file" 'docker compose up -d ...'
  assert_log "$file" 'healthcheck: passed'
}
```

- [ ] **Step 4: Rewrite `tests/e2e/scenario.sh` (full content)**

```bash
#!/usr/bin/env bash
set -euo pipefail

source /opt/e2e/lib.sh
e2e_bootstrap

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

- [ ] **Step 5: Run the node tests to verify they pass**

Run: `rtk npm run test:node`
Expected: PASS (contracts + runner tests).

- [ ] **Step 6: Commit**

```bash
rtk git add tests/e2e/lib.sh tests/e2e/scenario.sh tests/e2e/contracts.test.mjs
rtk git commit -m "test(e2e): extract shared client assertion library"
```

---
### Task 3: Layout flip — `scenarios/` folders, parametrized base stack

**Files:**
- Rename: `tests/e2e/compose.yaml` → `tests/e2e/base.compose.yaml` (then parametrize)
- Rename: `tests/e2e/agent.toml` → `tests/e2e/agent.default.toml` (content unchanged)
- Rename: `tests/e2e/git-entrypoint.sh` → `tests/e2e/entrypoints/git-entrypoint.sh` (then scenario-aware)
- Rename: `tests/e2e/target-entrypoint.sh` → `tests/e2e/entrypoints/target-entrypoint.sh` (then scenario-aware)
- Rename: `tests/e2e/scenario.sh` → `tests/e2e/scenarios/happy-path/scenario.sh` (content unchanged from Task 2)
- Rename: `tests/e2e/fixtures/app/` → `tests/e2e/scenarios/happy-path/app/` (content unchanged)
- Modify: `tests/e2e/Dockerfile` (recursive `*.sh` normalization)
- Modify: `.gitattributes`
- Modify: `tests/e2e/run.mjs` (3 small compat edits — stays single-scenario)
- Modify: `tests/e2e/run.test.mjs` (2 regex updates)
- Test: `tests/e2e/contracts.test.mjs` (full rewrite)

**Interfaces:**
- Consumes: `lib.sh` helpers from Task 2.
- Produces (Task 4 consumes): the layout contract — `base.compose.yaml` requiring `RPI_E2E_SCENARIO` (compose `${RPI_E2E_SCENARIO:?…}` guards), entrypoints under `/opt/e2e/entrypoints/`, scenario assets under `/opt/e2e/scenarios/<name>/`, default agent config at `/opt/e2e/agent.default.toml`.

- [ ] **Step 1: Rewrite `tests/e2e/contracts.test.mjs` (full content — failing first)**

```js
import test from 'node:test';
import assert from 'node:assert/strict';
import { access, readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, '..', '..');
const read = (relative) => readFile(path.join(ROOT, relative), 'utf8');

test('happy-path satisfies the scenario folder contract', async () => {
  await access(path.join(ROOT, 'tests/e2e/scenarios/happy-path/scenario.sh'));
  await access(path.join(ROOT, 'tests/e2e/scenarios/happy-path/app/rpi.toml'));
});

test('happy-path fixture uses local Git, managed port allocation, HTTP fallback, and LF content', async () => {
  const [attributes, config, compose, dockerfile, health] = await Promise.all([
    read('.gitattributes'),
    read('tests/e2e/scenarios/happy-path/app/rpi.toml'),
    read('tests/e2e/scenarios/happy-path/app/compose.yaml'),
    read('tests/e2e/scenarios/happy-path/app/Dockerfile'),
    read('tests/e2e/scenarios/happy-path/app/health'),
  ]);
  assert.match(attributes, /tests\/e2e\/\*\*\/\*\.sh text eol=lf/);
  assert.match(attributes, /tests\/e2e\/scenarios\/\*\/app\/health text eol=lf/);
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

test('runtime builds one current rpi binary, ships target tools, and normalizes nested scripts', async () => {
  const [dockerfile, agent, target, git] = await Promise.all([
    read('tests/e2e/Dockerfile'),
    read('tests/e2e/agent.default.toml'),
    read('tests/e2e/entrypoints/target-entrypoint.sh'),
    read('tests/e2e/entrypoints/git-entrypoint.sh'),
  ]);
  assert.match(dockerfile, /cargo build --locked -p pi/);
  assert.match(dockerfile, /COPY --from=builder \/out\/rpi \/usr\/local\/bin\/rpi/);
  assert.match(dockerfile, /FROM docker:28-cli AS docker_cli/);
  assert.match(dockerfile, /docker-compose/);
  assert.match(dockerfile, /find \/opt\/e2e -name '\*\.sh'/);
  assert.match(agent, /socket = "\/run\/rpi\/agent\.sock"/);
  assert.match(agent, /port_min = 18080/);
  assert.match(agent, /port_max = 18089/);
  assert.match(target, /runuser -u rpi-agent/);
  assert.match(target, /AllowStreamLocalForwarding=yes/);
  assert.match(target, /RPI_E2E_SCENARIO/);
  assert.match(target, /agent\.default\.toml/);
  assert.match(git, /RPI_E2E_SCENARIO/);
  assert.match(git, /scenarios\/\$SCENARIO\/app/);
  assert.match(git, /git daemon/);
  assert.match(git, /fixture\.git/);
});

test('base compose isolates DinD, keeps the loopback model, and is scenario-parametrized', async () => {
  const compose = await read('tests/e2e/base.compose.yaml');
  assert.match(compose, /privileged: true/);
  assert.equal((compose.match(/privileged: true/g) || []).length, 1);
  assert.match(compose, /127\.0\.0\.1:2375/);
  assert.match(compose, /network_mode: service:dind/);
  assert.match(compose, /aliases:\s*\n\s*- target/);
  assert.match(compose, /condition: service_completed_successfully/);
  assert.match(compose, /condition: service_healthy/);
  assert.doesNotMatch(compose, /\/var\/run\/docker\.sock/);
  assert.doesNotMatch(compose, /^\s{4}ports:/m);
  assert.match(compose, /RPI_E2E_SCENARIO: \$\{RPI_E2E_SCENARIO:\?/);
  assert.match(compose, /\/opt\/e2e\/entrypoints\/target-entrypoint\.sh/);
  assert.match(compose, /\/opt\/e2e\/entrypoints\/git-entrypoint\.sh/);
  assert.match(compose, /\/opt\/e2e\/scenarios\/\$\{RPI_E2E_SCENARIO\}\/scenario\.sh/);
  assert.match(compose, /working_dir: \/opt\/e2e\/scenarios\/\$\{RPI_E2E_SCENARIO\}\/app/);
  assert.doesNotMatch(compose, /\.\/agent\.toml/, 'agent config is baked, not bind-mounted');
  const targetBlock = /^  target:\s*$([\s\S]*?)^  git-fixture:\s*$/m.exec(compose)?.[1] || '';
  assert.match(targetBlock, /ssh-public:\/run\/e2e-public:ro/);
  assert.doesNotMatch(targetBlock, /ssh-private/);
  const dindBlock = /^  dind:\s*$([\s\S]*?)^  target:\s*$/m.exec(compose)?.[1] || '';
  assert.notEqual(dindBlock, '', 'dind service block must be present');
  assert.match(dindBlock, /command:\s*\["dockerd", "--host=tcp:\/\/127\.0\.0\.1:2375"\]/);
  assert.doesNotMatch(dindBlock, /0\.0\.0\.0:2375/);
});

test('compose service names match the launcher contract', async () => {
  const compose = await read('tests/e2e/base.compose.yaml');
  for (const service of ['keygen', 'dind', 'target', 'git-fixture', 'client']) {
    assert.match(compose, new RegExp(`^  ${service}:`, 'm'));
  }
});

test('happy-path scenario drives deploy, redeploy, and remove through the shared library', async () => {
  const [scenario, lib] = await Promise.all([
    read('tests/e2e/scenarios/happy-path/scenario.sh'),
    read('tests/e2e/lib.sh'),
  ]);
  assert.match(scenario, /^source \/opt\/e2e\/lib\.sh$/m);
  assert.match(scenario, /^e2e_bootstrap$/m);
  for (const helper of [
    'fail()',
    'e2e_client_init()',
    'e2e_bootstrap()',
    'run_capture()',
    'assert_log()',
    'assert_deploy_log()',
  ]) {
    assert.ok(lib.includes(helper), `lib.sh defines ${helper}`);
  }
  assert.match(lib, /unset PI_AGENT_URL/);
  assert.match(lib, /ssh-keyscan -H target/);
  assert.match(lib, /\/etc\/ssh\/ssh_known_hosts/);
  assert.doesNotMatch(scenario, /PI_AGENT_URL=/);
  assert.doesNotMatch(scenario, /\$HOME\/\.ssh/);
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
    assert.match(lib, new RegExp(milestone.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
  }
});

test('dev profile provides an exec-able client and stays out of the CI path', async () => {
  const compose = await read('tests/e2e/base.compose.yaml');
  assert.match(compose, /^  client-dev:/m);
  const devBlock = /^  client-dev:\s*$([\s\S]*?)^networks:/m.exec(compose)?.[1] || '';
  assert.match(devBlock, /profiles: \["dev"\]/);
  assert.match(devBlock, /command: \["sleep", "infinity"\]/);
  assert.match(devBlock, /init: true/);
  const workflow = await read('.github/workflows/ci.yml');
  assert.doesNotMatch(workflow, /RPI_E2E_KEEP|client-dev|--profile dev/);
  const pkg = await read('package.json');
  assert.match(pkg, /"e2e:dev:up": "node tests\/e2e\/run\.mjs --dev-up"/);
  assert.match(pkg, /"e2e:dev:down": "node tests\/e2e\/run\.mjs --dev-down"/);
});

test('CI runs the e2e gate with Buildx cache and failure-only artifacts', async () => {
  const workflow = await read('.github/workflows/ci.yml');
  assert.match(workflow, /branches: \[master\]/);
  assert.match(workflow, /^  pull_request:\s*$/m);
  assert.match(workflow, /permissions:\s*\n  contents: read/);
  assert.match(workflow, /^  e2e:\s*$/m);
  assert.match(workflow, /needs: linux/);
  assert.match(workflow, /runs-on: ubuntu-latest/);
  assert.match(workflow, /timeout-minutes: 30/);
  assert.match(workflow, /docker\/setup-buildx-action@v4/);
  assert.match(workflow, /docker\/build-push-action@v7/);
  assert.match(workflow, /cache-from: type=gha,scope=rpi-e2e/);
  assert.match(workflow, /cache-to: type=gha,mode=max,scope=rpi-e2e,ignore-error=true/);
  assert.match(workflow, /RPI_E2E_PREBUILT: "1"/);
  assert.match(workflow, /npm run test:e2e/);
  assert.match(workflow, /if: failure\(\)/);
  assert.match(workflow, /actions\/upload-artifact@v7/);
  assert.doesNotMatch(workflow, /runs-on: self-hosted/);
});
```

- [ ] **Step 2: Run contracts to verify they fail**

Run: `rtk npm run test:node -- tests/e2e/contracts.test.mjs`
Expected: FAIL — ENOENT on `tests/e2e/scenarios/happy-path/...` and `tests/e2e/base.compose.yaml`.

- [ ] **Step 3: Move the files**

```bash
rtk git mv tests/e2e/compose.yaml tests/e2e/base.compose.yaml
rtk git mv tests/e2e/agent.toml tests/e2e/agent.default.toml
mkdir -p tests/e2e/entrypoints tests/e2e/scenarios/happy-path
rtk git mv tests/e2e/git-entrypoint.sh tests/e2e/entrypoints/git-entrypoint.sh
rtk git mv tests/e2e/target-entrypoint.sh tests/e2e/entrypoints/target-entrypoint.sh
rtk git mv tests/e2e/scenario.sh tests/e2e/scenarios/happy-path/scenario.sh
rtk git mv tests/e2e/fixtures/app tests/e2e/scenarios/happy-path/app
rmdir tests/e2e/fixtures 2>/dev/null || true
```

- [ ] **Step 4: Make `entrypoints/git-entrypoint.sh` scenario-aware (full content)**

```bash
#!/usr/bin/env bash
set -euo pipefail

SCENARIO=${RPI_E2E_SCENARIO:?RPI_E2E_SCENARIO must be set}
APP=/opt/e2e/scenarios/$SCENARIO/app

rm -rf /srv/git/fixture.git /tmp/fixture-work
mkdir -p /srv/git
cp -a "$APP" /tmp/fixture-work
cd /tmp/fixture-work
git init --initial-branch=main
git config user.name 'rpi e2e'
git config user.email 'rpi-e2e@example.invalid'
git add .
git commit -m "fixture: $SCENARIO app"
git clone --bare . /srv/git/fixture.git
exec git daemon --reuseaddr --verbose --export-all --base-path=/srv/git /srv/git
```

- [ ] **Step 5: Make `entrypoints/target-entrypoint.sh` scenario-aware (full content)**

```bash
#!/usr/bin/env bash
set -euo pipefail

SCENARIO=${RPI_E2E_SCENARIO:?RPI_E2E_SCENARIO must be set}
AGENT_CONFIG=/opt/e2e/scenarios/$SCENARIO/agent.toml
[[ -f $AGENT_CONFIG ]] || AGENT_CONFIG=/opt/e2e/agent.default.toml

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
  /usr/local/bin/rpi agent run --config "$AGENT_CONFIG" &
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

- [ ] **Step 6: Parametrize `tests/e2e/base.compose.yaml` (full content)**

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
    # The docker:28-dind entrypoint script injects its own all-interfaces
    # default host binding only when `$#` is 0 or the *first* command
    # argument starts with `-`. Passing a bare `--host=tcp://127.0.0.1:2375`
    # as `command` triggered that guard and made dockerd try to bind port
    # 2375 twice ("port is already allocated"). Prefixing the command with
    # the literal `dockerd` executable name as argument 0 (which does not
    # start with `-`) skips the guard, so `exec "$@"` runs
    # `dockerd --host=tcp://127.0.0.1:2375` exactly once, bound to loopback
    # only. This keeps the Docker API unreachable from sibling containers
    # (`git-fixture`, `client`) on the shared `e2e` bridge network; only
    # `target`, which shares this container's network namespace via
    # `network_mode: service:dind`, can reach it.
    command: ["dockerd", "--host=tcp://127.0.0.1:2375"]
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
      RPI_E2E_SCENARIO: ${RPI_E2E_SCENARIO:?RPI_E2E_SCENARIO must be set by run.mjs}
    command: ["/opt/e2e/entrypoints/target-entrypoint.sh"]
    volumes:
      - ssh-public:/run/e2e-public:ro
      - agent-data:/var/lib/rpi
    # The healthcheck pins /run/rpi/agent.sock: a per-scenario agent.toml may
    # override anything except `socket` (and must keep sshd on port 22).
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
    environment:
      RPI_E2E_SCENARIO: ${RPI_E2E_SCENARIO:?RPI_E2E_SCENARIO must be set by run.mjs}
    command: ["/opt/e2e/entrypoints/git-entrypoint.sh"]
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
      RPI_E2E_SCENARIO: ${RPI_E2E_SCENARIO:?RPI_E2E_SCENARIO must be set by run.mjs}
    working_dir: /opt/e2e/scenarios/${RPI_E2E_SCENARIO}/app
    command: ["/opt/e2e/scenarios/${RPI_E2E_SCENARIO}/scenario.sh"]
    networks:
      - e2e
    volumes:
      - ssh-private:/run/e2e-keys:ro
      - type: bind
        source: ${RPI_E2E_ARTIFACT_DIR:?RPI_E2E_ARTIFACT_DIR must be set by run.mjs}
        target: /artifacts

  client-dev:
    <<: *runtime
    profiles: ["dev"]
    init: true
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
      RPI_E2E_SCENARIO: ${RPI_E2E_SCENARIO:?RPI_E2E_SCENARIO must be set by run.mjs}
    working_dir: /opt/e2e/scenarios/${RPI_E2E_SCENARIO}/app
    command: ["sleep", "infinity"]
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

- [ ] **Step 7: Make Dockerfile normalization recursive + update `.gitattributes`**

In `tests/e2e/Dockerfile`, replace:

```dockerfile
RUN sed -i 's/\r$//' /opt/e2e/*.sh && chmod 0755 /opt/e2e/*.sh
```

with:

```dockerfile
RUN find /opt/e2e -name '*.sh' -exec sed -i 's/\r$//' {} + -exec chmod 0755 {} +
```

Replace the full content of `.gitattributes` with:

```
tests/e2e/**/*.sh text eol=lf
tests/e2e/scenarios/*/app/health text eol=lf
```

- [ ] **Step 8: Compat edits in `tests/e2e/run.mjs` and `tests/e2e/run.test.mjs`**

The runner stays single-scenario but must satisfy the new `${RPI_E2E_SCENARIO:?}` guards. Three edits in `run.mjs`:

1. `const COMPOSE_FILE = path.join('tests', 'e2e', 'compose.yaml');` → `const COMPOSE_FILE = path.join('tests', 'e2e', 'base.compose.yaml');`
2. In `runE2E`, extend `composeEnv`:

```js
  const composeEnv = {
    ...env,
    RPI_E2E_ARTIFACT_DIR: artifactDir,
    RPI_E2E_RUNTIME_IMAGE: runtimeImage,
    RPI_E2E_SCENARIO: 'happy-path',
  };
```

3. In `runDev`, extend `composeEnv` identically (add `RPI_E2E_SCENARIO: 'happy-path',`).

In `run.test.mjs`, update the two path regexes inside `'local success builds, starts dependencies, runs client, then tears down'`:

```js
    assert.match(commands[1], /--file tests[/\\]e2e[/\\]base\.compose\.yaml config --quiet$/);
    assert.match(commands[2], /--file tests[/\\]e2e[/\\]base\.compose\.yaml build client$/);
```

- [ ] **Step 9: Run the node tests to verify they pass**

Run: `rtk npm run test:node`
Expected: PASS — full rewrite of contracts + updated runner tests.

- [ ] **Step 10: Commit**

```bash
rtk git add -A tests/e2e .gitattributes
rtk git commit -m "test(e2e): move harness to scenarios/ layout with parametrized base stack"
```

---
### Task 4: Multi-scenario runner — `runScenario`, pooled `runE2E`, scenario-aware `runDev`

**Files:**
- Modify: `tests/e2e/run.mjs` (extract `runScenario`, rewrite `runE2E` as orchestrator, extend `runDev`, new CLI block)
- Test: `tests/e2e/run.test.mjs` (full rewrite)

**Interfaces:**
- Consumes: Task 1 utilities (`discoverScenarios`, `runPool`, `formatSummary`, `scenarioTimeoutMs`, `parseRunArgs`), Task 3 layout (`base.compose.yaml` + `${RPI_E2E_SCENARIO:?}` guards).
- Produces:
  - `runScenario({ scenario, projectName, runtimeImage, artifactDir, runner?, env?, keep?, timeoutMs?, scenariosDir?, signal? }): Promise<{ scenario, code, timedOut, durationMs, artifactDir }>` — one full stack: `config --quiet` → `up -d --no-build --wait dind target git-fixture` → `run --rm --no-deps client` → diagnostics on failure → `down -v` unless keep.
  - `runE2E({ runner?, env?, projectName?, scenarios?, available?, concurrency?, failFast?, artifactDir?, signal? }): Promise<number>` — validates names (exit 2 on unknown), checks compose version, builds the image once, pools `runScenario` calls, prints `formatSummary`, removes the local image, returns 0 or the first failing scenario's code (130 aborted).
  - `runDev(action, { scenario? = 'happy-path', ... })`.

- [ ] **Step 1: Rewrite `tests/e2e/run.test.mjs` (full content — failing first)**

```js
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
```

- [ ] **Step 2: Run the tests to verify the new ones fail**

Run: `rtk npm run test:node -- tests/e2e/run.test.mjs`
Expected: FAIL — `does not provide an export named 'runScenario'` (and, after that export exists, order/shape mismatches until Step 3 is complete).

- [ ] **Step 3: Rewrite the execution half of `tests/e2e/run.mjs`**

Keep everything from Task 1 (imports, constants, `parseComposeVersion`, `versionAtLeast`, `makeProjectName`, `spawnDocker`, `discoverScenarios`, `runPool`, `formatSummary`, `parseMetaEnv`, `scenarioTimeoutMs`, `parseRunArgs`, `collectDiagnostics`). Replace `runE2E`, `runDev`, and the CLI block with:

```js
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
  await mkdir(artifactDir, { recursive: true });
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

  let code = 1;
  let timedOut = false;
  let attemptedStart = false;
  try {
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
      await collectDiagnostics(compose, artifactDir);
    } else {
      const client = await compose(['run', '--rm', '--no-deps', 'client'], {
        logPath: path.join(artifactDir, 'scenario.log'),
        timeoutMs,
      });
      timedOut = client.timedOut;
      code = signal?.aborted ? 130 : (client.timedOut ? 124 : client.code);
      if (code !== 0) await collectDiagnostics(compose, artifactDir);
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
    if (attemptedStart) await collectDiagnostics(compose, artifactDir);
  } finally {
    if (attemptedStart && keep) {
      console.warn(`rpi e2e: RPI_E2E_KEEP=1 — stack kept (project ${projectName})`);
      console.warn(`rpi e2e:   docker exec -it ${projectName}-target-1 bash`);
      console.warn(`rpi e2e:   clean up: node tests/e2e/run.mjs --down ${projectName}`);
    }
    if (attemptedStart && !keep) {
      const down = await compose(['down', '-v', '--remove-orphans'], {
        logPath: path.join(artifactDir, 'cleanup.log'),
        timeoutMs: 120_000,
      });
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
} = {}) {
  await mkdir(artifactDir, { recursive: true });
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
      await runner(['image', 'rm', runtimeImage], { env: buildEnv, quiet: true, signal });
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
  signal,
} = {}) {
  await mkdir(artifactDir, { recursive: true });
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
    const down = await compose(['down', '-v', '--remove-orphans'], { timeoutMs: 120_000 });
    await runner(['image', 'rm', runtimeImage], { env: composeEnv, quiet: true, signal });
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `rtk npm run test:node`
Expected: PASS — all runner + contracts tests.

- [ ] **Step 5: Commit**

```bash
rtk git add tests/e2e/run.mjs tests/e2e/run.test.mjs
rtk git commit -m "test(e2e): run scenarios in parallel with a bounded worker pool"
```

---
### Task 5: CI concurrency + full verification

**Files:**
- Modify: `.github/workflows/ci.yml` (add `RPI_E2E_CONCURRENCY: "3"` to the `e2e` job env)
- Test: `tests/e2e/contracts.test.mjs` (extend the CI test)

**Interfaces:**
- Consumes: Task 4 runner (`RPI_E2E_CONCURRENCY` env knob).
- Produces: nothing new — final gate.

- [ ] **Step 1: Add the failing CI contract assertion**

In `tests/e2e/contracts.test.mjs`, inside the test `'CI runs the e2e gate with Buildx cache and failure-only artifacts'`, add after the `RPI_E2E_PREBUILT` assertion:

```js
  assert.match(workflow, /RPI_E2E_CONCURRENCY: "3"/);
```

- [ ] **Step 2: Run contracts to verify the new assertion fails**

Run: `rtk npm run test:node -- tests/e2e/contracts.test.mjs`
Expected: FAIL — `RPI_E2E_CONCURRENCY: "3"` not found in the workflow.

- [ ] **Step 3: Add the env var to `.github/workflows/ci.yml`**

In the `e2e` job `env:` block, after `RPI_E2E_ARTIFACT_DIR`:

```yaml
    env:
      RPI_E2E_RUNTIME_IMAGE: rpi-e2e-runtime:ci
      RPI_E2E_PREBUILT: "1"
      RPI_E2E_ARTIFACT_DIR: ${{ runner.temp }}/rpi-e2e
      RPI_E2E_CONCURRENCY: "3"
```

- [ ] **Step 4: Run the full node suite**

Run: `rtk npm run test:node`
Expected: PASS.

- [ ] **Step 5: Run the repo gates (Rust untouched, but required before finishing)**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
```

Expected: all PASS with no diffs/warnings.

- [ ] **Step 6: Run the real Docker e2e end-to-end**

Run: `node tests/e2e/run.mjs`
Expected: the happy-path stack builds, deploys, redeploys, removes; final output contains:

```
rpi e2e: PASS
rpi e2e: 1/1 scenarios passed
  happy-path  PASS <n>s
```

and exit code 0. Requires Docker Desktop (Linux engine); ~10–20 min on first build. If Docker is unavailable in this environment, stop and report — do not claim verification.

Also sanity-check the filter path: `node tests/e2e/run.mjs happy-path --concurrency 1` — same PASS.

- [ ] **Step 7: Commit**

```bash
rtk git add .github/workflows/ci.yml tests/e2e/contracts.test.mjs
rtk git commit -m "ci: run e2e scenarios with concurrency 3"
```

---

## Verification summary (Definition of Done)

1. `rtk npm run test:node` — green (runner units + structural contracts).
2. `rtk cargo fmt --all -- --check`, `rtk cargo clippy --all-targets --locked -- -D warnings`, `rtk cargo test --locked` — green.
3. `node tests/e2e/run.mjs` — happy-path scenario passes end-to-end via the new layout, summary printed, exit 0.
4. `node tests/e2e/run.mjs --dev-up` / `--dev-down` — dev stack still works against `scenarios/happy-path` (manual, optional).
5. Follow-up unblocked: adding `tests/e2e/scenarios/rm-root-owned/` (next spec) requires only a new folder — no runner changes.
