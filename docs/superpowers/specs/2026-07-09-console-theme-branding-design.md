# Console theme layer + rpi brand styling

Date: 2026-07-09

## Problem

`2026-07-09-console-colour-expansion-design.md` shipped a semantic styling
layer (`crates/bin/src/output/`), but its palette is hard-coded in three
places: `console_style()` maps `Sem::Accent` to cyan, `table.rs` maps header
and accent cells to `Color::Cyan`, and `spinner.rs` embeds `{spinner:.cyan}`
in its template. The marker glyph `●` is a module constant.

Meanwhile the product now has a visual identity (the `rpi-deploy-site`
landing): a raspberry-red right-pointing triangle `▸` on dark ground, with a
supporting palette. The user wants:

1. every console response to carry the brand — the `▸` triangle in the
   raspberry accent colour, like the logo;
2. the whole output pipeline unified behind **one theme object**, so the
   palette can be changed at any moment (new theme = one constructor, no
   call-site edits).

This supersedes the "no configurable themes (YAGNI)" non-goal of the previous
spec — themes are now an explicit requirement.

## Brand palette (from `rpi-deploy-site/styles.css`)

| Token | RGB | Nearest xterm-256 | Terminal role |
|-------|-----|-------------------|---------------|
| raspberry | `#C51A4A` | 161 | accent (marker, headings, spinner, table headers, pane label) |
| green | `#75A928` | 106 | success |
| amber | `#d4a017` | 178 | warn |
| — (site has no error colour) | — | terminal red | error |

The `console` crate has no truecolor support, so RGB values are reduced to
xterm-256 via one shared conversion function (nearest point in the 6×6×6
cube / grey ramp). The same index feeds `console` (messages, pane, spinner)
and `comfy-table` (`Color::AnsiValue`), so every surface shows the identical
colour.

## Theme layer (`crates/bin/src/output/theme.rs`, new)

One struct is the single source of truth for *how roles look*; `Sem` remains
the single source of truth for *what a colour means*.

```rust
/// How a role is painted. `Rgb` renders as the nearest xterm-256 colour.
pub enum Paint {
    Default,               // terminal default, no colour
    Ansi(console::Color),  // named ANSI colour (classic theme)
    Rgb(u8, u8, u8),       // brand colour, reduced to 256 via rgb_to_ansi256()
}

pub struct Theme {
    pub accent: Paint,
    pub success: Paint,
    pub warn: Paint,
    pub error: Paint,
    // muted stays a dim modifier, frame stays bright-black — not palette slots
    pub marker: (&'static str, &'static str), // (unicode, ascii fallback)
    pub marker_accent: bool, // true: marker always accent; false: marker follows Sem
}
```

- `Theme::raspberry()` — **default**: accent `Rgb(197,26,74)`, success
  `Rgb(117,169,40)`, warn `Rgb(212,160,23)`, error `Ansi(Red)`, marker
  `("▸", ">")`, `marker_accent: true`.
- `Theme::classic()` — today's palette, kept as the escape hatch and as
  proof the mechanism works: accent `Ansi(Cyan)`, success/warn/error named
  ANSI, marker `("●", "*")`, `marker_accent: false`. A theme controls only
  palette and glyphs; the structural changes below (marker prefixes on
  `heading`/`note`, the new `info()`) apply under every theme.
- Active theme: `OnceLock<Theme>`, initialised lazily on first access from
  `PI_THEME` (`raspberry` default; `classic`; unknown values silently fall
  back to `raspberry` so scripts never break). Selection logic lives in a
  pure `Theme::from_env_value(Option<&str>)` for testability.
- Converters (all pure): `Paint -> console::Style`,
  `Paint -> comfy_table::Color`, `Paint -> String` (indicatif template colour
  token: a 0-255 number or ANSI name).

`rgb_to_ansi256()` is unit-tested against the three brand colours
(161/106/178).

## What changes on each surface

All call sites keep talking to `Sem`; only the role→style mapping moves into
the theme.

- **`console_style(Sem)`** (`output/mod.rs`) — reads the active theme instead
  of hard-coding colours. `Muted` stays dim, `Frame` stays bright-black,
  `Neutral` stays a no-op.
- **Marker** — glyph and colour come from the theme. With
  `marker_accent: true` (raspberry) every status line is
  `▸ message`: bold **raspberry** triangle (the logo), message text tinted by
  its own role (success green / warn amber / error red) exactly as today.
- **`heading()`** — gains the marker prefix: `▸ Heading` in accent bold.
- **`note()`** — gains the marker prefix: accent `▸` + dim `note: …` text,
  so notes carry the brand too.
- **New `info()`** (`output/mod.rs`) — accent `▸` + untinted text, for
  informational lines that today go through raw `println!`. Prints to stdout
  (it is answer content, not status).
- **Raw `println!`/`eprintln!` call sites** (`cli/commands.rs`,
  `cli/init.rs`, `cli/setup.rs`, `agent/setup.rs`, `agent/run.rs`) — human
  informational lines are routed through `info()`/existing semantic helpers.
  **Excluded (must stay unbranded):** `--json` output, SSE/log streams
  (`stream_sse`, `rpi logs`, log-pane line content), table bodies,
  machine-readable lists (`agent migrate --list` TSV), and secondary detail
  lines that read as continuations (they stay plain or dim, listed
  case-by-case in the implementation plan).
- **Spinner** (`output/spinner.rs`) — template built dynamically from the
  theme's accent token: `{spinner:.161} {msg}` under raspberry,
  `{spinner:.cyan}` under classic. `indicatif` gates on `console`'s colour
  state, so `NO_COLOR`/non-TTY stays handled.
- **Tables** (`output/table.rs`) — `header()` and `sem_colour()` read the
  theme (`comfy_table::Color::AnsiValue(161)` etc.).
- **Log pane** (`output/logpane.rs`) — no code change: the label already
  renders via `Sem::Accent` and the error frame via `Sem::Error`, so it
  follows the theme automatically.

## ratatui-tui skill

Its style principles are applied (semantic colour mapping, symbol + colour —
never colour alone, one palette, terminal-theme friendliness). The `ratatui`
crate itself is **not** added: `rpi` is a line-oriented CLI, not a
full-screen TUI. The `Paint`/`Theme` shape converts to `ratatui::Style` with
one additional function if a full-screen dashboard is ever built.

## Non-goals

- No truecolor engine swap; 256-colour approximation is accepted
  (`#C51A4A` → 161 is visually near-identical).
- No `--theme` CLI flag for now; `PI_THEME` env var only.
- No change to colour auto-disable semantics (`NO_COLOR`, non-TTY, piping) —
  all styling still flows through `console`/`comfy-table`/`indicatif` gating.
- No colouring of streamed subprocess/log content.

## Testing

- `rgb_to_ansi256()`: brand colours map to 161/106/178; grey and cube edge
  cases.
- `Theme::from_env_value`: `None`/`"raspberry"` → raspberry, `"classic"` →
  classic, unknown → raspberry.
- Paint converters: `Paint::Default` is a no-op style; `Rgb` produces a
  `color256` style / `AnsiValue` cell / numeric template token.
- Message helpers: existing "plain text when colours disabled" tests keep
  passing with the new marker; `info()`/`note()`/`heading()` contain the
  marker glyph and no ANSI when colours are off.
- Existing `output` tests (logpane widths, table render, spinner lifecycle,
  sem mappers) keep passing unchanged.
- Workspace gate per `CLAUDE.md`: `cargo fmt --all -- --check`,
  `cargo clippy --all-targets --locked -- -D warnings`,
  `cargo test --locked`.
