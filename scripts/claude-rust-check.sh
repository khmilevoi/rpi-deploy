#!/usr/bin/env bash
# Fast incremental feedback for Claude Code's PostToolUse hook. After an edit to
# a .rs file, run `cargo check` and, on failure, feed the compiler errors back
# into the model's context so it can fix them immediately instead of waiting for
# the heavier Stop-time gate. Stays silent (and cheap) when the edit isn't Rust
# or the workspace still compiles.
set -uo pipefail

input=$(cat)

# Extract the edited file path from the hook payload (no jq on this host).
file=$(printf '%s' "$input" | node -e '
  let s = "";
  process.stdin.on("data", (d) => (s += d)).on("end", () => {
    try {
      const j = JSON.parse(s);
      const p =
        (j.tool_input && j.tool_input.file_path) ||
        (j.tool_response && j.tool_response.filePath) ||
        "";
      process.stdout.write(String(p));
    } catch (_) {
      /* malformed payload -> empty path -> no-op below */
    }
  });
')

# Only react to Rust source edits; everything else is a no-op.
case "$file" in
  *.rs) ;;
  *)
    printf '{}'
    exit 0
    ;;
esac

if out=$(rtk cargo check --all-targets --locked 2>&1); then
  # Compiles cleanly — say nothing, stay out of the way.
  printf '{}'
else
  CHECK_OUTPUT="$out" node -e '
    const out = process.env.CHECK_OUTPUT.slice(0, 3000);
    const ctx = "`cargo check` failed after this edit:\n" + out;
    process.stdout.write(
      JSON.stringify({
        hookSpecificOutput: {
          hookEventName: "PostToolUse",
          additionalContext: ctx,
        },
        suppressOutput: true,
      })
    );
  '
fi
