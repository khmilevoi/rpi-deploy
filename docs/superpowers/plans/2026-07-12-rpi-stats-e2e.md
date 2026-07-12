# rpi stats — e2e scenario Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a dedicated Docker-in-Docker e2e scenario that deploys the default fixture and asserts `rpi stats` renders the new host panel (TEMP + history-backed CPU% sparkline) and services table, plus `rpi stats --json` carries the additive `temp_celsius`/`host_history` fields and a real sampled `at_ms`.

**Architecture:** The existing e2e harness (`tests/e2e/run.mjs`) auto-discovers each `scenarios/<name>/scenario.sh` and runs it in an isolated stack (privileged `dind` engine + `target` agent + `git-fixture` + `client`). The `client` container's `working_dir` is `scenarios/<name>/app`, which the e2e `Dockerfile` auto-fills from `app.default` (project `e2e-fixture`, service `web`) unless the scenario ships its own `app/`. So a new scenario needs only `scenarios/stats/scenario.sh`; it deploys the fixture over the SSH tunnel, then exercises `rpi stats` end-to-end against the real agent (which now runs the background host-metrics sampler).

**Tech Stack:** Bash scenario script + the harness's `lib.sh` helpers (`e2e_bootstrap`, `run_capture`, `assert_log`, `assert_deploy_log`, `CONNECT`, `SSH`, `fail`). Node-based harness runner/tests (`run.mjs`, `run.test.mjs`, `contracts.test.mjs`). The e2e image builds `rpi` from the branch source with `rust:1.88-bookworm`.

## Global Constraints

- **Scenario folder name** must match the harness contract `^[a-z0-9][a-z0-9-]*$` (a valid Docker Compose project-name component). Use exactly `stats`.
- **Reuse the default app fixture** — do NOT add a `scenarios/stats/app/` directory. The Dockerfile copies `app.default` into every scenario's `app/` with `cp -rn` (no-clobber). The fixture project is `e2e-fixture`, its public service is `web`.
- **Assertions are literal substring matches** via `assert_log` (`grep -F`). The `client` runs with `NO_COLOR=1`, so `comfy-table` output is plain (no ANSI) — header/label text matches directly.
- **DinD e2e runs in CI only.** This Windows dev box cannot run the privileged DinD stack. Local verification is limited to: bash syntax (`bash -n`), scenario discovery (`discoverScenarios` picks up `stats`), and the harness's own Node unit tests (`run.test.mjs`, `contracts.test.mjs`) staying green — none of which need Docker.
- **MSRV note (no action, just awareness):** `ratatui 0.30.2` has `rust-version = "1.88.0"`, exactly met by the e2e Dockerfile's `rust:1.88-bookworm`. The e2e image build now compiles the TUI module; CI validates this. Do not bump the Dockerfile.
- **Do not modify `happy-path` or `rm-root-owned`.** This is additive.
- **CI gate for the surrounding workspace stays green:** the Rust `fmt`/`clippy`/`test` gate is unaffected by a bash scenario, but the harness Node tests must still pass.

---

## File Structure

**Created:**
- `tests/e2e/scenarios/stats/scenario.sh` — the new scenario: bootstrap → deploy → running-gate (`rpi ls`) → `rpi stats` static assertions → `rpi stats --json` assertions.

**Not created (intentionally):**
- No `tests/e2e/scenarios/stats/app/` — the default fixture is auto-filled by the Dockerfile.
- No `meta.env` — the default 15-minute scenario timeout is ample for a single deploy + two stats reads.

**Not modified:**
- `tests/e2e/run.mjs`, `lib.sh`, `base.compose.yaml`, `Dockerfile` — the scenario is auto-discovered and auto-provisioned; no harness change is required.

---

## Task 1: Add the `stats` e2e scenario

**Files:**
- Create: `tests/e2e/scenarios/stats/scenario.sh`

**Interfaces:**
- Consumes (from `tests/e2e/lib.sh`, sourced at runtime): `e2e_bootstrap` (sets `ARTIFACTS`, `KEY`, `CONNECT` array, `SSH` array), `run_capture <artifact> <cmd...>` (runs, tees to `$ARTIFACTS/<artifact>`, fails on nonzero exit), `assert_log <artifact> <literal>` (`grep -F`), `assert_deploy_log <artifact>` (asserts the four deploy milestones), `fail <msg>`.
- Produces: a self-contained scenario script the harness discovers and runs.

- [ ] **Step 1: Write the scenario script**

Create `tests/e2e/scenarios/stats/scenario.sh` with exactly this content:

```bash
#!/usr/bin/env bash
set -euo pipefail

source /opt/e2e/lib.sh
e2e_bootstrap

rpi --version
run_capture deploy.log rpi deploy "${CONNECT[@]}"
assert_deploy_log deploy.log

# Confirm the fixture is actually running before asserting its live metrics.
run_capture ls.log rpi ls "${CONNECT[@]}"
assert_log ls.log 'web:running'

# Static view: host panel (TEMP column + history-backed CPU% sparkline row,
# proving the agent's background sampler retained a time series) + the
# per-service table row for the running `web` service.
run_capture stats.log rpi stats e2e-fixture "${CONNECT[@]}"
assert_log stats.log 'TEMP'
assert_log stats.log 'CPU%'
assert_log stats.log 'e2e-fixture'
assert_log stats.log 'web'

# JSON view: the additive fields the upgraded agent now serves. `at_ms` only
# appears inside a host_history entry, so its presence proves the sampler
# produced at least one real sample; the project always appears in projects[].
run_capture stats-json.log rpi stats e2e-fixture --json "${CONNECT[@]}"
assert_log stats-json.log 'temp_celsius'
assert_log stats-json.log 'host_history'
assert_log stats-json.log 'at_ms'
assert_log stats-json.log 'e2e-fixture'

echo 'rpi e2e: PASS'
```

Rationale for each assertion (do not add or drop any without cause):
- `assert_deploy_log deploy.log` — reuses the harness's four-milestone deploy check; guarantees the fixture built, came up, and passed its healthcheck before we read metrics.
- `ls.log` → `web:running` — running-gate so the per-service `docker stats` row is reliably present (same gate `happy-path` relies on).
- `stats.log` → `TEMP` — the host panel's TEMP column header always renders (value is `n/a` on a DinD host with no thermal zone; the header proves the column exists).
- `stats.log` → `CPU%` — the CPU% sparkline row label; it renders only when `host_history` is non-empty. The agent seeds one sample at construction and samples every 2s, so history is non-empty by the time deploy finishes — this asserts the history-backed graph actually rendered.
- `stats.log` → `e2e-fixture` and `web` — the services table shows the deployed project and its running service.
- `stats-json.log` → `temp_celsius` — the new host field is serialized (as `null` on DinD, but the key is present because it has no `skip_serializing_if`).
- `stats-json.log` → `host_history` and `at_ms` — the new history field is serialized and contains at least one real sample (`at_ms` is a per-sample field), proving the sampler → ring buffer → HTTP → CLI-decode path end-to-end.
- `stats-json.log` → `e2e-fixture` — the project is present in `projects[]` regardless of docker-stats timing (host-level, non-flaky).
- No `rpi rm` — the harness tears the whole stack down (`compose down -v`) after the scenario; a focused stats scenario needs no teardown of its own.

- [ ] **Step 2: Normalize line endings and make it executable-safe**

The e2e `Dockerfile` strips CRLF and `chmod 0755`es every `*.sh` under `/opt/e2e` at image build (`find /opt/e2e -name '*.sh' -exec sed -i 's/\r$//' {} + -exec chmod 0755 {} +`), so the committed file just needs LF line endings. On this Windows checkout, confirm the file has no CR bytes:

Run: `git show :tests/e2e/scenarios/stats/scenario.sh 2>/dev/null | rtk grep -c $'\r'` is not applicable pre-commit; instead check the working file:
Run: `rtk grep -c $'\r' tests/e2e/scenarios/stats/scenario.sh`
Expected: `0` (no carriage returns). If nonzero, re-save the file with LF endings (the repo's `.gitattributes`/editor should already enforce LF for `.sh`).

- [ ] **Step 3: Bash syntax check**

Run: `bash -n tests/e2e/scenarios/stats/scenario.sh`
Expected: no output, exit 0 (valid bash — no execution, DinD not required).

- [ ] **Step 4: Confirm the harness discovers the new scenario**

Run:
```bash
node -e "import('./tests/e2e/run.mjs').then(m => m.discoverScenarios()).then(s => { console.log(JSON.stringify(s)); process.exit(s.includes('stats') ? 0 : 1); })"
```
Expected: prints a JSON array that includes `"stats"` (e.g. `["happy-path","rm-root-owned","stats"]`), exit 0. This proves the folder name matches `SCENARIO_NAME_RE` and the `scenario.sh` is found.

- [ ] **Step 5: Run the harness's own Node unit tests (no Docker needed)**

Run: `node --test tests/e2e/run.test.mjs tests/e2e/contracts.test.mjs`
Expected: all tests pass. These validate the runner/contract logic with mocked `available` scenario lists, so the new real scenario must not break them. (If `contracts.test.mjs` asserts something about every discovered scenario, this is where a mismatch would surface — read the failure and align the scenario to the contract, do not weaken the test.)

- [ ] **Step 6: (Optional, only if Docker is available locally) confirm the e2e image still builds**

The e2e image compiles `rpi` (now including the ratatui TUI) on `rust:1.88-bookworm`. If Docker is available on this machine:
Run: `docker build -f tests/e2e/Dockerfile -t rpi-e2e-runtime:stats-check .`
Expected: build succeeds (confirms ratatui 0.30's MSRV 1.88.0 is satisfied by the base image). If Docker is unavailable, skip — CI's `Build cached e2e runtime` step validates this.

- [ ] **Step 7: Commit**

```bash
rtk git add tests/e2e/scenarios/stats/scenario.sh
rtk git commit -m "test(e2e): stats scenario asserts host panel, sparkline, and json temp/history"
```

---

## Self-Review (run after Task 1)

- **Spec coverage:** the scenario exercises static `rpi stats <project>` (host panel TEMP + CPU% sparkline + services table) and `rpi stats <project> --json` (additive `temp_celsius`/`host_history` + a real `at_ms`). The realtime `-w` TUI is intentionally NOT covered — it needs an interactive TTY the non-interactive `client` container does not provide.
- **No harness edits:** discovery, app provisioning, and teardown are all automatic; the only new file is the scenario script.
- **Non-flaky by construction:** host-level JSON assertions (`temp_celsius`, `host_history`, `at_ms`, project name) do not depend on `docker stats` timing; the per-service `web` assertion is gated behind `assert_deploy_log` (healthcheck passed) + `rpi ls` (`web:running`).

## Execution note

Full DinD execution is CI-only. Local acceptance for this plan = Steps 3–5 green (syntax, discovery, harness unit tests). CI's `Production-path Docker e2e` job (`npm run test:e2e`) runs the real scenario.
