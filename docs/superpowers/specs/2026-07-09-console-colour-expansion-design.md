# Console colour expansion

Date: 2026-07-09

## Problem

The colour work in `2026-07-08-colorful-console-output-design.md` shipped a
working styling layer (`crates/bin/src/output/`) whose colour detection is
correct — but it colours almost nothing. In practice the only coloured text
`rpi` ever emits is the semantic *marker glyph* (`✓`/`✗`/`⚠`), plus dim `note:`
lines, bold headings, and green/red `PASS`/`FAIL` in `rpi doctor`. Everything
else — the message text itself, the `deploy`/`command` log-pane frame, every
`ls`/`status`/`stats` table, the spinner — is monochrome. The result reads as
"barely coloured", and the emoji markers don't match the restrained
dot-marker aesthetic the user wants (Claude-Code style).

This is a cosmetic/UX change only. There is no correctness bug: `console`'s
auto-detection already enables colour on an attended terminal and disables it
under `NO_COLOR` / when piped (verified — an attended Windows Terminal renders
the styled `✗` today).

## Goal

1. Replace the emoji markers `✓ ✗ ⚠` with a single filled-dot marker `●`,
   coloured by semantic, with an ASCII fallback (`*`).
2. Tint the **whole** message line in its semantic colour (not just the
   marker), for `success`/`error`/`warn`. `note` stays dim (quiet secondary
   info). `heading` becomes cyan + bold.
3. Colour the `deploy`/`command` log pane: a neutral (grey) frame with a cyan
   label while streaming; on failure keep a **red** frame plus the full log
   dump; on success collapse as today into a single green summary line.
4. Colour tables: cyan bold headers, status values coloured by state, and
   CPU/MEM coloured by threshold.
5. Colour the spinner glyph (cyan).
6. Route every colour through **one semantic palette** so the whole CLI reads
   as one system and colours can be retuned in a single place.

Non-goals:
- No change to what is coloured under `NO_COLOR` / non-TTY / redirected output
  — those must still auto-disable, exactly as today. This is guaranteed for
  free because `console`, `comfy-table`, and `indicatif` all gate styling on
  their own TTY/colour detection; the only new code is a `force_no_tty()` call
  on tables when `NO_COLOR` is set (see Tables).
- No colour for the streamed docker/subprocess log *content*. It arrives
  monochrome because the agent builds with `BUILDKIT_PROGRESS=plain`
  (`crates/infrastructure/src/docker.rs`); that is a deliberate anti-flicker
  choice and out of scope here. The pane still displays any SGR a line
  happens to carry (`side_line` already preserves it) — we just don't
  manufacture colour that isn't there.
- No new CLI flags, no configurable themes (YAGNI), no new dependencies.
- `rpi logs` / `rpi agent logs` stay plain `println!` streams (unchanged).

## Semantic palette (`crates/bin/src/output/mod.rs`)

One place defines the meaning→style mapping; every surface references it.

| Role | Style | Used by |
|------|-------|---------|
| `success` | green | success marker+text, table `running`/healthy |
| `error` | red | error marker+text, failed pane frame, table `exited`/dead, over-threshold CPU/MEM |
| `warn` | yellow | warn marker+text, table `starting`/`restarting`/unhealthy, near-threshold CPU/MEM |
| `muted` | dim | `note:` lines, continuation text |
| `accent` | cyan | headings, pane label, table headers, spinner |
| `frame` | grey (bright-black) | neutral pane frame border |

Introduce small helpers so the mapping is testable as pure functions and so
no call site hard-codes a `console::Color`:

```rust
pub const MARKER: console::Emoji<'_, '_> = console::Emoji("●", "*");

// Semantic role -> console::Style. Kept in one place.
enum Sem { Success, Error, Warn, Muted, Accent }
fn sem_style(role: Sem) -> console::Style { /* green/red/yellow/dim/cyan */ }
```

`console::Style`/`style(..)` already renders as plain text when colour is
disabled, so all existing substring-matching tests keep passing.

## Message helpers (`output/mod.rs`)

Rewrite the semantic printers to a coloured `●` marker + fully-tinted text,
preserving today's stdout/stderr split and `for_stderr()` gating:

```rust
pub fn success(msg) { // stderr
    eprintln!("{} {}", green(MARKER).for_stderr(), green(msg).for_stderr());
}
pub fn error(msg)   { // stderr — red, marker bold
    eprintln!("{} {}", red_bold(MARKER).for_stderr(), red(msg).for_stderr());
}
pub fn warn(msg)    { // stderr
    eprintln!("{} {}", yellow(MARKER).for_stderr(), yellow(msg).for_stderr());
}
pub fn note(msg)    { eprintln!("{}", dim(format!("note: {msg}")).for_stderr()); } // unchanged
pub fn heading(msg) { println!("{}", cyan_bold(msg)); }                            // + cyan
```

`styled_ok`/`styled_err` (used by `render_doctor`) keep their current
green / red-bold contract — no change.

`init_colors()` is unchanged.

## Log pane (`output/logpane.rs`)

Give the frame a colour, driven by a small outcome enum so the border colour
is chosen in one place and the border helpers stay pure string functions
(they receive the already-styled pieces, or a `console::Style` to apply):

- **While streaming** (`redraw`): frame border in `frame` grey, label in
  `accent` cyan. `top_border`/`side_line`/`bottom_border` gain a
  `console::Style` (or pre-styled border string) parameter; the box glyphs
  and label are wrapped with it, the *content* between the side bars is left
  untouched (so any SGR the streamed line carries is preserved and the border
  colour can't bleed into it — `side_line` already appends a reset).
- **`finish_ok`** — unchanged behaviour: clear the pane (`clear_last_lines`),
  then `success(summary)` (now a green `●` line). Tidy happy path, no leftover
  box.
- **`finish_err`** — do **not** clear. Redraw the final frame once with a
  **red** border (last `max_visible` lines still shown) and leave it on
  screen as the "here it stopped" marker; then dump the complete captured log
  plain under a dim `— full log —` separator (so it reads as the complete
  record, not a repeat of the framed tail); then `error(summary)` (red `●`).
  The full dump stays because there is no log file — nothing needed for
  debugging may be lost.
- **`finish_neutral`** — unchanged: clear, then `note(summary)`.
- Non-interactive path (`!interactive`) is unchanged: plain per-line
  streaming, no frame, no colour manufactured.

Colour insertion goes through `console::style(..)`, so a `NO_COLOR` / non-TTY
run emits the same plain box it does today.

## Tables (`output/table.rs` + call sites in `cli/commands.rs`)

`comfy-table` 7.2.2 styles cells via `Cell::new(x).fg(Color::..)` /
`.add_attribute(..)` and auto-suppresses styling when stdout isn't a TTY.

- `table()` helper: when `NO_COLOR` is set, call `.force_no_tty()` so tables
  match the rest of the CLI (comfy-table checks TTY but not `NO_COLOR`
  itself). Header cells: cyan + bold.
- Semantic value colouring via shared pure mappers (unit-tested), reused by
  every table:
  - `status_style(state, health) -> Sem`: `running`/`healthy` → success,
    `starting`/`restarting`/`unhealthy` → warn, `exited`/`dead`/other →
    error.
  - `usage_style(percent) -> Sem`: `>=90` → error, `>=70` → warn, else none.
- Apply at the call sites that have the data:
  - `ls` (`commands.rs:271`): SERVICES cell coloured by aggregate — all
    services `running` → success, else warn/error. Other columns unchanged.
  - `status`/`stats` service tables (`:328`, `:342`): STATE/HEALTH via
    `status_style`, CPU/MEM via `usage_style`.
  - `render_doctor` (`:486`): unchanged (already `PASS`/`FAIL` styled).

## Spinner (`output/spinner.rs`)

Template `"{spinner:.cyan} {msg}"` so the animated glyph is cyan;
`indicatif` respects `console`'s colour state, so `NO_COLOR`/non-TTY is
handled for free. No behaviour change otherwise.

## Testing

Pure functions get direct unit tests; terminal/cursor behaviour is verified
manually against the mockup already agreed with the user.

- `sem_style` / `status_style` / `usage_style`: assert the role/threshold
  mapping (e.g. `usage_style(95) == Error`, `status_style("starting", None)
  == Warn`).
- Message helpers: existing "plain text when colours disabled" test extended
  to the new marker (`success("x")` etc. contain the plain text and no ANSI
  when colour is disabled — captured test output isn't a TTY).
- `top_border`/`side_line`/`bottom_border`: existing width/truncation tests
  keep passing; add one asserting that with colour disabled the styled and
  plain border strings are identical (no stray codes).
- Tables: `status_style`/`usage_style` covered above; `table()` still renders
  headers/rows (existing test).
- Workspace gate per `CLAUDE.md`: `cargo fmt --all -- --check`,
  `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`.
