#!/usr/bin/env bash
set -euo pipefail

SCENARIO=${RPI_E2E_SCENARIO:?RPI_E2E_SCENARIO must be set}
AGENT_CONFIG=/opt/e2e/scenarios/$SCENARIO/agent.toml
[[ -f $AGENT_CONFIG ]] || AGENT_CONFIG=/opt/e2e/agent.default.toml

install -d -o rpi-agent -g rpi-agent -m 0750 /var/lib/rpi /var/log/rpi
install -d -o rpi-agent -g rpi-agent -m 0770 /run/rpi
install -d -o deploy -g deploy -m 0700 /home/deploy/.ssh
install -o deploy -g deploy -m 0600 \
  /run/e2e-public/id_ed25519.pub /home/deploy/.ssh/authorized_keys
ssh-keygen -A

runuser -u rpi-agent -- env \
  HOME=/var/lib/rpi \
  XDG_CONFIG_HOME=/var/lib/rpi/.config \
  XDG_CACHE_HOME=/var/lib/rpi/.cache \
  DOCKER_HOST=tcp://127.0.0.1:2375 \
  /usr/local/bin/rpi agent run --config "$AGENT_CONFIG" &
agent_pid=$!

/usr/sbin/sshd -D -e \
  -o PasswordAuthentication=no \
  -o PermitEmptyPasswords=no \
  -o KbdInteractiveAuthentication=no \
  -o PermitRootLogin=no \
  -o PubkeyAuthentication=yes \
  -o AllowTcpForwarding=yes \
  -o AllowStreamLocalForwarding=yes &
sshd_pid=$!

shutdown_children() {
  kill "$agent_pid" "$sshd_pid" 2>/dev/null || true
  wait "$agent_pid" "$sshd_pid" 2>/dev/null || true
}
trap shutdown_children EXIT
trap 'exit 143' TERM INT

set +e
wait -n "$agent_pid" "$sshd_pid"
status=$?
set -e
exit "$status"
