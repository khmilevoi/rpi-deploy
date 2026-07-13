#!/bin/sh
# One-line installer for the rpi deploy CLI/agent binary (no npm/node needed).
# Usage:  curl -fsSL <raw-url>/scripts/install.sh | sh
# Env overrides:
#   RPI_VERSION           target version (default: latest published release)
#   RPI_INSTALL_DIR       install dir    (default: /usr/local/bin)
#   RPI_RELEASE_BASE_URL  release download root (default: GitHub Releases)
#   RPI_RELEASE_API_URL   GitHub API base (default: api.github.com repo)
# Shares the download/verify recipe with scripts/postinstall.js and the
# board-side `rpi agent update`.
set -eu

REPO="khmilevoi/rpi-deploy"
BASE_URL="${RPI_RELEASE_BASE_URL:-https://github.com/${REPO}/releases/download}"
API_URL="${RPI_RELEASE_API_URL:-https://api.github.com/repos/${REPO}}"
INSTALL_DIR="${RPI_INSTALL_DIR:-/usr/local/bin}"

log() { echo "rpi-install: $*"; }
fail() { echo "rpi-install: error: $*" >&2; exit 1; }

# 1. arch -> target triple
arch="$(uname -m)"
case "$arch" in
  aarch64 | arm64) triple="aarch64-unknown-linux-musl" ;;
  x86_64 | amd64) triple="x86_64-unknown-linux-musl" ;;
  *) fail "unsupported architecture: $arch" ;;
esac

# 2. resolve version
version="${RPI_VERSION:-}"
if [ -z "$version" ]; then
  version="$(curl -fsSL -H 'Accept: application/vnd.github+json' "${API_URL}/releases/latest" \
    | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -n1 \
    | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\{0,1\}\([^"]*\)".*/\1/')"
  [ -n "$version" ] || fail "could not resolve the latest release version"
fi
version="${version#v}"

asset="rpi-v${version}-${triple}.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# 3. download + verify
log "downloading ${asset} ..."
curl -fsSL -o "${tmp}/${asset}" "${BASE_URL}/v${version}/${asset}"
curl -fsSL -o "${tmp}/SHA256SUMS" "${BASE_URL}/v${version}/SHA256SUMS"

expected="$(awk -v a="$asset" '$2 == a || $2 == "*" a { print $1; exit }' "${tmp}/SHA256SUMS")"
[ -n "$expected" ] || fail "${asset} not listed in SHA256SUMS"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "${tmp}/${asset}" | awk '{ print $1 }')"
else
  actual="$(shasum -a 256 "${tmp}/${asset}" | awk '{ print $1 }')"
fi
[ "$actual" = "$expected" ] || fail "sha256 mismatch for ${asset}: expected ${expected}, got ${actual}"

# 4. extract + install
tar -xf "${tmp}/${asset}" -C "$tmp"
[ -f "${tmp}/rpi" ] || fail "archive did not contain rpi"
chmod 0755 "${tmp}/rpi"

if [ -w "$INSTALL_DIR" ]; then
  install -m 0755 "${tmp}/rpi" "${INSTALL_DIR}/rpi"
else
  log "sudo required to write ${INSTALL_DIR}"
  sudo install -m 0755 "${tmp}/rpi" "${INSTALL_DIR}/rpi"
fi

log "installed rpi v${version} to ${INSTALL_DIR}/rpi"

# 5. next steps (do NOT run setup here)
log "next steps:"
log "  Raspberry Pi agent: sudo rpi agent setup   (Docker must already be installed)"
log "  developer machine:  rpi setup"
