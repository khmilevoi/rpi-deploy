#!/usr/bin/env bash
set -euo pipefail

# Environment-overlays spec: `rpi deploy --env <name>` deploys an isolated
# stack keyed `<base>--<name>` alongside the base project, running the
# overlay's `[environment] on_create` command exactly once, right after the
# key's first fully-successful deploy. This scenario walks the full
# lifecycle: create (on_create fires), redeploy (on_create must NOT fire
# again), `rpi env ls`, `rpi env reset-data` (wipes the volume and
# on_create_done, so the next deploy re-seeds), and `rpi env destroy`
# (idempotent -- a second destroy reports nothing-to-do instead of erroring)
# -- while asserting the base project is never touched by any of it.

source /opt/e2e/lib.sh
e2e_bootstrap

run_capture deploy-base.log rpi deploy "${CONNECT[@]}"
assert_deploy_log deploy-base.log

run_capture deploy-env.log rpi deploy --env test "${CONNECT[@]}"
assert_deploy_log deploy-env.log
assert_log deploy-env.log "on_create 'seed' completed"

run_capture ls.log rpi ls "${CONNECT[@]}"
assert_log ls.log 'e2e-fixture'
assert_log ls.log 'e2e-fixture--test'

# Redeploy the same env: on_create already completed once, must not run again.
run_capture deploy-env-2.log rpi deploy --env test "${CONNECT[@]}"
assert_deploy_log deploy-env-2.log
if grep -Fq "on_create 'seed'" "$ARTIFACTS/deploy-env-2.log"; then
  fail 'on_create ran again on redeploy'
fi

run_capture env-ls.log rpi env ls "${CONNECT[@]}"
assert_log env-ls.log 'e2e-fixture--test'

# reset-data wipes the volume (and clears on_create_done); the next deploy of
# the same key must re-seed from scratch.
run_capture reset.log rpi env reset-data test --yes "${CONNECT[@]}"

run_capture deploy-env-3.log rpi deploy --env test "${CONNECT[@]}"
assert_deploy_log deploy-env-3.log
assert_log deploy-env-3.log "on_create 'seed' completed"

run_capture destroy.log rpi env destroy test --yes "${CONNECT[@]}"
assert_log destroy.log 'destroyed'

run_capture destroy-again.log rpi env destroy test --yes "${CONNECT[@]}"
assert_log destroy-again.log 'nothing to destroy'

run_capture env-ls-2.log rpi env ls "${CONNECT[@]}"
assert_log env-ls-2.log 'no environments'

# Base project is untouched by any of the environment operations above.
run_capture ls-2.log rpi ls "${CONNECT[@]}"
assert_log ls-2.log 'e2e-fixture'

echo 'rpi e2e: PASS'
