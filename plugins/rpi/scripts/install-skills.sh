#!/usr/bin/env sh
set -eu

target="${1:-both}"
plugin_root="$(CDPATH= cd "$(dirname "$0")/.." && pwd)"
source_dir="$plugin_root/skills"

codex_dir="${CODEX_HOME:-$HOME/.codex}/skills"
claude_dir="$HOME/.claude/skills"

install_to() {
  destination="$1"
  mkdir -p "$destination"
  for skill in "$source_dir"/*; do
    [ -d "$skill" ] || continue
    name="$(basename "$skill")"
    rm -rf "$destination/$name"
    cp -R "$skill" "$destination/$name"
    printf 'Installed %s -> %s\n' "$name" "$destination/$name"
  done
}

case "$target" in
  codex)
    install_to "$codex_dir"
    ;;
  claude)
    install_to "$claude_dir"
    ;;
  both)
    install_to "$codex_dir"
    install_to "$claude_dir"
    ;;
  *)
    printf 'usage: %s [codex|claude|both]\n' "$0" >&2
    exit 2
    ;;
esac
