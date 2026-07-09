# CLI brand visuals (package A)

Date: 2026-07-09

## Problem

v0.14.0 (`2026-07-09-console-theme-branding-design.md`) unified `rpi`'s output
behind one `Theme`/`Sem`/`Paint` layer: raspberry accent, `▸` marker, themed
messages / tables / spinner. The next step is to make the CLI *feel* branded,
not just tinted:

1. every command already carries the raspberry `▸` marker (accent colour is
   done) — no further per-command branding is wanted;
2. `rpi deploy` should open with a real ASCII logo — a raspberry triangle with
   a character gradient — and close with a satisfying result stamp;
3. `rpi` with no arguments and `rpi --version` should show the logo banner;
4. the brand colour should render as the exact `#C51A4A` where the terminal
   and rendering backend allow it (truecolor), not only the 256-colour
   approximation.

This is **package A** of a two-part effort. **Package B** — a deploy pipeline
view (build→push→start→health as timed phases) — is a separate spec because it
requires a new agent→CLI SSE `stage` event (protocol + behaviour change),
whereas everything here is pure CLI-side presentation with no protocol change.

## Scope decisions (already settled)

- **Big triangle logo only on `rpi deploy`** and on the bare `rpi` /
  `--version` banner. All other commands keep today's look (just the `▸`
  marker) — no per-command header, to avoid noise in scriptable commands.
- **Gradient:** vertical character-density ramp, light on top → solid on the
  bottom (`░ ▒ ▓ ▓ █` by row). On truecolor terminals the ramp is additionally
  tinted with a colour sweep, pink on top → raspberry on the bottom, so density
  and colour intensify together downward.
- **Deploy result stamp** shows status glyph, project, URL, and elapsed time.
  It does **not** show a service count — that needs agent data and belongs to
  package B.
- **Truecolor** is applied only where the backend can emit 24-bit colour
  (see §Truecolor). Messages, the marker, and the spinner stay 256-colour.

## Components

All new code lives in `crates/bin/src/output/` plus small wiring in
`crates/bin/src/main.rs` and `crates/bin/src/cli/commands.rs`. No change to the
agent, proto, domain, or application crates.

### `output/banner.rs` (new)

Pure string builders from the active `Theme`; callers print the returned
`String`. Nothing here reads argv or performs IO except the TTY check helper.

- **Triangle geometry** — 5 rows, right-pointing (flat left edge, apex right),
  row widths `2,4,6,4,2`:

  ```
  ░░
  ▒▒▒▒       r p i
  ▓▓▓▓▓▓     deploy · myboard
  ▓▓▓▓
  ██
  ```

  Row fill glyph is the vertical density ramp `[░, ▒, ▓, ▓, █]` (one glyph per
  row, top→bottom). The wordmark (`r p i` + a subtitle line) sits to the right,
  vertically centred against the triangle (rows 2–3).

- `deploy_banner(project: &str) -> String` — triangle + `r p i` /
  `deploy · <project>`.
- `brand_banner(version: &str) -> String` — triangle + `r p i vX.Y.Z` /
  `deploy anything to your Pi` + a dim `rpi.iiskelo.com` line. Used by bare
  `rpi` and `--version`.
- `deploy_stamp(outcome, project, url: Option<&str>, elapsed) -> String` — one
  line:
  - success: `▸ deployed ✓  <project>  →  <url>   (12.4s)` (accent marker,
    green `✓`), URL omitted when none is known;
  - superseded: `▸ deploy superseded  <project>   (a newer deploy replaced
    this one)` — neutral;
  - failed: `▸ deploy failed ✗  <project>   (see log above)` — red `✗`.
  `✓`/`✗` and the triangle/ramp glyphs each have an ASCII fallback (`✓`→`ok`,
  `✗`→`x`), rendered via the same unicode-capability check `console::Emoji`
  uses. When unicode is unavailable the banner degrades to a plain wordmark
  (no triangle): `rpi — deploy · <project>`.

- `show_banner_to_stderr()` gate: print the multi-line banner **only when
  stderr is a TTY** (`console::Term::stderr().is_term()`). Under a pipe, file,
  or CI, the banner is skipped entirely so logs stay clean. Colour within the
  banner still obeys the existing gating (`NO_COLOR`, `console` colour state).
  The stamp is a single informational line and is always printed (uncoloured
  when colour is off).

### Truecolor (`output/theme.rs`)

Backend capabilities differ, so truecolor is layered honestly:

| Surface | Backend | Truecolor? | Behaviour |
|---------|---------|-----------|-----------|
| Tables | `comfy-table` 7 | yes (`Color::Rgb`) | exact `#C51A4A` when capable, else `AnsiValue(161)` |
| Banner + stamp | our own string | yes (manual SGR) | emit `\x1b[38;2;r;g;bm…\x1b[39m` when capable, else `color256(161)` |
| Messages, marker | `console` 0.15 | no | stay `color256` (161) — delta on one glyph is imperceptible |
| Spinner | `indicatif` template | no | stays 256/named token |

- New `theme::truecolor_enabled() -> bool` = colours are enabled **and**
  `COLORTERM` ∈ {`truecolor`, `24bit`}. Pure w.r.t. an injected env lookup so
  it is unit-testable.
- New `Paint::sgr_fg(&self) -> String` returning the SGR foreground params
  (`"38;2;197;26;74"` for `Rgb` under truecolor, `"38;5;161"` otherwise). The
  banner/stamp painter wraps text with it and a `\x1b[39m` reset, gated on
  `console::colors_enabled()`; when colour is off it returns the text
  unwrapped.
- `Paint::table()` gains capability awareness: `Color::Rgb { r, g, b }` under
  truecolor, `Color::AnsiValue(idx)` otherwise. `console_style()` (messages)
  and the spinner token are unchanged.
- Truecolor colour sweep for the banner: rows interpolate linearly from pink
  `#F06CA0` (row 1) to raspberry `#C51A4A` (row 5). Off-truecolor the banner
  uses the single accent colour and relies on the character ramp for the
  gradient.

### Wiring: bare `rpi` and `--version` (`main.rs`)

- Make the subcommand optional: `cmd: Option<Cmd>`. When `None`, print
  `brand_banner` to stderr (TTY-gated) plus a one-line pointer to `rpi --help`,
  and exit `0`.
- Take over version output: `#[command(disable_version_flag = true)]` and add a
  top-level `#[arg(short = 'V', long = "version")] version: bool`. When set,
  print `brand_banner` (colour art on a TTY, else plain `rpi X.Y.Z`) and exit
  `0` before dispatching any subcommand.

### Wiring: `deploy` (`cli/commands.rs`)

- Print `deploy_banner(&rpitoml.project.name)` at the start of `deploy()`
  (TTY-gated), before the version/status lines.
- Start a wall-clock `Instant` before `api.deploy`; on completion format the
  elapsed via the existing `crate::duration` module.
- Resolve the display URL from the project's ingress hostname in `rpi.toml`;
  fall back to the LAN `host:port` when there is no public hostname, or `None`
  when neither applies.
- After `follow_logs` returns, print `deploy_stamp(...)` mapping the returned
  status (`success` / `superseded` / other→failed). The `LogPane`'s own final
  line becomes secondary: keep the pane's neutral close but let the stamp be
  the headline result, so the outcome is stated once, not twice.

## Error handling / edge cases

- The banner never corrupts output: non-TTY → skipped; `NO_COLOR` → no colour;
  non-unicode terminal → plain wordmark, no triangle.
- stdout stays machine-clean: banner, stamp, and all status lines go to stderr;
  `--json` output and SSE/log streams are untouched.
- Unknown/absent `COLORTERM` → 256-colour path (safe default); raw 24-bit
  escapes are never emitted to terminals that did not advertise truecolor.

## Testing

- `banner`: `deploy_banner` / `brand_banner` contain the wordmark, the subtitle,
  and (unicode branch) the ramp glyphs; ASCII branch contains the plain
  wordmark and no block glyphs; neither contains ANSI when colour is disabled.
- `deploy_stamp`: success/superseded/failed variants carry the project and the
  right status token (`✓`/`ok`, `✗`/`x`); URL present only when supplied;
  elapsed rendered via `duration`.
- `truecolor_enabled()`: true only when colour is enabled and `COLORTERM` ∈
  {`truecolor`,`24bit`}; false for empty/`256color`/absent.
- `Paint::sgr_fg`: `Rgb` → `38;2;197;26;74` under truecolor, `38;5;161`
  otherwise; named paints → their `38;5;n`.
- `Paint::table`: `Color::Rgb` under truecolor, `Color::AnsiValue` otherwise;
  classic theme keeps named ANSI colours.
- `main.rs` clap: subcommand is optional and parses to `None`; `-V/--version`
  parses to a bool; all existing parse tests stay green.
- Existing `output` tests (marker, info/status lines, table render, spinner
  lifecycle, logpane widths, `rgb_to_ansi256`) stay green unchanged.
- Workspace gate per `CLAUDE.md`: `cargo fmt --all -- --check`,
  `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`.

## Non-goals (→ package B or later)

- Deploy phases as a timed pipeline (build→push→start→health) and the required
  agent→CLI `stage` SSE event.
- Service-count in the deploy stamp (needs agent data).
- Truecolor for the spinner (indicatif template limitation) or reworking the
  message hot path to truecolor (256 is visually sufficient there).
- Any new CLI flags beyond the `-V/--version` and bare-`rpi` banner behaviour;
  `PI_THEME` remains the only theme switch.
