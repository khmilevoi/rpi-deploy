#!/usr/bin/env bash
set -euo pipefail

# Regression guard for the "container does not survive a host reboot" bug: the
# deployed public container came up with Docker's default restart policy `no`,
# so after a board reboot it stayed down (the site returned 530 until someone
# ran `docker start` by hand). pi owns this service's lifecycle, so its
# generated compose override must pin `restart: unless-stopped`. This scenario
# deploys the default fixture -- whose own compose declares NO restart policy --
# and asserts the running container ends up with `unless-stopped`, which can
# only have come from pi's override. It passes with the fix and fails (policy
# `no`) if that override regresses.

source /opt/e2e/lib.sh
e2e_bootstrap

run_capture deploy.log rpi deploy "${CONNECT[@]}"
assert_deploy_log deploy.log

DOCKER=(env DOCKER_HOST=tcp://127.0.0.1:2375 docker)

# Precondition: the fixture's own compose declares no restart policy, so any
# policy observed on the container below can only come from pi's override --
# otherwise this scenario would say nothing about the bug it guards.
if "${SSH[@]}" grep -q restart /var/lib/rpi/workdirs/e2e-fixture/compose.yaml; then
  fail 'fixture compose unexpectedly declares a restart policy (precondition not met)'
fi

cid=$("${SSH[@]}" "${DOCKER[@]}" ps -q \
  --filter label=com.docker.compose.project=e2e-fixture \
  --filter label=com.docker.compose.service=web)
[[ -n $cid ]] || fail 'web container not found after deploy'

policy=$("${SSH[@]}" "${DOCKER[@]}" inspect -f '{{.HostConfig.RestartPolicy.Name}}' "$cid")
[[ $policy == 'unless-stopped' ]] || \
  fail "web container restart policy is '$policy', expected 'unless-stopped' (would not survive a reboot)"

run_capture rm.log rpi rm e2e-fixture --yes "${CONNECT[@]}"
assert_log rm.log "project 'e2e-fixture' removed"

echo 'rpi e2e: PASS'
