#!/usr/bin/env bash
# Shared client prologue, sourced by scenario.sh and by interactive dev
# shells. OpenSSH resolves `~` through the passwd database (pw_dir), not
# $HOME, and the rpi CLI spawns plain `ssh` with no way to pass
# -o UserKnownHostsFile — so the target host key must be recorded in the
# system-wide /etc/ssh/ssh_known_hosts to cover both ssh paths.

E2E_KEY=/run/e2e-keys/id_ed25519

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
