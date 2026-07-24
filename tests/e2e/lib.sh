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

# expect_fail <artifact-file> <cmd...> — run, tee output into the artifact,
# fail the scenario when the command unexpectedly exits ZERO. The negative
# counterpart to run_capture: assert_log then checks the error text.
expect_fail() {
  local file=$1
  shift
  set +e
  "$@" 2>&1 | tee "$ARTIFACTS/$file"
  local status=${PIPESTATUS[0]}
  set -e
  [[ $status -ne 0 ]] || fail "$file command unexpectedly succeeded"
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
