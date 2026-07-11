#!/usr/bin/env bash
set -euo pipefail

# Regression guard for the `rpi rm` root-owned-workdir bug: `rpi rm <project>`
# used to fail with `source error: Permission denied (os error 13)` and leave
# the project half-removed when the deployed project left root-owned files in
# its workdir (docker-created bind-mount dirs, owned by root, that the
# non-root rpi-agent cannot delete). The fix (GitSource::remove_tree) falls
# back to a one-shot root container that force-removes the leftovers. This
# scenario asserts the CORRECT outcome -- rm succeeds and the project is fully
# gone -- so it passes with the fix in place and fails if that fix regresses.

source /opt/e2e/lib.sh
e2e_bootstrap

run_capture deploy-1.log rpi deploy "${CONNECT[@]}"
assert_deploy_log deploy-1.log

run_capture ls-1.log rpi ls "${CONNECT[@]}"
assert_log ls-1.log 'rm-root-owned'
assert_log ls-1.log 'web:running'
health=$("${SSH[@]}" curl -fsS http://127.0.0.1:18080/health)
[[ $health == 'ok' ]] || fail "unexpected health body: $health"

# Confirm the fixture actually left a root-owned file inside the workdir
# before exercising `rpi rm` -- otherwise a pass or fail below would say
# nothing about the bug this scenario exists to reproduce.
leftover_stat=$("${SSH[@]}" stat -c '%U:%a' \
  /var/lib/rpi/workdirs/rm-root-owned/data/sub/root-owned.txt) || \
  fail 'root-owned fixture file is missing before rpi rm (precondition not met)'
[[ $leftover_stat == root:* ]] || \
  fail "expected a root-owned fixture file, got: $leftover_stat"

run_capture rm.log rpi rm rm-root-owned --yes "${CONNECT[@]}"
assert_log rm.log "project 'rm-root-owned' removed"

run_capture ls-after-rm.log rpi ls "${CONNECT[@]}"
assert_log ls-after-rm.log 'no projects deployed yet'

if "${SSH[@]}" test -e /var/lib/rpi/workdirs/rm-root-owned; then
  fail 'workdir still present after rpi rm'
fi

leftovers=$("${SSH[@]}" env DOCKER_HOST=tcp://127.0.0.1:2375 docker ps -aq \
  --filter label=com.docker.compose.project=rm-root-owned)
[[ -z $leftovers ]] || fail "fixture containers remain after rpi rm: $leftovers"

echo 'rpi e2e: PASS'
