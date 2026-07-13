#!/usr/bin/env bash
set -euo pipefail

# Board-side `rpi agent update` against an offline file:// fixture release.
# Proves the new download -> verify(SHA256) -> swap path: we serve the
# pre-built legacy binary (v0.17.1) as the "release" for version 0.17.1 and
# assert `rpi agent update --version 0.17.1` swaps /usr/local/bin/rpi (which
# ships as the CURRENT build) down to it. No systemd here, so this asserts the
# on-disk swap, not a live agent restart.

source /opt/e2e/lib.sh
e2e_bootstrap

# Precondition: the canonical binary is NOT already 0.17.1 (else the swap would
# be a no-op and prove nothing).
before=$("${SSH[@]}" rpi --version)
echo "$before" | grep -q '0.17.1' && fail "precondition: /usr/local/bin/rpi is already 0.17.1"

# Stage a release-shaped fixture on the target, served via file://. The archive
# must contain a member named `rpi`, so copy rpi-legacy -> rpi before taring.
"${SSH[@]}" sudo sh -euc '
  arch=$(uname -m)
  case "$arch" in
    aarch64|arm64) triple=aarch64-unknown-linux-musl ;;
    x86_64|amd64)  triple=x86_64-unknown-linux-musl ;;
    *) echo "unsupported arch: $arch" >&2; exit 1 ;;
  esac
  work=$(mktemp -d)
  cp /usr/local/bin/rpi-legacy "$work/rpi"
  d=/opt/e2e-release/v0.17.1
  mkdir -p "$d"
  tar -C "$work" -czf "$d/rpi-v0.17.1-$triple.tar.gz" rpi
  ( cd "$d" && sha256sum "rpi-v0.17.1-$triple.tar.gz" > SHA256SUMS )
  rm -rf "$work"
'

# Run the update as root with the fixture base URL injected. `env` sets the var
# for the rpi child regardless of the sudoers env policy; SUDO_USER=deploy is
# preserved by sudo so npm-channel detection resolves (npm absent -> github).
run_capture update.log "${SSH[@]}" \
  sudo env RPI_RELEASE_BASE_URL=file:///opt/e2e-release \
  rpi agent update --version 0.17.1
assert_log update.log 'installed'
assert_log update.log 'v0.17.1'

# The canonical binary is now the legacy build.
after=$("${SSH[@]}" rpi --version)
echo "$after" | grep -q '0.17.1' || fail "rpi --version after update: $after (expected 0.17.1)"

echo 'rpi e2e: PASS'
