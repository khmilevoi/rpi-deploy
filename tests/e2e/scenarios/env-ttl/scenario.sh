#!/usr/bin/env bash
set -euo pipefail

# Environment-overlays spec: the agent's `[environments] reap_interval` sweep
# tears down any environment overlay whose `ttl` has elapsed since its last
# successful deploy (or since creation, if it never deployed successfully).
# This scenario's own agent.toml shortens the sweep to 5s and its `temp`
# overlay sets a 10s ttl, so the reaper fires well inside the test's normal
# timeout; it polls `rpi env ls` until the environment disappears, then
# asserts everything the reaper must NOT touch survived: the `keep`
# environment (no ttl -- never reaped, regardless of age) and the base
# project. `keep` deploys before `temp` so temp's ttl clock starts last and
# the first `env ls` is guaranteed to still see both.

source /opt/e2e/lib.sh
e2e_bootstrap

run_capture deploy-base.log rpi deploy "${CONNECT[@]}"
assert_deploy_log deploy-base.log

run_capture deploy-keep.log rpi deploy --env keep "${CONNECT[@]}"
assert_deploy_log deploy-keep.log

run_capture deploy-env.log rpi deploy --env temp "${CONNECT[@]}"
assert_deploy_log deploy-env.log

run_capture env-ls-1.log rpi env ls "${CONNECT[@]}"
assert_log env-ls-1.log 'e2e-fixture--temp'
assert_log env-ls-1.log 'e2e-fixture--keep'

# Poll (up to 60s) for the reaper to expire and destroy the ttl'd environment.
reaped=0
for _ in $(seq 1 12); do
  sleep 5
  rpi env ls "${CONNECT[@]}" >"$ARTIFACTS/env-ls-poll.log" 2>&1
  if ! grep -Fq 'e2e-fixture--temp' "$ARTIFACTS/env-ls-poll.log"; then
    reaped=1
    break
  fi
done
[[ $reaped -eq 1 ]] || fail 'environment e2e-fixture--temp was not reaped within 60s'

# The no-ttl environment and the base project both survive the reap.
run_capture env-ls-2.log rpi env ls "${CONNECT[@]}"
assert_log env-ls-2.log 'e2e-fixture--keep'

run_capture ls.log rpi ls "${CONNECT[@]}"
assert_log ls.log 'e2e-fixture'

echo 'rpi e2e: PASS'
