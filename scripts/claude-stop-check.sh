#!/usr/bin/env bash
# CI-parity gate for Claude Code's Stop hook — mirrors the checks required
# by CLAUDE.md before a task can be considered done. Blocks the stop with
# the failing command's output when a check fails.
set -uo pipefail

# Skip the (expensive) CI-parity gate entirely when no Rust sources changed.
# Text-only turns and doc/config-only edits don't need fmt/clippy/test, and the
# PostToolUse `cargo check` hook already gives fast feedback while editing. If
# git is unavailable we fall through and run the gate (fail-safe).
if command -v git >/dev/null 2>&1; then
  rs_changed=$(git status --porcelain --untracked-files=all 2>/dev/null | grep -E '\.rs"?$' || true)
  if [ -z "$rs_changed" ]; then
    printf '{}'
    exit 0
  fi
fi

output=""
failed=""

check() {
  local label="$1"
  shift
  local out
  if ! out=$("$@" 2>&1); then
    output="$out"
    failed="$label"
    return 1
  fi
  return 0
}

if check "cargo fmt" rtk cargo fmt --all -- --check; then
  if check "cargo clippy" rtk cargo clippy --all-targets --locked -- -D warnings; then
    check "cargo test" rtk cargo test --locked
  fi
fi

if [ -n "$failed" ]; then
  FAILED_CHECK="$failed" FAILED_OUTPUT="$output" node -e '
    const reason = "`" + process.env.FAILED_CHECK + "` failed:\n" + process.env.FAILED_OUTPUT.slice(0, 4000);
    process.stdout.write(JSON.stringify({ decision: "block", reason }));
  '
else
  printf '{}'
fi
