#!/usr/bin/env bash
set -euo pipefail

source /opt/e2e/lib.sh
e2e_bootstrap

rpi --version

# Current CLI against a v0.17.1 agent: the handshake has no `features`
# field, so capabilities come from the frozen legacy matrix. source-check
# is absent on 0.17.1 and Silent-gated — the deploy must sail through.
run_capture deploy.log rpi deploy "${CONNECT[@]}"
assert_deploy_log deploy.log
assert_log deploy.log 'agent 0.17.1 (api v1)'
# Version-skew banner: CLI is newer than the legacy agent.
assert_log deploy.log 'update the agent on the Pi'

# secrets is matrix-inferred as available on 0.17.1: the gate passes and the
# legacy route answers.
run_capture secrets-ls.log rpi secrets ls "${CONNECT[@]}"
assert_log secrets-ls.log 'no secrets stored'

# stats gate passes the same way; the legacy agent has no host_history, so
# the CLI's additive-field degradation warning must appear.
run_capture stats.log rpi stats e2e-fixture "${CONNECT[@]}"
assert_log stats.log 'e2e-fixture'
assert_log stats.log 'no host history from the agent'

echo 'rpi e2e: PASS'
