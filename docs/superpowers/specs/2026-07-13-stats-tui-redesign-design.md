# rpi stats -w — dashboard TUI redesign

Date: 2026-07-13
Status: approved

## Context

`rpi stats -w/--watch` renders a full-screen `ratatui` realtime dashboard
(`crates/bin/src/cli/stats_tui.rs`), introduced in
[2026-07-12-rpi-stats-modes-design.md](2026-07-12-rpi-stats-modes-design.md).
Today it draws four regions: a one-line host summary, a single combined line
chart with three series (cpu%/mem%/temp on a shared 0..100 axis), a plain
services table (Project/Service/CPU/Mem), and a help line.

An approved visual redesign (Claude Design project `4a04e62c`, file
`TUI Dashboard Redesign.dc.html`) replaces this with a card-based dashboard. The
user's directive is **maximum visual fidelity to the mockup**, accepting the
extra machinery that requires. This spec covers the CLI-side TUI rewrite plus a
small backend change needed to feed the new **Status** column.

The command surface is unchanged: `stats_watch(api, project, interval)` keeps its
signature, flags, refresh loop, and quit keys. Only the drawing and the
view-model change (plus the `state`/`health` fields added to service stats).

## Approved mockup (the target)

Top-to-bottom, the mockup is:

1. **Window chrome** (`● ● ●` + `pi@raspberrypi: ~ — rpi stats`) — this is the
   mockup's fake terminal frame, **not** part of the TUI. Ignored.
2. **Status strip** — one line: `DISK 16%` and `UPTIME 4d 6h` (muted label + bold
   value), with a bottom rule. CPU/MEM/TEMP are **not** here anymore — they moved
   into the cards.
3. **Three metric cards** in a row (equal width, rounded borders):
   - `cpu%` — large value `0.9%` in accent `#C51A4A` + mini line in accent.
   - `mem%` — large value `11.2%` in green `#75A928` + mini line in green.
   - `temp°C` — large value `48.5°C` in magenta `#D96AE0` + mini line in magenta.
4. **`services` panel** — rounded border, title on the top rule. Columns
   `Project / Service / CPU / Mem / Status`. Rows carry an accent `▸` marker,
   zebra striping, an inline memory bar next to the byte value, and a
   `● running` status pill.
5. **Footer** — `q/Esc/Ctrl-C` (accent, bold) + ` quit` (muted).

Palette from the mockup: accent `#C51A4A`, success `#75A928`, temp `#D96AE0`,
muted `#84838a`, border `#38373d`, track `#242327`, bar-fill `#59585f`, zebra bg
`#101013`, page bg `#0b0b0d`.

## Scope decisions (settled)

- **Drop the big combined chart.** The mockup has no shared 0..100 chart; the
  three per-metric mini-lines replace it. Accepted trade-off: each mini-line
  shows less history detail than today's wide chart.
- **Status strip carries only DISK + UPTIME.** UPTIME is new to this view;
  format `uptime_secs` as `Nd Nh` / `Nh Nm` / `Nm`.
- **Add a real Status column** (chosen: option A). Propagate container
  `state` + `health` from the agent into service stats and render a semantic
  pill. Not faked, not dropped.
- **New dependency: `tui-big-text`** for the large card numbers. Verified
  compatible with the workspace `ratatui = "0.30"`: `tui-big-text 0.8.8` depends
  on `ratatui-core ^0.1` + `ratatui-widgets ^0.3`, the split crates that
  ratatui 0.30 is built on. Adds transitive `font8x8`, `derive_builder`.
- **Force the dark page background** (`#0b0b0d`) on the alt-screen only when
  truecolor is active; keep the terminal's own background under 256-color / no
  color so we never paint a jarring dark box on a light terminal.
- **Keep the pure view-model split.** `build_frame` stays terminal-independent
  and unit-testable; only its shape grows. All rendering reads from it.

## Backend change — service state/health (for the Status column)

The data already exists at collection time and is discarded. In
`crates/infrastructure/src/docker.rs`, `stats()` runs `docker compose ps
--format json` (whose lines carry `State` and `Health`, see `service_state`)
and joins it with `docker stats` in `parse_stats_json`. Today that join only
maps `Name -> Service`.

Changes:

- **`pi_domain::entities::ServiceStats`** gains `state: String` and
  `health: Option<String>`.
- **`parse_stats_json`** builds `Name -> (service, state, health)` from the ps
  JSON and populates the new fields on each `ServiceStats`. Containers absent
  from `docker stats` are still skipped (unchanged join behaviour).
- **`ServiceStatsDto`** (`crates/bin/src/proto.rs`) gains
  `#[serde(default)] state: String` and `#[serde(default)] health:
  Option<String>`, with the `From<ServiceStats>` impl updated. `#[serde(default)]`
  keeps a new CLI working against an **old agent**: missing fields → empty
  `state` → rendered as a neutral, blank pill (never a fake "running").
- All `ServiceStats { .. }` literals in tests get the two new fields.

Rendering maps state via the existing
`output::status_sem(state, health) -> Sem` (running/healthy = success,
restarting/paused/created / unhealthy = warn, exited/dead/unknown = error).

## Layout (ratatui)

Vertical split of `f.area()`:

| Region   | Constraint        | Contents |
|----------|-------------------|----------|
| strip    | `Length(2)`       | DISK + UPTIME line + bottom rule |
| cards    | `Length(H)`       | three metric cards (`H` = 10 rich / 6 compact) |
| services | `Fill(1)`         | services table (top-aligned rows) |
| footer   | `Length(1)`       | key hints |

**Cards row** — `Layout::horizontal([Fill(1); 3])`. Each card is a
`Block::bordered().border_type(BorderType::Rounded)` with border style
`#38373d`, split vertically into: label line (`cpu%` etc., muted) / big value /
mini line-chart.

- **Big value** — `tui_big_text::BigText` with a compact `PixelSize`
  (`Quadrant`, ≈4 rows and ≈4 cols per glyph, tuned to fit a third-width card),
  colored per metric. Only the numeric part is drawn big; the unit suffix
  (`%`, `°C`) is normal text beside it, because the `font8x8` BASIC set has no
  `°` glyph. Degraded mode (see below) renders the value as plain **bold** text.
- **Mini line-chart** — borderless `Chart` with one `Dataset`, `Marker::Braille`,
  `GraphType::Line`, no axes/labels, colored per metric. X-bounds span the full
  history; Y-bounds fit the series `[min, max]` with small padding so the line
  fills the card's ≈2–3 rows (matching the mockup polyline, which wiggles within
  the mini-area rather than sitting against a 0..100 baseline).

**Services table** — `Table` with five columns using proportional `Fill`
weights mirroring the mockup ratios (`13 / 15 / 7 / 16 / 9`), each with a small
minimum. Header: uppercase muted labels. Cells are `Line`s of styled spans:

- *Project*: `▸`(accent) + ` ` + project name.
- *Service*: default fg.
- *CPU*: `{:.1}%` muted (`#84838a`).
- *Mem*: `"{used} "` + inline bar. Bar uses eighth-block glyphs
  (`▏▎▍▌▋▊▉█`) for sub-cell precision, fill `#59585f` over a `#242327` track,
  width scaled to `mem_used / max(mem_used across rows)` (reads as "who eats the
  most"). When `mem_limit_bytes == 0` (Pi memory cgroup disabled → `docker
  stats` reports `0B/0B`), render `n/a` muted with **no** bar.
- *Status*: `●`(sem color) + ` ` + state text, painted with the semantic color;
  a dim same-hue cell background approximates the pill when truecolor is active.
  Empty state (old agent) → blank.

Zebra striping via `Row::style` on alternate rows (bg `#101013`). Footer is a
`Paragraph` line: `q/Esc/Ctrl-C`(accent bold) + ` quit`(muted).

## Colors / theme

Reuse the existing `Paint::Rgb` + truecolor detection so every hue downgrades to
the nearest xterm-256 on non-truecolor terminals, identically to tables and the
current chart.

- **cpu** = `output::accent_ratatui_color()` (theme-driven; already the raspberry
  `Rgb(197,26,74)` = `#C51A4A`, and respects theme overrides).
- **mem** = fixed `Rgb(117,169,40)` (`#75A928`).
- **temp** = fixed `Rgb(217,106,224)` (`#D96AE0`).

Add small helpers in `crates/bin/src/output/mod.rs` mirroring
`accent_ratatui_color`: `mem_ratatui_color()`, `temp_ratatui_color()`, and
`sem_ratatui_color(Sem)` (for the status pill), each honoring
`theme::truecolor_enabled()`. Border/track/muted/zebra greys are applied as
`Style` colors on the drawn widgets. The forced page background (`#0b0b0d`) is
painted once over `f.area()` before the regions, gated on truecolor.

## Responsive degradation

The rich layout is tall. A pure `fn layout_mode(area: Rect) -> Mode` picks:

- **Rich** (default): big-text numbers, full mini-charts, cards height 10.
- **Compact** (`area.height < ~20` or `area.width < ~70`): plain bold numbers,
  shorter mini-charts, cards height ≈6.
- **Tiny** (`area.height < ~10`): fall back to a single-line host summary (the
  current-style `CPU … MEM … TEMP … DISK`) + services table only, so the
  dashboard never overflows or panics on a small window.

`layout_mode` is unit-tested independently of any terminal.

## Fidelity limits (honest, terminal-inherent)

- **No rounded pill corners, no shadows** — the status pill is color + optional
  background; corners are square.
- **Big numbers are bitmap, not proportional 28px** — `tui-big-text` gives large
  block glyphs; `°` is drawn as normal text.
- **Pixel spacing → integer cell grid.**
- **Exact hex needs truecolor** (`COLORTERM=truecolor|24bit`); otherwise nearest
  xterm-256 (existing downgrade path).
- **Per-service memory is often `n/a` on a real Pi** (memory cgroup disabled);
  the mem value + bar degrade to `n/a` with no bar, unlike the mockup's sample
  numbers.

## Testing

- Extend `build_frame` and its view-model; keep it terminal-free. Add unit tests:
  - uptime formatting (`4d 6h`, `6h 5m`, `3m`),
  - mem-bar ratio scaling (relative to max used; `None`/no-bar when limit 0),
  - status → `Sem` wiring per state/health,
  - `layout_mode` thresholds (Rich/Compact/Tiny by `Rect`).
- Add a `ratatui::backend::TestBackend` render test: draw one frame at a fixed
  size into a `Buffer` and assert it does not panic and that the region labels
  (`services`, `cpu%`, key hints) are present.
- Backend: unit-test `parse_stats_json` now carries `state`/`health`; DTO
  round-trip test that an old-agent payload without the fields deserializes
  (`#[serde(default)]`).
- Before done, per repo rules: `cargo fmt --all -- --check`,
  `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`.

## Risks / open items

- **`tui-big-text` glyph fit** in a third-width card at small terminal widths —
  mitigated by the compact `PixelSize` + Compact/Tiny degradation. Exact
  `PixelSize` variant is tuned during implementation against real widths.
- **Forced background** could still look off on unusual terminals; gated on
  truecolor and drawn only inside the alt-screen (restored on exit by the
  existing `TerminalGuard`).
- **Status pill background** fidelity is limited; if it reads poorly at 256
  colors, fall back to colored text with no background.
