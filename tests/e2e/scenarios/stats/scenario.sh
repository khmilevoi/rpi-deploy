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
