#!/usr/bin/env bash
set -euo pipefail

source /opt/e2e/lib.sh

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

mkdir -p "$ARTIFACTS"
e2e_client_init || fail 'client init failed'
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
