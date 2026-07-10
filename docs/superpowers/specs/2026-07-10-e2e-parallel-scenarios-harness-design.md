# E2E Parallel Scenarios Harness — Design

- **Date:** 2026-07-10
- **Status:** Approved (design), pending implementation plan
- **Scope:** e2e test harness only. The `rpi rm` root-owned-cleanup bug and its reproduction scenario are a **separate follow-up spec** that consumes this harness.

## 1. Problem

The Docker e2e harness under `tests/e2e/` runs exactly one hard-coded scenario:
a single fixture app (`tests/e2e/fixtures/app`), a single client script
(`tests/e2e/scenario.sh`), a single agent config (`tests/e2e/agent.toml`), all
wired into one compose stack (`tests/e2e/compose.yaml`) launched once by
`tests/e2e/run.mjs`.

Adding a second end-to-end case (e.g. a `rpi rm` regression that needs a
root-owned bind mount) has nowhere to live. There is no way to:

- keep more than one scenario, each with its own fixture app / client script /
  agent config;
- run scenarios in parallel under a concurrency cap;
- reuse the client-side assertion helpers across scenarios without copy-paste.

## 2. Goals / Non-goals

**Goals**

- A `scenarios/<name>/` folder layout: one self-contained folder per e2e case,
  holding its own fixture app, client script, and optional config/topology
  overrides.
- Full per-scenario isolation: each scenario runs its own complete stack (its
  own DinD engine, agent, git-fixture), so scenarios never share Docker state.
- Parallel execution with a worker-pool concurrency limit (default 2,
  configurable; CI raises to 3).
- Reusable client tooling: shared assertion/bootstrap helpers in `lib.sh`.
- Flexibility: a scenario can customize its stack topology without duplicating
  the whole compose file.
- Migrate the existing happy-path case into the new structure with no loss of
  coverage.

**Non-goals**

- The `rpi rm` bug fix and its reproduction scenario (separate spec).
- Any change to the production `rpi` CLI or agent code.
- A full git server (Gitea/gitolite). The anonymous read-only `git daemon`
  fixture stays as-is, just parametrized per scenario.
- Cross-scenario port/network coordination (unnecessary: each stack is fully
  isolated via its own DinD network namespace).

## 3. Architecture Overview

A scenario is a folder. The runtime image already bakes the entire `tests/e2e`
tree into `/opt/e2e` (`COPY tests/e2e /opt/e2e`), so **all** scenarios ship in
one image; selecting a scenario is a run-time concern driven by the
`RPI_E2E_SCENARIO` environment variable.

Each scenario is launched as an independent Docker Compose **project** (unique
`--project-name`) built from a shared `base.compose.yaml` plus an optional
per-scenario `compose.override.yaml`. Because every stack gets its own `dind`
service, N scenarios share neither engine nor state. `run.mjs` discovers the
scenario folders, builds the shared runtime image once, then drives up to
`RPI_E2E_CONCURRENCY` scenario stacks concurrently through a worker pool,
aggregating a pass/fail summary at the end.

```
build shared runtime image (once)
        │
        ▼
discover scenarios/*/  ──▶  worker pool (cap = RPI_E2E_CONCURRENCY)
                                 ├─ scenario A → own compose project → own dind/agent/git-fixture
                                 ├─ scenario B → own compose project → own dind/agent/git-fixture
                                 └─ … (bounded concurrency)
        │
        ▼
aggregate summary (scenario → pass/fail/duration); exit ≠ 0 if any failed
```

## 4. Directory Layout & Scenario Contract

```
tests/e2e/
  base.compose.yaml        # was compose.yaml: keygen, dind, target, git-fixture, client, client-dev
  agent.default.toml       # was agent.toml: shared default agent config
  Dockerfile               # COPY tests/e2e → /opt/e2e unchanged; *.sh normalize step made recursive
  run.mjs                  # discovery + worker pool + reporting
  lib.sh                   # shared client bootstrap + assertion library
  entrypoints/
    git-entrypoint.sh      # serves scenarios/$RPI_E2E_SCENARIO/app
    target-entrypoint.sh   # resolves per-scenario agent config
  run.test.mjs             # unit tests for the runner (updated)
  contracts.test.mjs       # structural contract tests (rewritten)
  scenarios/
    happy-path/            # migrated existing scenario
      app/                 # fixture repo: Dockerfile, compose.yaml, rpi.toml, health
      scenario.sh          # client assertions (source /opt/e2e/lib.sh; e2e_bootstrap)
      agent.toml           # OPTIONAL — absent ⇒ agent.default.toml
      compose.override.yaml# OPTIONAL — absent ⇒ base stack unchanged
      meta.env             # OPTIONAL — per-scenario knobs (e.g. RPI_E2E_TIMEOUT)
```

**Scenario contract (what makes a folder a scenario):**

- **Required:** `scenario.sh` (client test) and `app/` (fixture repo the
  git-fixture serves). A folder under `scenarios/` missing `scenario.sh` is not
  a scenario and is skipped by discovery.
- **Optional:** `agent.toml` (per-scenario agent config; falls back to
  `agent.default.toml`), `compose.override.yaml` (topology customization),
  `meta.env` (scenario knobs; only `RPI_E2E_TIMEOUT` initially — YAGNI on the
  rest).

Entrypoints resolve everything from `/opt/e2e/scenarios/$RPI_E2E_SCENARIO/…`
inside the baked image, so no bind mounts of scenario files are needed.

## 5. Base Stack Parametrization

`base.compose.yaml` threads `RPI_E2E_SCENARIO` (declared
`${RPI_E2E_SCENARIO:?…}` so an unset value fails loudly) into the scenario-aware
services:

- **git-fixture** → `git-entrypoint.sh` serves
  `/opt/e2e/scenarios/$RPI_E2E_SCENARIO/app` as the bare repo, still published
  under the stable name `git://git-fixture/fixture.git`. Fixture `rpi.toml`
  keeps `repo = "git://git-fixture/fixture.git"` across all scenarios.
- **target** → `target-entrypoint.sh` picks the agent config: use
  `/opt/e2e/scenarios/$RPI_E2E_SCENARIO/agent.toml` if it exists, else
  `/opt/e2e/agent.default.toml`. The current `./agent.toml` bind mount is
  removed in favor of this baked-path resolution.
- **client** → runs `/opt/e2e/scenarios/$RPI_E2E_SCENARIO/scenario.sh`
  (replaces the hard-coded `command`/`working_dir`).

Isolation is inherent: each stack has its own `dind` (`network_mode:
service:dind`), its own network/volumes/container names under a unique
`--project-name`. Agent port ranges (`port_min`/`port_max`) and container names
therefore never collide between scenarios; no cross-scenario coordination.

## 6. Flexibility Mechanism

Two layers, from simplest to most powerful:

1. **Config override** — a scenario drops in `agent.toml` to change agent
   settings (ports, timeouts, gc, cloudflared off) without touching compose.
2. **Topology override** — a scenario drops in `compose.override.yaml`, which
   `run.mjs` appends to the `-f` chain
   (`-f base.compose.yaml -f scenarios/<name>/compose.override.yaml`). Compose
   merges it: the scenario can add services (a second agent, a registry),
   change env, or — combined with `profiles:` on optional base services —
   disable a base service it does not need. Absent override ⇒ base stack
   verbatim.

This keeps the common case zero-config while giving an escape hatch for
divergent stacks, without cloning the whole compose file per scenario.

## 7. Runner: Discovery, Worker Pool, Reporting

`run.mjs` is refactored around a single-stack function and a pool orchestrator.

- **`runScenario({ scenario, runtimeImage, artifactDir, concurrencySafeProject, env, signal })`**
  runs one stack: `compose config --quiet` (with `RPI_E2E_SCENARIO` set) →
  `up -d --no-build --wait dind target git-fixture` →
  `run --rm --no-deps client` → on failure collect diagnostics → teardown
  (`down -v --remove-orphans`) unless `RPI_E2E_KEEP=1`. Returns
  `{ scenario, code, durationMs, artifactDir }`. This preserves the current
  `runE2E` exit-code semantics (124 timeout, 130 aborted, cleanup-failure
  cannot mask a scenario failure).
- **Shared image, built once.** The runtime image is built (non-prebuilt) or
  assumed present (`RPI_E2E_PREBUILT=1`) **before** the pool, and reused by
  every scenario stack (`--no-build`). The local image is removed once after
  the pool (non-prebuilt, non-keep) — not per scenario.
- **Discovery.** Enumerate `scenarios/*/` with a `scenario.sh`, sorted for
  deterministic order. `node run.mjs <name> [<name>…]` runs a subset.
- **Worker pool.** Run up to `RPI_E2E_CONCURRENCY` (default 2; CI 3;
  overridable by `--concurrency N`) `runScenario` calls concurrently. Project
  name per stack = `${baseProject}-${scenario}` for uniqueness and readable
  container names. Per-scenario artifacts under
  `${artifactDir}/${scenario}/…`.
- **Reporting.** Default runs **all** scenarios and reports every failure (no
  abort on first); `--fail-fast` aborts remaining work on the first failure. A
  final summary table (`scenario → pass/fail/duration`) prints to stdout; the
  process exits non-zero if any scenario failed.
- **Preserved DX.** `RPI_E2E_KEEP=1`, `--down <project>`, and the dev profile
  (`--dev-up <scenario>` defaulting to `happy-path`, `--dev-down`) keep working.
  `RPI_E2E_PREBUILT`, `RPI_E2E_RUNTIME_IMAGE`, `RPI_E2E_ARTIFACT_DIR` are
  honored unchanged.

## 8. Reuse Tooling (`lib.sh`)

Generic client helpers currently inlined in `scenario.sh` move into `lib.sh`
so every scenario shares one implementation:

- `e2e_bootstrap` — sets the shared globals (`ARTIFACTS=/artifacts`, `KEY`,
  `CONNECT=(--host target --user deploy --key …)`, `SSH=(…)`), makes the
  artifacts dir, calls `e2e_client_init`, waits for SSH. (This was the prologue
  of `scenario.sh`.)
- `e2e_client_init` — existing; records the `target` SSH host key.
- `fail <msg>` — print to stderr and exit non-zero.
- `run_capture <artifact-file> <cmd…>` — run, tee to `$ARTIFACTS/<file>`,
  assert exit 0.
- `assert_log <file> <text>` — `grep -F` a substring.
- `assert_deploy_log <file>` — composite deploy milestones (`fetched `,
  `docker compose build ...`, `docker compose up -d ...`,
  `healthcheck: passed`); reusable by any deploy scenario.

A scenario script becomes: `source /opt/e2e/lib.sh; e2e_bootstrap` followed by
scenario-specific steps built on the shared assertions — no prologue copy-paste.

## 9. Migration of Happy-Path

- `tests/e2e/fixtures/app/**` → `tests/e2e/scenarios/happy-path/app/**`.
- Body of `tests/e2e/scenario.sh` → `tests/e2e/scenarios/happy-path/scenario.sh`,
  trimmed to use `lib.sh` helpers. Same assertions: deploy, redeploy, ls,
  health probe, `rpi rm e2e-fixture --yes`, post-rm emptiness, no leftover
  containers.
- `tests/e2e/agent.toml` → `tests/e2e/agent.default.toml`; happy-path ships no
  per-scenario `agent.toml`.
- `tests/e2e/compose.yaml` → `tests/e2e/base.compose.yaml`;
  `git-entrypoint.sh`/`target-entrypoint.sh` moved under `entrypoints/` and made
  scenario-aware.
- `.gitattributes` LF rules updated for the new paths
  (`tests/e2e/scenarios/**` shell + `health`, `tests/e2e/entrypoints/**`).
- Dockerfile: `COPY tests/e2e /opt/e2e` stays; the CRLF-strip + `chmod 0755`
  step changes from the top-level `/opt/e2e/*.sh` glob to a recursive form
  (`find /opt/e2e -name '*.sh'`) so relocated entrypoints and per-scenario
  scripts are normalized and executable.

## 10. CI Integration

`.github/workflows/ci.yml` `e2e` job keeps its shape: build the cached runtime
image via Buildx, then `npm run test:e2e` (now runs all scenarios under the
pool). Add `RPI_E2E_CONCURRENCY: "3"` to the job env. Failure-only artifact
upload of `${{ runner.temp }}/rpi-e2e` still applies; the per-scenario
subdirectories land under it. `package.json` scripts (`test:e2e`,
`e2e:dev:up`, `e2e:dev:down`) keep their names.

## 11. Testing the Harness

- **`run.test.mjs`** (unit, no Docker, fake runner): keep coverage for a single
  stack via `runScenario` (build/up/run/down ordering, 124/130 exit codes,
  cleanup-failure semantics, prebuilt skip, keep skip, dev up/down). Add:
  discovery selects folders with `scenario.sh`; the pool respects
  `RPI_E2E_CONCURRENCY` (never more than N stacks "up" at once); the summary
  aggregates results and the overall exit code is non-zero iff any scenario
  failed; a single-scenario invocation runs only that stack.
- **`contracts.test.mjs`** (structural): rewrite for the new layout —
  `base.compose.yaml` service set and DinD-loopback invariants,
  `scenarios/happy-path/app/rpi.toml` fixture contract, `agent.default.toml`
  values, entrypoints reading `RPI_E2E_SCENARIO`, `lib.sh` exposing the shared
  helpers, `scenarios/happy-path/scenario.sh` sourcing `lib.sh` and covering
  deploy/redeploy/rm, CI running the e2e gate with concurrency, dev profile out
  of the CI path.

## 12. Backward-Compatibility Notes

- The single-stack env contract (`RPI_E2E_PREBUILT`, `RPI_E2E_RUNTIME_IMAGE`,
  `RPI_E2E_ARTIFACT_DIR`, `RPI_E2E_KEEP`) is preserved.
- New knobs: `RPI_E2E_CONCURRENCY` (default 2), `RPI_E2E_SCENARIO` (per-stack,
  set by the runner), optional `--concurrency` / `--fail-fast` CLI flags and a
  positional scenario filter.
- Removed: the `./agent.toml` bind mount (superseded by baked-path resolution);
  the top-level `tests/e2e/scenario.sh`, `fixtures/`, `compose.yaml`,
  `agent.toml` (relocated as above).

## 13. Follow-up (out of scope here)

The next spec adds a `scenarios/rm-root-owned/` scenario whose fixture writes a
root-owned file into a bind mount, reproducing `rpi rm` failing with
`source error: Permission denied (os error 13)`, then fixes the agent so
cleanup removes root-owned workdir content. This harness is the vehicle that
makes that scenario a first-class, isolated, parallel test.
