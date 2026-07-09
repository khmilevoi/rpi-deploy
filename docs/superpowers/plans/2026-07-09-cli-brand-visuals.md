# CLI brand visuals (package A) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `rpi` a branded look — a raspberry triangle logo with a vertical character gradient on `rpi deploy`, the same logo on bare `rpi` / `--version`, a deploy result stamp, and exact-`#C51A4A` truecolor where the backend allows it.

**Architecture:** All work is CLI-side presentation. A new `crates/bin/src/output/banner.rs` holds pure string builders (triangle, colour, banner, stamp) driven by fixed brand colours; `theme.rs` gains a truecolor capability probe used by tables; `commands.rs` and `main.rs` do the printing. No agent, proto, domain, or application crate changes — the deploy SSE protocol is untouched.

**Tech Stack:** Rust, `clap` (derive), `console` 0.15 (styling/TTY/unicode detection — no truecolor), `comfy-table` 7 (`Color::Rgb` for truecolor tables), `indicatif` (unchanged).

## Global Constraints

- Workspace gate (must pass before any task is "done", matches Linux CI):
  `rtk cargo fmt --all -- --check`,
  `rtk cargo clippy --all-targets --locked -- -D warnings`,
  `rtk cargo test --locked`.
- Clippy runs with `-D warnings`: no dead code, no unused imports. If a step
  removes the last caller of a function, remove the function in the same step.
- Package name for focused tests is `pi` (the binary is `rpi`):
  `rtk cargo test -p pi <name>`.
- Brand colours (fixed, independent of `PI_THEME` — the logo is always
  raspberry): gradient sweep is pink `#F06CA0` = `(240, 108, 160)` at the top
  row → raspberry `#C51A4A` = `(197, 26, 74)` at the bottom row.
- Prefix git/cargo with `rtk` per repo `CLAUDE.md`.
- Branch: work continues on `cli-brand-visuals` (already created; the design
  spec is committed there).

### Refinements to the spec (intentional, simpler than the written spec)

1. The deploy **stamp** is rendered through the existing message path
   (`LogPane::finish_ok/finish_neutral/finish_err` → `output::success/note/error`),
   so it inherits the `▸` marker and 256-colour styling. Only the **banner**
   uses manual truecolor. Rationale: the stamp is a single line where 256 vs
   exact is imperceptible, and this avoids adding `LogPane` API surface.
2. The stamp **URL** comes only from `rpi.toml` `[ingress].hostname`; when
   absent the URL is omitted. The spec's "LAN host:port fallback" is dropped —
   the CLI does not reliably know the host's LAN IP (that line is emitted by
   the agent).

Reference spec: `docs/superpowers/specs/2026-07-09-cli-brand-visuals-design.md`.

---

### Task 1: `format_elapsed` duration helper

Foundation for the deploy stamp: turn a wall-clock `Duration` into `12.4s` /
`1m03s`.

**Files:**
- Modify: `crates/bin/src/duration.rs` (add function + tests)

**Interfaces:**
- Produces: `pub(crate) fn format_elapsed(d: std::time::Duration) -> String`
  — `< 60s` → `"{:.1}s"` (e.g. `12.4s`, `0.5s`); `>= 60s` → `"{m}m{ss:02}s"`
  (e.g. `1m03s`).

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/bin/src/duration.rs`:

```rust
    #[test]
    fn format_elapsed_sub_minute_uses_one_decimal_second() {
        use std::time::Duration;
        assert_eq!(format_elapsed(Duration::from_millis(12_400)), "12.4s");
        assert_eq!(format_elapsed(Duration::from_millis(500)), "0.5s");
    }

    #[test]
    fn format_elapsed_over_a_minute_uses_m_and_zero_padded_s() {
        use std::time::Duration;
        assert_eq!(format_elapsed(Duration::from_secs(63)), "1m03s");
        assert_eq!(format_elapsed(Duration::from_secs(600)), "10m00s");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk cargo test -p pi format_elapsed`
Expected: FAIL — `cannot find function format_elapsed in this scope`.

- [ ] **Step 3: Implement the function**

Add above the `#[cfg(test)]` module in `crates/bin/src/duration.rs`. The
`#[allow(dead_code)]` is required because this bin crate flags `pub(crate)`
items `main` cannot yet reach (clippy runs `-D warnings`); the first live caller
lands in Task 4, which removes the allow:

```rust
/// Wall-clock elapsed time for the deploy stamp. Under a minute: one decimal
/// second (`12.4s`). A minute or more: `1m03s` (seconds zero-padded).
// Allow removed in Task 4 once `deploy_stamp` calls this from the live deploy path.
#[allow(dead_code)]
pub(crate) fn format_elapsed(d: std::time::Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let total = secs.round() as u64;
        format!("{}m{:02}s", total / 60, total % 60)
    }
}
```

- [ ] **Step 4: Run tests and the clippy gate to verify both pass**

Run: `rtk cargo test -p pi format_elapsed`
Expected: PASS (2 tests). Then
`rtk cargo clippy --all-targets --locked -- -D warnings`
Expected: no warnings (the `#[allow(dead_code)]` covers the not-yet-wired
helper).

- [ ] **Step 5: Commit**

```bash
rtk git add crates/bin/src/duration.rs
rtk git commit -m "feat(output): format_elapsed helper for the deploy stamp"
```

---

### Task 2: Truecolor capability + `Paint::table_color`

Tables are the one backend (`comfy-table`) that can emit 24-bit colour. Add a
capability probe and make table colours exact under truecolor, 256 otherwise.

**Files:**
- Modify: `crates/bin/src/output/theme.rs` (add `truecolor_enabled`,
  `truecolor_from`; replace `Paint::table` with `Paint::table_color`; update the
  `paint_converts_to_each_backend` test)
- Modify: `crates/bin/src/output/table.rs` (call `table_color(truecolor_enabled())`;
  update the `sem_colour_follows_the_theme` test)

**Interfaces:**
- Consumes: `pub fn rgb_to_ansi256(u8,u8,u8) -> u8` (already in `theme.rs`).
- Produces:
  - `pub fn truecolor_enabled() -> bool` — colours on **and** `COLORTERM` ∈
    {`truecolor`,`24bit`}.
  - `pub fn table_color(self, truecolor: bool) -> Option<comfy_table::Color>` on
    `Paint` — replaces `table(self)`. `Rgb` → `Color::Rgb{r,g,b}` when
    `truecolor`, else `Color::AnsiValue(rgb_to_ansi256(..))`; named → named;
    `Default` → `None`.

- [ ] **Step 1: Write the failing tests**

In `crates/bin/src/output/theme.rs`, replace the existing
`paint_converts_to_each_backend` test body's `table()` assertions and add a
truecolor test. Replace the two `.table()` assertion lines:

```rust
        assert!(Paint::Default.table().is_none());
        assert!(matches!(
            Paint::Rgb(197, 26, 74).table(),
            Some(comfy_table::Color::AnsiValue(161))
        ));
        assert!(matches!(
            Paint::Green.table(),
            Some(comfy_table::Color::Green)
        ));
```

with:

```rust
        assert!(Paint::Default.table_color(false).is_none());
        assert!(matches!(
            Paint::Rgb(197, 26, 74).table_color(false),
            Some(comfy_table::Color::AnsiValue(161))
        ));
        assert!(matches!(
            Paint::Rgb(197, 26, 74).table_color(true),
            Some(comfy_table::Color::Rgb { r: 197, g: 26, b: 74 })
        ));
        assert!(matches!(
            Paint::Green.table_color(false),
            Some(comfy_table::Color::Green)
        ));
```

Add a new test after `paint_converts_to_each_backend`:

```rust
    #[test]
    fn truecolor_needs_colours_on_and_a_truecolor_colorterm() {
        assert!(truecolor_from(Some("truecolor"), true));
        assert!(truecolor_from(Some("24bit"), true));
        assert!(!truecolor_from(Some("truecolor"), false)); // colours off
        assert!(!truecolor_from(Some("256color"), true));
        assert!(!truecolor_from(None, true));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk cargo test -p pi -- theme`
Expected: FAIL — `no method named table_color`, `cannot find function truecolor_from`.

- [ ] **Step 3: Implement**

In `crates/bin/src/output/theme.rs`, replace the whole `pub fn table(self)`
method (the one returning `Option<comfy_table::Color>`) with:

```rust
    /// Foreground colour for `comfy-table` cells; `None` = uncoloured.
    /// `Rgb` is exact under truecolor, else reduced to the nearest xterm-256.
    pub fn table_color(self, truecolor: bool) -> Option<comfy_table::Color> {
        use comfy_table::Color;
        match self {
            Paint::Default => None,
            Paint::Cyan => Some(Color::Cyan),
            Paint::Green => Some(Color::Green),
            Paint::Yellow => Some(Color::Yellow),
            Paint::Red => Some(Color::Red),
            Paint::Rgb(r, g, b) => Some(if truecolor {
                Color::Rgb { r, g, b }
            } else {
                Color::AnsiValue(rgb_to_ansi256(r, g, b))
            }),
        }
    }
```

Add these two functions after the `theme()` function (near the `ACTIVE`
`OnceLock`):

```rust
/// True when 24-bit colour should be emitted: colours are enabled and the
/// terminal advertises truecolor via `COLORTERM`. Pure core in `truecolor_from`.
pub fn truecolor_enabled() -> bool {
    truecolor_from(
        std::env::var("COLORTERM").ok().as_deref(),
        console::colors_enabled(),
    )
}

fn truecolor_from(colorterm: Option<&str>, colors_on: bool) -> bool {
    colors_on && matches!(colorterm, Some("truecolor") | Some("24bit"))
}
```

In `crates/bin/src/output/table.rs`, update the two call sites of `.table()`:

`header()` — change:

```rust
            match super::theme::theme().accent.table() {
```
to:
```rust
            match super::theme::theme()
                .accent
                .table_color(super::theme::truecolor_enabled())
            {
```

`sem_colour()` — change its body to thread the capability:

```rust
fn sem_colour(sem: Sem) -> Option<Color> {
    let t = super::theme::theme();
    let tc = super::theme::truecolor_enabled();
    match sem {
        Sem::Success => t.success.table_color(tc),
        Sem::Error => t.error.table_color(tc),
        Sem::Warn => t.warn.table_color(tc),
        Sem::Accent => t.accent.table_color(tc),
        Sem::Muted | Sem::Neutral | Sem::Frame => None,
    }
}
```

Update the `sem_colour_follows_the_theme` test in `table.rs` to compare against
`table_color` with the same capability:

```rust
    #[test]
    fn sem_colour_follows_the_theme() {
        let t = super::super::theme::theme();
        let tc = super::super::theme::truecolor_enabled();
        assert_eq!(sem_colour(Sem::Accent), t.accent.table_color(tc));
        assert_eq!(sem_colour(Sem::Success), t.success.table_color(tc));
        assert_eq!(sem_colour(Sem::Warn), t.warn.table_color(tc));
        assert_eq!(sem_colour(Sem::Error), t.error.table_color(tc));
        assert_eq!(sem_colour(Sem::Neutral), None);
        assert_eq!(sem_colour(Sem::Muted), None);
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test -p pi -- theme table`
Expected: PASS. Then `rtk cargo clippy --all-targets --locked -- -D warnings`
Expected: no warnings (the old `table` method has no remaining callers).

- [ ] **Step 5: Commit**

```bash
rtk git add crates/bin/src/output/theme.rs crates/bin/src/output/table.rs
rtk git commit -m "feat(output): truecolor capability probe + exact-Rgb table cells"
```

---

### Task 3: Banner module (triangle, gradient, banners, stamp)

Pure string builders. Colour and unicode are read from `console` at the edges
but the layout/gradient logic is deterministic and fully tested via `*_inner`
functions that take `unicode: bool` explicitly.

**Files:**
- Create: `crates/bin/src/output/banner.rs`
- Modify: `crates/bin/src/output/mod.rs` (add `mod banner;` and re-exports)

**Interfaces:**
- Consumes: `theme::truecolor_enabled()`, `theme::rgb_to_ansi256()` (Task 2 /
  existing).
- Produces (all `pub` in `banner`; re-exported from `output` only in the task
  that first consumes each, to avoid unused-import warnings):
  - `pub fn deploy_banner(project: &str) -> String`
  - `pub fn brand_banner(version: &str) -> String`
  - `pub enum StampOutcome { Success, Superseded, Failed }`
  - `pub fn deploy_stamp(outcome: StampOutcome, project: &str, url: Option<&str>, elapsed: std::time::Duration) -> String`
  - `pub fn stderr_is_tty() -> bool`

**Dead-code note:** this bin crate flags `pub` items that `main` cannot reach
(clippy runs `-D warnings`). The whole module is reachable only from tests until
Task 4, so `banner.rs` opens with a file-level `#![allow(dead_code)]`. Task 5
removes it once every entry point has a real caller. Re-exports and the
`show_deploy_banner` helper are added in Tasks 4–5 (where they get used), not
here.

- [ ] **Step 1: Write the failing tests (create the file with tests first)**

Create `crates/bin/src/output/banner.rs` with the test module only, so the
build fails on the missing items:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn triangle_rows_have_the_expected_gradient_glyphs_and_widths() {
        assert_eq!(triangle_row(0), "░░");
        assert_eq!(triangle_row(1), "▒▒▒▒");
        assert_eq!(triangle_row(2), "▓▓▓▓▓▓");
        assert_eq!(triangle_row(3), "▓▓▓▓");
        assert_eq!(triangle_row(4), "██");
    }

    #[test]
    fn row_rgb_sweeps_pink_top_to_raspberry_bottom() {
        assert_eq!(row_rgb(0), (240, 108, 160)); // pink
        assert_eq!(row_rgb(4), (197, 26, 74)); // raspberry
        // middle row is strictly between the endpoints on every channel
        let (r, g, b) = row_rgb(2);
        assert!((197..=240).contains(&r) && (26..=108).contains(&g) && (74..=160).contains(&b));
    }

    #[test]
    fn sgr_truecolor_wraps_text_in_a_24bit_escape() {
        assert_eq!(
            sgr_truecolor("X", (197, 26, 74)),
            "\u{1b}[38;2;197;26;74mX\u{1b}[39m"
        );
    }

    #[test]
    fn unicode_deploy_banner_has_triangle_and_wordmark_no_ansi_when_colours_off() {
        // Colours are off under captured test output, so paint_row is a no-op.
        let s = render_banner_inner(true, "r p i", "deploy · myboard", None);
        assert!(s.contains("▒▒▒▒"), "{s:?}");
        assert!(s.contains("▓▓▓▓▓▓"), "{s:?}");
        assert!(s.contains("r p i"), "{s:?}");
        assert!(s.contains("deploy · myboard"), "{s:?}");
        assert!(!s.contains('\u{1b}'), "{s:?}");
    }

    #[test]
    fn ascii_banner_falls_back_to_a_plain_wordmark() {
        let s = render_banner_inner(false, "r p i v1.2.3", "tagline", Some("rpi.iiskelo.com"));
        assert!(s.contains("rpi"), "{s:?}");
        assert!(s.contains("tagline"), "{s:?}");
        assert!(!s.contains('░') && !s.contains('▓'), "{s:?}");
        assert!(!s.contains('\u{1b}'), "{s:?}");
    }

    #[test]
    fn stamp_success_carries_glyph_project_url_and_elapsed() {
        let uni = deploy_stamp_inner(true, StampOutcome::Success, "myboard", Some("rpi.iiskelo.com"), Duration::from_millis(12_400));
        assert!(uni.contains("✓"), "{uni:?}");
        assert!(uni.contains("myboard"), "{uni:?}");
        assert!(uni.contains("rpi.iiskelo.com"), "{uni:?}");
        assert!(uni.contains("12.4s"), "{uni:?}");

        let ascii = deploy_stamp_inner(false, StampOutcome::Success, "myboard", None, Duration::from_secs(1));
        assert!(ascii.contains("ok"), "{ascii:?}");
        assert!(!ascii.contains('✓'), "{ascii:?}");
        assert!(!ascii.contains('→'), "no arrow when url is absent: {ascii:?}");
    }

    #[test]
    fn stamp_failed_uses_the_cross_glyph_and_superseded_is_neutral_text() {
        let failed = deploy_stamp_inner(true, StampOutcome::Failed, "api", None, Duration::from_secs(2));
        assert!(failed.contains("✗") && failed.contains("failed"), "{failed:?}");
        let sup = deploy_stamp_inner(true, StampOutcome::Superseded, "api", None, Duration::from_secs(2));
        assert!(sup.to_lowercase().contains("superseded"), "{sup:?}");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk cargo test -p pi -- banner`
Expected: FAIL to compile — the module isn't declared and the items don't exist.

- [ ] **Step 3: Implement the module**

Prepend the implementation above the test module in
`crates/bin/src/output/banner.rs`. The first line is the file-level allow that
keeps the not-yet-wired builders from tripping `-D warnings` (removed in Task 5):

```rust
// Removed in Task 5 once every entry point has a real caller.
#![allow(dead_code)]

use std::time::Duration;

use super::theme;

/// Top row colour (pink `#F06CA0`) of the logo gradient.
const PINK: (u8, u8, u8) = (240, 108, 160);
/// Bottom row colour (raspberry `#C51A4A`) of the logo gradient.
const RASPBERRY: (u8, u8, u8) = (197, 26, 74);

/// The five right-pointing triangle rows, top→bottom, each filled with its
/// density-ramp glyph. Widths 2,4,6,4,2 make the point; the ramp
/// `░ ▒ ▓ ▓ █` darkens downward.
fn triangle_row(i: usize) -> String {
    const RAMP: [char; 5] = ['░', '▒', '▓', '▓', '█'];
    const WIDTH: [usize; 5] = [2, 4, 6, 4, 2];
    RAMP[i].to_string().repeat(WIDTH[i])
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

/// Per-row gradient colour, pink (row 0) → raspberry (row 4).
fn row_rgb(i: usize) -> (u8, u8, u8) {
    let t = i as f32 / 4.0;
    (
        lerp(PINK.0, RASPBERRY.0, t),
        lerp(PINK.1, RASPBERRY.1, t),
        lerp(PINK.2, RASPBERRY.2, t),
    )
}

/// Raw 24-bit SGR wrap. Used only when truecolor is active.
fn sgr_truecolor(text: &str, (r, g, b): (u8, u8, u8)) -> String {
    format!("\u{1b}[38;2;{r};{g};{b}m{text}\u{1b}[39m")
}

/// Colour a triangle row for the current terminal: truecolor escape when
/// available, else the nearest xterm-256 via `console`; plain when colours off.
fn paint_row(text: &str, rgb: (u8, u8, u8)) -> String {
    if !console::colors_enabled() {
        return text.to_string();
    }
    if theme::truecolor_enabled() {
        sgr_truecolor(text, rgb)
    } else {
        let idx = theme::rgb_to_ansi256(rgb.0, rgb.1, rgb.2);
        console::Style::new().color256(idx).apply_to(text).to_string()
    }
}

/// Wordmark styling per row: bold on the name row, dim on the URL row.
fn style_word(row: usize, word: &str) -> String {
    match row {
        1 => console::Style::new().bold().apply_to(word).to_string(),
        3 => console::Style::new().dim().apply_to(word).to_string(),
        _ => word.to_string(),
    }
}

/// Does this terminal render the block/emoji glyphs? Mirrors how the `▸`
/// marker degrades — non-unicode (and non-TTY) terminals get the plain form.
fn wants_unicode() -> bool {
    console::Term::stderr().features().wants_emoji()
}

pub fn stderr_is_tty() -> bool {
    console::Term::stderr().is_term()
}

/// Assemble the banner. `unicode=false` degrades to a one-line wordmark.
/// `line1` sits beside triangle row 1, `line2` row 2, `line3` (optional) row 3.
fn render_banner_inner(unicode: bool, line1: &str, line2: &str, line3: Option<&str>) -> String {
    if !unicode {
        return match line3 {
            Some(l3) => format!("rpi — {line2}  ({l3})"),
            None => format!("rpi — {line2}"),
        };
    }
    let words = ["", line1, line2, line3.unwrap_or(""), ""];
    (0..5)
        .map(|i| {
            let tri = paint_row(&format!("{:<6}", triangle_row(i)), row_rgb(i));
            if words[i].is_empty() {
                tri
            } else {
                format!("{tri}  {}", style_word(i, words[i]))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Deploy header: triangle + `r p i` / `deploy · <project>`.
pub fn deploy_banner(project: &str) -> String {
    render_banner_inner(wants_unicode(), "r p i", &format!("deploy · {project}"), None)
}

/// Brand banner for bare `rpi` / `--version`: triangle + version, tagline, URL.
pub fn brand_banner(version: &str) -> String {
    render_banner_inner(
        wants_unicode(),
        &format!("r p i v{version}"),
        "deploy anything to your Pi",
        Some("rpi.iiskelo.com"),
    )
}

pub enum StampOutcome {
    Success,
    Superseded,
    Failed,
}

/// Inner stamp builder with the unicode decision injected for testing. Returns
/// the summary text only (no marker/colour); callers pass it to the pane's
/// `finish_*` which add the `▸` marker and semantic colour.
fn deploy_stamp_inner(
    unicode: bool,
    outcome: StampOutcome,
    project: &str,
    url: Option<&str>,
    elapsed: Duration,
) -> String {
    let elapsed = crate::duration::format_elapsed(elapsed);
    match outcome {
        StampOutcome::Success => {
            let check = if unicode { "✓" } else { "ok" };
            let arrow = if unicode { "→" } else { "->" };
            let dest = url
                .map(|u| format!("  {arrow}  {u}"))
                .unwrap_or_default();
            format!("deployed {check} {project}{dest} ({elapsed})")
        }
        StampOutcome::Superseded => {
            format!("deploy superseded — {project} (a newer deploy replaced this one) ({elapsed})")
        }
        StampOutcome::Failed => {
            let cross = if unicode { "✗" } else { "x" };
            format!("deploy failed {cross} {project} — see log above ({elapsed})")
        }
    }
}

pub fn deploy_stamp(
    outcome: StampOutcome,
    project: &str,
    url: Option<&str>,
    elapsed: Duration,
) -> String {
    deploy_stamp_inner(wants_unicode(), outcome, project, url, elapsed)
}
```

In `crates/bin/src/output/mod.rs`, add ONLY the module declaration after the
existing `mod theme;` line (no re-exports and no `show_deploy_banner` yet —
those land in Tasks 4–5 where they are consumed, so nothing is an unused
import):

```rust
mod banner;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test -p pi -- banner`
Expected: PASS (7 banner tests). Then
`rtk cargo clippy --all-targets --locked -- -D warnings`
Expected: no warnings — the file-level `#![allow(dead_code)]` covers the
builders until Tasks 4–5 wire them.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/bin/src/output/banner.rs crates/bin/src/output/mod.rs
rtk git commit -m "feat(output): banner module — triangle logo, gradient, deploy stamp"
```

---

### Task 4: Wire the banner + stamp into `rpi deploy`

**Files:**
- Modify: `crates/bin/src/output/mod.rs` (add `show_deploy_banner` + two
  re-exports)
- Modify: `crates/bin/src/duration.rs` (remove `format_elapsed`'s
  `#[allow(dead_code)]` — the deploy path now reaches it)
- Modify: `crates/bin/src/cli/commands.rs` (`deploy`, lines 15–74)

**Interfaces:**
- Consumes: `banner::deploy_banner`, `banner::stderr_is_tty`,
  `banner::deploy_stamp`, `banner::StampOutcome` (Task 3);
  `rpitoml.ingress.hostname: Option<String>`, `rpitoml.project.name: String`.
- Produces: `output::show_deploy_banner`, `output::deploy_stamp`,
  `output::StampOutcome` (re-exports).

- [ ] **Step 1: Add the print helper and re-exports in `mod.rs`**

In `crates/bin/src/output/mod.rs`, add the two re-exports right after the
`mod banner;` line from Task 3:

```rust
pub use banner::{deploy_stamp, StampOutcome};
```

And add this helper near the other `pub fn` message helpers (e.g. after
`status`):

```rust
/// Print the deploy logo banner to stderr, but only on an interactive
/// terminal — under a pipe, file, or CI it is skipped so logs stay clean.
pub fn show_deploy_banner(project: &str) {
    if banner::stderr_is_tty() {
        eprintln!("{}", banner::deploy_banner(project));
    }
}
```

Then, in `crates/bin/src/duration.rs`, delete the two lines above
`pub(crate) fn format_elapsed` (the `// Allow removed in Task 4 …` comment and
the `#[allow(dead_code)]`) — the live deploy path now reaches it.

- [ ] **Step 2: Add the banner call and the timing start**

In `crates/bin/src/cli/commands.rs`, in `deploy`, insert the banner print right
after the project config is built (after line 17 `let project = ...`):

```rust
    output::show_deploy_banner(&rpitoml.project.name);
```

Immediately before `let accepted = api.deploy(&req).await?;` add:

```rust
    let started = std::time::Instant::now();
```

- [ ] **Step 3: Replace the finish summaries with the stamp**

Replace the whole `match status.as_str() { ... }` block (current lines 56–69)
with:

```rust
    let elapsed = started.elapsed();
    let name = &rpitoml.project.name;
    let url = rpitoml.ingress.hostname.as_deref();
    match status.as_str() {
        "success" => pane.finish_ok(&output::deploy_stamp(
            output::StampOutcome::Success,
            name,
            url,
            elapsed,
        )),
        "superseded" => pane.finish_neutral(&output::deploy_stamp(
            output::StampOutcome::Superseded,
            name,
            url,
            elapsed,
        )),
        _ => {
            pane.finish_err(&output::deploy_stamp(
                output::StampOutcome::Failed,
                name,
                url,
                elapsed,
            ));
            for w in &warnings {
                output::warn(w);
            }
            drop(tunnel);
            std::process::exit(1);
        }
    }
```

(The trailing `for w in &warnings { output::warn(w); }` and `Ok(())` after the
match stay unchanged.)

- [ ] **Step 4: Build and verify it compiles**

Run: `rtk cargo build -p pi`
Expected: builds clean. Then
`rtk cargo clippy --all-targets --locked -- -D warnings`
Expected: no warnings (`brand_banner` is still unused but covered by the
file-level `#![allow(dead_code)]` in `banner.rs` until Task 5).

- [ ] **Step 5: Verify the deploy path still tests green**

Run: `rtk cargo test -p pi`
Expected: PASS (existing deploy/command tests unaffected — the stamp is
constructed from a `Duration` and pure inputs; no test drives a live deploy).

- [ ] **Step 6: Manual smoke check of the stamp string (no server needed)**

The stamp is pure. Its success/failed/superseded shapes are already covered by
the banner tests. No live deploy is required for this task.

Run: `rtk cargo test -p pi -- banner`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
rtk git add crates/bin/src/output/mod.rs crates/bin/src/duration.rs crates/bin/src/cli/commands.rs
rtk git commit -m "feat(cli): logo banner + result stamp on rpi deploy"
```

---

### Task 5: Logo on bare `rpi` and `--version`

Take over clap's version flag and make the subcommand optional so both paths
can print the brand banner.

**Files:**
- Modify: `crates/bin/src/output/mod.rs` (re-export `brand_banner`,
  `stderr_is_tty`)
- Modify: `crates/bin/src/output/banner.rs` (remove the file-level
  `#![allow(dead_code)]` — every entry point now has a caller)
- Modify: `crates/bin/src/main.rs` (`Cli` struct, `run()`, and the parse tests)

**Interfaces:**
- Consumes: `banner::brand_banner`, `banner::stderr_is_tty` (Task 3).
- Produces: `output::brand_banner`, `output::stderr_is_tty` (re-exports);
  `Cli { version: bool, cmd: Option<Cmd> }`; `run()` handles the `version` flag
  and the `cmd == None` case before dispatching.

- [ ] **Step 1: Write the failing parse tests**

Add to the `tests` module in `crates/bin/src/main.rs`:

```rust
    #[test]
    fn bare_rpi_parses_with_no_subcommand() {
        let cli = Cli::try_parse_from(["rpi"]).unwrap();
        assert!(cli.cmd.is_none());
        assert!(!cli.version);
    }

    #[test]
    fn version_flag_parses() {
        let cli = Cli::try_parse_from(["rpi", "--version"]).unwrap();
        assert!(cli.version);
        let short = Cli::try_parse_from(["rpi", "-V"]).unwrap();
        assert!(short.version);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk cargo test -p pi -- bare_rpi_parses version_flag_parses`
Expected: FAIL — `no field version on type Cli`, and `cli.cmd.is_none()` won't
compile (`cmd` is not `Option`).

- [ ] **Step 3: Change the `Cli` struct**

Replace the `Cli` struct and its `#[command(...)]` attribute (lines 11–20) with:

```rust
#[derive(Parser)]
#[command(
    name = "rpi",
    about = "deploy tool for Raspberry Pi (CLI + agent)",
    disable_version_flag = true
)]
struct Cli {
    /// Print version (with the brand banner on a terminal)
    #[arg(short = 'V', long = "version")]
    version: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}
```

- [ ] **Step 4: Re-export the banner helpers and drop the dead-code allow**

In `crates/bin/src/output/mod.rs`, extend the banner re-exports (the line added
in Task 4) so `main.rs` can reach them:

```rust
pub use banner::{brand_banner, deploy_stamp, stderr_is_tty, StampOutcome};
```

In `crates/bin/src/output/banner.rs`, delete the first two lines (the comment
and `#![allow(dead_code)]`) — every builder now has a real caller, so the allow
is no longer needed and clippy would flag it as an unnecessary `allow`.

- [ ] **Step 5: Handle the two new paths at the top of `run()`**

In `run()`, replace the opening (from `let cli = Cli::parse();` down to the end
of the agent-run early-return block) with:

```rust
async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.version {
        let v = env!("CARGO_PKG_VERSION");
        if output::stderr_is_tty() {
            println!("{}", output::brand_banner(v));
        } else {
            println!("rpi {v}");
        }
        return Ok(());
    }

    let cmd = match cli.cmd {
        Some(cmd) => cmd,
        None => {
            if output::stderr_is_tty() {
                eprintln!("{}", output::brand_banner(env!("CARGO_PKG_VERSION")));
            }
            eprintln!("run `rpi --help` to see available commands");
            return Ok(());
        }
    };

    if matches!(
        cmd,
        Cmd::Agent {
            cmd: AgentCmd::Run { .. }
        }
    ) {
        return match cmd {
            Cmd::Agent {
                cmd: AgentCmd::Run { config },
            } => agent::run::run(config).await,
            _ => unreachable!(),
        };
    }
```

Then change the two later references from `cli.cmd` to `cmd`:
- `tracing_subscriber` init stays as-is.
- The big dispatch `match cli.cmd {` becomes `match cmd {`.

- [ ] **Step 6: Update the existing parse tests to unwrap the optional subcommand**

Every existing test that does `match cli.cmd { Cmd::... }` must now unwrap the
`Option`. In `crates/bin/src/main.rs` tests, change each occurrence of
`match cli.cmd {` to `match cli.cmd.unwrap() {`. The affected tests are:
`deploy_ci_flags_parse`, `agent_logs_flags_parse`, `init_flags_parse`,
`setup_flags_parse`, `agent_setup_flags_parse`, `parses_agent_migrate`,
`parses_agent_setup_cloudflare_flags`, `agent_uninstall_flags_parse`,
`secrets_commands_parse_and_env_is_gone`, `command_parses_name_and_trailing_args`,
`bare_command_means_list_mode`.

The `is_err()` tests (`deploy_host_requires_user`, `server_flag_conflicts_with_host`)
are unchanged.

Example — `deploy_ci_flags_parse` changes:

```rust
        match cli.cmd.unwrap() {
            Cmd::Deploy { connect, .. } => {
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `rtk cargo test -p pi`
Expected: PASS (all parse tests, including the two new ones).

- [ ] **Step 8: Verify clippy and formatting**

Run:
```
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
```
Expected: no diff, no warnings.

- [ ] **Step 9: Manual smoke check**

Run: `rtk cargo run -p pi --bin rpi -- --version`
Expected on a terminal: the raspberry triangle banner with `r p i v0.14.0`,
tagline, and `rpi.iiskelo.com`. Piped (`... -- --version | cat`): plain
`rpi 0.14.0`. Run `rtk cargo run -p pi --bin rpi` (no args): banner (on a TTY)
plus `run \`rpi --help\` to see available commands`.

- [ ] **Step 10: Commit**

```bash
rtk git add crates/bin/src/output/mod.rs crates/bin/src/output/banner.rs crates/bin/src/main.rs
rtk git commit -m "feat(cli): brand banner on bare rpi and --version"
```

---

### Task 6: Final verification + README note

**Files:**
- Modify: `README.md` (document the banner + `--version` behaviour under the
  existing "Console theme" section)

- [ ] **Step 1: Full workspace gate**

Run:
```
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
```
Expected: clean format, no warnings, all tests pass.

- [ ] **Step 2: Visual confirmation in a real terminal**

Run each and eyeball the output:
- `rtk cargo run -p pi --bin rpi -- --version` — colourful triangle + gradient.
- `PI_THEME=classic rtk cargo run -p pi --bin rpi -- --version` — banner is
  still raspberry (the logo is brand-fixed; only messages/tables follow the
  theme).
- `NO_COLOR=1 rtk cargo run -p pi --bin rpi -- --version` — triangle glyphs, no
  colour.
- `rtk cargo run -p pi --bin rpi -- --version | cat` — plain `rpi <version>`.

- [ ] **Step 3: Document in README**

In `README.md`, under the "### Console theme" section, append:

```markdown
`rpi deploy`, bare `rpi`, and `rpi --version` show a raspberry triangle logo
with a vertical gradient. The banner appears only on an interactive terminal —
piped or CI output stays plain, and `rpi --version | cat` prints just
`rpi <version>`. On truecolor terminals (`COLORTERM=truecolor`) the logo and
table colours render as the exact brand `#C51A4A`; elsewhere they use the
nearest 256-colour. The logo is always raspberry, independent of `PI_THEME`.
```

- [ ] **Step 4: Commit**

```bash
rtk git add README.md
rtk git commit -m "docs: document the CLI logo banner and truecolor behaviour"
```

---

## Self-Review

**1. Spec coverage:**
- Deploy triangle logo with vertical char gradient → Tasks 3 (builder) + 4 (wiring). ✓
- Gradient = vertical density ramp `░▒▓▓█` + truecolor pink→raspberry sweep → Task 3 (`triangle_row`, `row_rgb`, `paint_row`). ✓
- Logo on bare `rpi` / `--version` → Task 5. ✓
- Deploy result stamp (status, project, URL, elapsed; no service count) → Tasks 1 + 3 + 4. ✓
- Truecolor layered: tables `Color::Rgb` (Task 2), banner manual SGR (Task 3), messages/marker/spinner stay 256 (untouched). ✓ Stamp intentionally on the 256 message path — noted in "Refinements". ✓
- TTY-gated banner, `NO_COLOR`/non-unicode fallbacks → Task 3 (`stderr_is_tty`, `wants_unicode`, `paint_row`), Task 6 verification. ✓
- Banner never corrupts stdout (banner→stderr; `--version` plain form for non-TTY→stdout) → Tasks 3–5. ✓
- Non-goals (phases, service count, spinner truecolor, extra flags) → not implemented. ✓

**2. Placeholder scan:** No TBD/TODO; every code step shows complete code; every test step shows the assertions. ✓

**3. Type consistency:** `truecolor_enabled()`/`truecolor_from()` (Task 2) used by `paint_row` and `table_color` (Tasks 2–3). `StampOutcome` variants `Success`/`Superseded`/`Failed` consistent across Tasks 3–4. `deploy_stamp(outcome, project, url: Option<&str>, elapsed: Duration)` signature identical in Tasks 3 and 4. `render_banner_inner(unicode, line1, line2, line3)` and `deploy_stamp_inner(unicode, ...)` are the tested cores of the public `deploy_banner`/`brand_banner`/`deploy_stamp`. `Cli { version: bool, cmd: Option<Cmd> }` consumed consistently in Task 5. ✓
