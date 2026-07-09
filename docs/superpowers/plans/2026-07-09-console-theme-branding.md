# Console Theme Layer + rpi Brand Styling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One theme object drives every colour and glyph in `rpi`'s console output; the default theme is the rpi-deploy-site brand (raspberry `▸` marker on every line, site green/amber), switchable via `PI_THEME`.

**Architecture:** New `crates/bin/src/output/theme.rs` holds `Paint` (colour value → console/comfy-table/indicatif converters, RGB reduced to xterm-256) and `Theme` (palette + marker glyphs + marker policy), selected once from `PI_THEME` via `OnceLock`. Existing `console_style(Sem)` and the marker become theme-driven, so `mod.rs` messages, the log pane, tables, and the spinner all follow automatically. New `info()`/`status()` helpers brand the raw informational `println!`/`eprintln!` lines in the CLI/agent commands.

**Tech Stack:** Rust workspace; `console` 0.15 (no truecolor — 256-colour only), `comfy-table` 7.2 (`Color::AnsiValue`), `indicatif` 0.17 (template colours parse via `console::Style::from_dotted_str`, which accepts numeric `0-255` tokens — verified in registry source).

**Spec:** `docs/superpowers/specs/2026-07-09-console-theme-branding-design.md`

## Global Constraints

- Brand palette: accent `#C51A4A` → xterm 161, success `#75A928` → 106, warn `#d4a017` → 178; error stays terminal red.
- Marker glyphs: raspberry theme `("▸", ">")`, classic theme `("●", "*")` — unicode + ASCII fallback via `console::Emoji`.
- `PI_THEME` env var: `raspberry` (default), `classic`; unknown values silently fall back to raspberry.
- A theme controls only palette and glyphs; structural changes (marker prefixes on `heading`/`note`, `info()`/`status()`) apply under every theme.
- Must NOT be branded: `--json` output, SSE/log streams, table bodies, `agent migrate --list` TSV, the interactive `eprint!` confirmation prompt in `rm`, indented continuation lines under headings (`secrets_ls` key/file lists, `command` list lines).
- Colour auto-disable semantics unchanged: everything flows through `console`/`comfy-table`/`indicatif` TTY gating + the existing `NO_COLOR` handling in `init_colors()`/`table()`.
- Each line keeps its current stream (stdout lines → `info()`, stderr lines → `status()`); no stream migrations.
- Gate before every commit (workspace `CLAUDE.md`): `rtk cargo fmt --all -- --check`, `rtk cargo clippy --all-targets --locked -- -D warnings`, `rtk cargo test --locked`. If fmt reports a diff, run `rtk cargo fmt --all` — never hand-edit formatting.

---

### Task 1: Theme layer (`theme.rs`)

**Files:**
- Create: `crates/bin/src/output/theme.rs`
- Modify: `crates/bin/src/output/mod.rs` (add `mod theme;` only — rewiring is Task 2)
- Test: inline `#[cfg(test)]` in `theme.rs`

**Interfaces:**
- Consumes: nothing (leaf module).
- Produces (used by Tasks 2–3):
  - `pub enum Paint { Default, Cyan, Green, Yellow, Red, Rgb(u8, u8, u8) }` with `pub fn console(self) -> console::Style`, `pub fn table(self) -> Option<comfy_table::Color>`, `pub fn template_token(self) -> Option<String>`
  - `pub struct Theme { pub accent: Paint, pub success: Paint, pub warn: Paint, pub error: Paint, pub marker: (&'static str, &'static str), pub marker_accent: bool }` with `Theme::raspberry()`, `Theme::classic()`, `Theme::from_env_value(Option<&str>) -> Theme`
  - `pub fn theme() -> &'static Theme` (process-wide, `OnceLock`, reads `PI_THEME` on first use)
  - `pub fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8`

- [ ] **Step 1: Write the failing tests**

Create `crates/bin/src/output/theme.rs` containing ONLY the test module for now (so the failure is "missing items", not "missing file" — simpler to keep one file):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_colours_map_to_expected_ansi256() {
        assert_eq!(rgb_to_ansi256(197, 26, 74), 161); // raspberry #C51A4A
        assert_eq!(rgb_to_ansi256(117, 169, 40), 106); // green #75A928
        assert_eq!(rgb_to_ansi256(212, 160, 23), 178); // amber #d4a017
    }

    #[test]
    fn greys_use_the_grey_ramp_and_extremes_use_the_cube() {
        assert_eq!(rgb_to_ansi256(128, 128, 128), 244); // mid grey -> ramp
        assert_eq!(rgb_to_ansi256(0, 0, 0), 16); // exact cube black
        assert_eq!(rgb_to_ansi256(255, 255, 255), 231); // exact cube white
    }

    #[test]
    fn env_value_selects_theme_and_unknown_falls_back() {
        assert!(Theme::from_env_value(None).marker_accent, "default = raspberry");
        assert_eq!(Theme::from_env_value(Some("classic")).accent, Paint::Cyan);
        assert_eq!(Theme::from_env_value(Some("classic")).marker, ("●", "*"));
        assert!(
            Theme::from_env_value(Some("purple")).marker_accent,
            "unknown value falls back to raspberry"
        );
        assert_eq!(Theme::from_env_value(None).marker, ("▸", ">"));
    }

    #[test]
    fn paint_converts_to_each_backend() {
        // A style with no attributes never emits ANSI, colours on or off.
        assert_eq!(Paint::Default.console().apply_to("x").to_string(), "x");
        assert_eq!(Paint::Default.template_token(), None);
        assert_eq!(Paint::Cyan.template_token().as_deref(), Some("cyan"));
        assert_eq!(
            Paint::Rgb(197, 26, 74).template_token().as_deref(),
            Some("161")
        );
        assert!(Paint::Default.table().is_none());
        assert!(matches!(
            Paint::Rgb(197, 26, 74).table(),
            Some(comfy_table::Color::AnsiValue(161))
        ));
        assert!(matches!(Paint::Green.table(), Some(comfy_table::Color::Green)));
    }
}
```

Register the module in `crates/bin/src/output/mod.rs` — after the existing `mod logpane;` block add:

```rust
mod theme;
```

(Private `mod` is enough: sibling submodules `spinner.rs`/`table.rs` are descendants of `output` and can use `super::theme::…`.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk cargo test --locked -p pi output::theme`
Expected: COMPILE ERROR — `rgb_to_ansi256`, `Theme`, `Paint` not found.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/bin/src/output/theme.rs` (above the test module):

```rust
use std::sync::OnceLock;

use console::Style;

/// How a semantic role is painted. `Rgb` is reduced to the nearest xterm-256
/// index so `console` (which has no truecolor support) and `comfy-table`
/// render the identical colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paint {
    Default,
    Cyan,
    Green,
    Yellow,
    Red,
    Rgb(u8, u8, u8),
}

impl Paint {
    /// Base `console` style (no modifiers) for this paint.
    pub fn console(self) -> Style {
        let s = Style::new();
        match self {
            Paint::Default => s,
            Paint::Cyan => s.cyan(),
            Paint::Green => s.green(),
            Paint::Yellow => s.yellow(),
            Paint::Red => s.red(),
            Paint::Rgb(r, g, b) => s.color256(rgb_to_ansi256(r, g, b)),
        }
    }

    /// Foreground colour for `comfy-table` cells; `None` = uncoloured.
    pub fn table(self) -> Option<comfy_table::Color> {
        use comfy_table::Color;
        match self {
            Paint::Default => None,
            Paint::Cyan => Some(Color::Cyan),
            Paint::Green => Some(Color::Green),
            Paint::Yellow => Some(Color::Yellow),
            Paint::Red => Some(Color::Red),
            Paint::Rgb(r, g, b) => Some(Color::AnsiValue(rgb_to_ansi256(r, g, b))),
        }
    }

    /// Colour token for an `indicatif` template (`{spinner:.<token>}`).
    /// `indicatif` parses it with `console::Style::from_dotted_str`, which
    /// accepts ANSI names and numeric `0-255` tokens. `None` = no colour.
    pub fn template_token(self) -> Option<String> {
        match self {
            Paint::Default => None,
            Paint::Cyan => Some("cyan".into()),
            Paint::Green => Some("green".into()),
            Paint::Yellow => Some("yellow".into()),
            Paint::Red => Some("red".into()),
            Paint::Rgb(r, g, b) => Some(rgb_to_ansi256(r, g, b).to_string()),
        }
    }
}

/// Nearest xterm-256 index for an RGB colour: the best of the 6x6x6 cube
/// (16..232) and the grey ramp (232..256) by squared RGB distance.
pub fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [i32; 6] = [0, 95, 135, 175, 215, 255];
    fn nearest_level(v: u8) -> usize {
        let mut best = 0;
        for (i, l) in LEVELS.iter().enumerate() {
            if (v as i32 - l).abs() < (v as i32 - LEVELS[best]).abs() {
                best = i;
            }
        }
        best
    }
    fn dist(a: (i32, i32, i32), b: (i32, i32, i32)) -> i32 {
        (a.0 - b.0).pow(2) + (a.1 - b.1).pow(2) + (a.2 - b.2).pow(2)
    }
    let want = (r as i32, g as i32, b as i32);
    let (ri, gi, bi) = (nearest_level(r), nearest_level(g), nearest_level(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let cube_idx = (16 + 36 * ri + 6 * gi + bi) as u8;
    // Grey ramp entry i (0..24) has value 8 + 10*i.
    let grey_i = ((want.0 + want.1 + want.2) / 3 - 8).clamp(0, 230) / 10;
    let grey_v = 8 + 10 * grey_i;
    if dist(want, (grey_v, grey_v, grey_v)) < dist(want, cube) {
        (232 + grey_i) as u8
    } else {
        cube_idx
    }
}

/// A theme controls palette and glyphs only — structural output decisions
/// (which lines get a marker, streams, prefixes) are theme-independent.
pub struct Theme {
    pub accent: Paint,
    pub success: Paint,
    pub warn: Paint,
    pub error: Paint,
    /// Marker glyph: (unicode, ascii fallback), rendered via `console::Emoji`.
    pub marker: (&'static str, &'static str),
    /// true: the marker is always painted accent (the brand mark);
    /// false: the marker follows the line's own semantic colour.
    pub marker_accent: bool,
}

impl Theme {
    /// Brand theme from rpi-deploy-site: raspberry accent + triangle logo
    /// marker, site green/amber for success/warn.
    pub fn raspberry() -> Self {
        Theme {
            accent: Paint::Rgb(197, 26, 74),   // #C51A4A -> 161
            success: Paint::Rgb(117, 169, 40), // #75A928 -> 106
            warn: Paint::Rgb(212, 160, 23),    // #d4a017 -> 178
            error: Paint::Red,
            marker: ("▸", ">"),
            marker_accent: true,
        }
    }

    /// The pre-brand look: cyan accent, named ANSI colours, dot marker.
    pub fn classic() -> Self {
        Theme {
            accent: Paint::Cyan,
            success: Paint::Green,
            warn: Paint::Yellow,
            error: Paint::Red,
            marker: ("●", "*"),
            marker_accent: false,
        }
    }

    /// `PI_THEME` value -> theme. Unknown values silently fall back to the
    /// default so scripts never break on a typo.
    pub fn from_env_value(value: Option<&str>) -> Self {
        match value {
            Some("classic") => Self::classic(),
            _ => Self::raspberry(),
        }
    }
}

static ACTIVE: OnceLock<Theme> = OnceLock::new();

/// The process-wide theme, chosen once from `PI_THEME` on first use.
pub fn theme() -> &'static Theme {
    ACTIVE.get_or_init(|| Theme::from_env_value(std::env::var("PI_THEME").ok().as_deref()))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test --locked -p pi output::theme`
Expected: 4 tests PASS.

- [ ] **Step 5: Gate + commit**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
rtk git add crates/bin/src/output/theme.rs crates/bin/src/output/mod.rs
rtk git commit -m "feat(output): theme layer with Paint/Theme and PI_THEME selection"
```

Note: nothing outside `theme.rs` calls these items until Tasks 2–3, and `pub` items inside a private module count as dead code to rustc. If the gate trips on `dead_code`, put a temporary `#[allow(dead_code)]` on the flagged item and remove it in the task that adds its first caller (Task 2 for `console()`/`Theme`/`theme()`, Task 3 for `table()`/`template_token()`).

---

### Task 2: Theme-driven messages (`mod.rs`)

**Files:**
- Modify: `crates/bin/src/output/mod.rs`
- Test: inline `#[cfg(test)]` in `mod.rs`

**Interfaces:**
- Consumes (Task 1): `theme::theme()`, `Paint::console()`, `Theme { marker, marker_accent, … }`.
- Produces (used by Task 4):
  - `pub fn info(msg: impl std::fmt::Display)` — stdout answer-content line: accent bold marker + untinted text.
  - `pub fn status(msg: impl std::fmt::Display)` — stderr progress line: accent bold marker + untinted text.
  - `success`/`error`/`warn`/`note`/`heading` keep their signatures; rendering changes only.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/bin/src/output/mod.rs`:

```rust
    #[test]
    fn info_and_status_lines_carry_text_and_no_ansi_when_disabled() {
        // Captured test output is not a TTY, so console styling is off.
        let line = info_line("hello world");
        assert!(line.contains("hello world"), "{line:?}");
        assert!(!line.contains('\u{1b}'), "{line:?}");
        let line = status_line("working...");
        assert!(line.contains("working..."), "{line:?}");
        assert!(!line.contains('\u{1b}'), "{line:?}");
    }

    #[test]
    fn marker_renders_one_of_the_active_theme_glyphs() {
        let t = theme::theme();
        let s = marker().to_string();
        assert!(s == t.marker.0 || s == t.marker.1, "{s:?}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk cargo test --locked -p pi output::tests`
Expected: COMPILE ERROR — `info_line`, `status_line`, `marker` not found.

- [ ] **Step 3: Rewire `mod.rs` to the theme**

In `crates/bin/src/output/mod.rs`:

**(a)** Delete the `MARKER` const:

```rust
/// Marker glyph for semantic messages; degrades to `*` on terminals without
/// unicode/emoji support.
const MARKER: Emoji<'_, '_> = Emoji("●", "*");
```

and replace with:

```rust
/// Marker glyph for message lines. The glyph pair comes from the active
/// theme; `console::Emoji` degrades to the ASCII form on terminals without
/// unicode support.
fn marker() -> Emoji<'static, 'static> {
    let t = theme::theme();
    Emoji(t.marker.0, t.marker.1)
}
```

**(b)** Replace `console_style` with the theme-reading version (Muted/Neutral/Frame stay modifier-based, not palette slots):

```rust
/// Role -> terminal style for `console`-rendered output (messages, pane).
pub(crate) fn console_style(sem: Sem) -> Style {
    let t = theme::theme();
    let s = Style::new();
    match sem {
        Sem::Success => t.success.console(),
        Sem::Error => t.error.console(),
        Sem::Warn => t.warn.console(),
        Sem::Accent => t.accent.console(),
        Sem::Muted => s.dim(),
        Sem::Neutral => s,
        Sem::Frame => s.black().bright(),
    }
}
```

**(c)** Replace `stderr_line` — marker paint honours `marker_accent`:

```rust
/// One stderr status line: a bold marker + colour-tinted text. Under a
/// `marker_accent` theme the marker is always accent (the brand mark);
/// otherwise it follows the line's own semantic colour.
fn stderr_line(sem: Sem, msg: &str) -> String {
    let marker_sem = if theme::theme().marker_accent {
        Sem::Accent
    } else {
        sem
    };
    format!(
        "{} {}",
        console_style(marker_sem)
            .for_stderr()
            .bold()
            .apply_to(marker()),
        console_style(sem).for_stderr().apply_to(msg)
    )
}
```

**(d)** Replace `note` (gains the marker prefix; text stays dim with the `note:` prefix):

```rust
pub fn note(msg: impl std::fmt::Display) {
    eprintln!(
        "{} {}",
        console_style(Sem::Accent)
            .for_stderr()
            .bold()
            .apply_to(marker()),
        console_style(Sem::Muted)
            .for_stderr()
            .apply_to(format!("note: {msg}"))
    );
}
```

**(e)** Replace `heading` (gains the marker prefix):

```rust
pub fn heading(msg: impl std::fmt::Display) {
    let s = console_style(Sem::Accent).bold();
    println!("{} {}", s.clone().apply_to(marker()), s.apply_to(msg));
}
```

**(f)** Add `info`/`status` after `heading` (pure `_line` builders keep them testable like `stderr_line`):

```rust
/// Informational stdout line (answer content, not a status verdict):
/// accent bold marker + untinted text.
fn info_line(msg: &str) -> String {
    format!(
        "{} {}",
        console_style(Sem::Accent).bold().apply_to(marker()),
        msg
    )
}

pub fn info(msg: impl std::fmt::Display) {
    println!("{}", info_line(&msg.to_string()));
}

/// Progress/status stderr line: accent bold marker + untinted text.
fn status_line(msg: &str) -> String {
    format!(
        "{} {}",
        console_style(Sem::Accent).for_stderr().bold().apply_to(marker()),
        msg
    )
}

pub fn status(msg: impl std::fmt::Display) {
    eprintln!("{}", status_line(&msg.to_string()));
}
```

`success`/`error`/`warn`, `styled_ok`/`styled_err`, `status_sem`/`usage_sem`/`services_sem`, and `init_colors` are untouched. `logpane.rs` needs no change — it already styles via `console_style(Sem::…)`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test --locked -p pi output`
Expected: all `output::` tests PASS, including the pre-existing `stderr_line_is_plain_text_when_colours_disabled`, logpane, and table tests.

- [ ] **Step 5: Gate + commit**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
rtk git add crates/bin/src/output/mod.rs
rtk git commit -m "feat(output): route message styling through the active theme"
```

---

### Task 3: Themed spinner and tables

**Files:**
- Modify: `crates/bin/src/output/spinner.rs`
- Modify: `crates/bin/src/output/table.rs`
- Test: inline `#[cfg(test)]` in both files

**Interfaces:**
- Consumes (Task 1): `super::theme::theme()`, `Paint::template_token()`, `Paint::table()`.
- Produces: `spinner()` / `table()` / `header()` / `cell_sem()` — signatures unchanged, colours now theme-driven.

- [ ] **Step 1: Write the failing test (spinner)**

Add to the `tests` module in `crates/bin/src/output/spinner.rs`:

```rust
    #[test]
    fn spinner_template_embeds_the_theme_accent() {
        let t = spinner_template();
        assert!(t.starts_with("{spinner"), "{t}");
        assert!(t.ends_with("{msg}"), "{t}");
        // The active theme always has a coloured accent, so the template
        // must carry a colour token (named or numeric).
        assert!(t.contains(":."), "{t}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `rtk cargo test --locked -p pi output::spinner`
Expected: COMPILE ERROR — `spinner_template` not found.

- [ ] **Step 3: Implement themed spinner**

Replace the whole non-test body of `crates/bin/src/output/spinner.rs`:

```rust
/// `indicatif` template with the animated glyph in the theme accent colour.
/// The colour token goes through `console::Style::from_dotted_str`, which
/// accepts names ("cyan") and numeric 256-colour tokens ("161").
fn spinner_template() -> String {
    match super::theme::theme().accent.template_token() {
        Some(token) => format!("{{spinner:.{token}}} {{msg}}"),
        None => "{spinner} {msg}".to_string(),
    }
}

pub fn spinner(msg: impl Into<String>) -> indicatif::ProgressBar {
    let pb = indicatif::ProgressBar::new_spinner();
    pb.set_style(
        indicatif::ProgressStyle::with_template(&spinner_template())
            .expect("theme spinner template is valid"),
    );
    pb.set_message(msg.into());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}
```

- [ ] **Step 4: Run spinner tests**

Run: `rtk cargo test --locked -p pi output::spinner`
Expected: 2 tests PASS (template + existing lifecycle test).

- [ ] **Step 5: Write the failing test (table)**

Add to the `tests` module in `crates/bin/src/output/table.rs`:

```rust
    #[test]
    fn sem_colour_follows_the_theme() {
        let t = super::super::theme::theme();
        assert_eq!(sem_colour(Sem::Accent), t.accent.table());
        assert_eq!(sem_colour(Sem::Success), t.success.table());
        assert_eq!(sem_colour(Sem::Warn), t.warn.table());
        assert_eq!(sem_colour(Sem::Error), t.error.table());
        assert_eq!(sem_colour(Sem::Neutral), None);
        assert_eq!(sem_colour(Sem::Muted), None);
    }
```

(`comfy_table::Color` derives `PartialEq` — verified in the 7.2.2 registry source — so `assert_eq!` compiles.)

- [ ] **Step 6: Run test to verify it fails**

Run: `rtk cargo test --locked -p pi output::table`
Expected: FAIL — `sem_colour(Sem::Accent)` returns `Some(Color::Cyan)`, theme expects `Some(Color::AnsiValue(161))`.

- [ ] **Step 7: Implement themed table colours**

In `crates/bin/src/output/table.rs`, replace `header` and `sem_colour`:

```rust
/// Accent + bold header cells (accent colour from the active theme).
pub fn header<const N: usize>(cols: [&str; N]) -> Vec<Cell> {
    cols.iter()
        .map(|c| {
            let cell = Cell::new(c).add_attribute(Attribute::Bold);
            match super::theme::theme().accent.table() {
                Some(colour) => cell.fg(colour),
                None => cell,
            }
        })
        .collect()
}

fn sem_colour(sem: Sem) -> Option<Color> {
    let t = super::theme::theme();
    match sem {
        Sem::Success => t.success.table(),
        Sem::Error => t.error.table(),
        Sem::Warn => t.warn.table(),
        Sem::Accent => t.accent.table(),
        Sem::Muted | Sem::Neutral | Sem::Frame => None,
    }
}
```

- [ ] **Step 8: Run table tests**

Run: `rtk cargo test --locked -p pi output::table`
Expected: PASS.

- [ ] **Step 9: Gate + commit**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
rtk git add crates/bin/src/output/spinner.rs crates/bin/src/output/table.rs
rtk git commit -m "feat(output): themed spinner and table colours"
```

---

### Task 4: Route informational lines through `info()`/`status()` + docs

**Files:**
- Modify: `crates/bin/src/cli/commands.rs`
- Modify: `crates/bin/src/cli/init.rs`
- Modify: `crates/bin/src/cli/setup.rs`
- Modify: `crates/bin/src/agent/setup.rs`
- Modify: `crates/bin/src/agent/run.rs`
- Modify: `README.md`
- Test: existing suite (no literal-string tests exist for these lines — verified by grep)

**Interfaces:**
- Consumes (Task 2): `output::info`, `output::status`, plus existing `output::success`/`warn`.
- Produces: nothing new — call-site routing only. Rule: the line keeps its current stream (stdout → `info`, stderr → `status`); text content is unchanged unless noted.

Lines that MUST stay raw (do not touch): `println!("{table}")` (commands.rs:301, 359, 386, 533), JSON (`:331`, `:509`), SSE streams (`:316`, `:709`), `format_command_line` list lines (`:430`), indented `  {key}`/`  {file}` continuations (`:229`, `:235`), the `eprint!("type the project name to confirm: ")` prompt (`:472`), `agent migrate --list` TSV (`agent/migrate.rs:173`), logpane `print_line` internals.

- [ ] **Step 1: Reroute `cli/commands.rs`**

Six edits (line numbers from the current tree):

`:24` —

```rust
    output::status(format!("agent {} (api {})", version.version, version.api));
```

`:34-44` (deploy queued/started) —

```rust
    if accepted.queued {
        output::status(format!(
            "deployment {} queued behind the active deploy (latest wins); waiting...",
            accepted.deployment_id
        ));
    } else {
        output::status(format!(
            "deployment {} started; streaming logs:",
            accepted.deployment_id
        ));
    }
```

`:74` —

```rust
        output::status(format!(
            "no active deployment for '{project_name}' - nothing to cancel"
        ));
```

`:82` —

```rust
            Ok(decision) => output::status(format!(
                "deployment {} ({}): {decision}",
                d.id, d.status
            )),
```

`:223` —

```rust
        output::info(format!("no secrets stored for project '{project_name}'"));
```

`:268` —

```rust
        output::info("no projects deployed yet");
```

`:425-427` —

```rust
            output::status(format!(
                "no commands deployed for '{project_name}' - declare [commands] in rpi.toml and run `rpi deploy`"
            ));
```

`:468-471` (destructive-removal notice becomes a proper warn; the `eprint!` prompt on the next lines stays raw) —

```rust
        output::warn(format!(
            "this removes containers{}, the ingress route, workdir, secrets, deploy key and history of '{project}'",
            if volumes { ", VOLUMES (project data!)" } else { "" }
        ));
```

- [ ] **Step 2: Reroute `cli/init.rs` and `cli/setup.rs`**

`cli/init.rs:216` —

```rust
            crate::output::info("aborted: rpi.toml left unchanged");
```

`cli/init.rs:223` —

```rust
    crate::output::info("next: `rpi secrets send` (if you use secrets), then `rpi deploy`");
```

`cli/setup.rs:103` (stdout today, stays stdout) —

```rust
    crate::output::info("testing connection...");
```

- [ ] **Step 3: Reroute `agent/setup.rs` and `agent/run.rs`**

`agent/setup.rs:907` —

```rust
    if opts.dry_run {
        crate::output::info("(dry run — no changes made)");
    }
```

`agent/setup.rs:990-1015` (the `match &action` block; applied-state lines align with `SetupReport::print()` which already uses `success` for "ok (already present)") —

```rust
    match &action {
        SelfInstallAction::AlreadyCanonical => {
            crate::output::success(format!(
                "ok (already present): {} (running from it)",
                self_install::AGENT_BIN_PATH
            ));
        }
        SelfInstallAction::UpToDate => {
            crate::output::success(format!(
                "ok (already present): {} (binary up to date)",
                self_install::AGENT_BIN_PATH
            ));
        }
        SelfInstallAction::Installed => {
            let line = format!(
                "{}: {} (from {})",
                if dry_run { "would install" } else { "installed" },
                self_install::AGENT_BIN_PATH,
                installed_from.display(),
            );
            if dry_run {
                crate::output::info(line);
            } else {
                crate::output::success(line);
            }
        }
    }
```

`agent/setup.rs:1020-1022` —

```rust
        if let Some(note) = restart_agent_if_active(&HostSys).await {
            crate::output::info(note);
        }
```

`agent/run.rs:19-25` (drop the hand-written `warning:` prefix — the amber warn line carries it):

```rust
        Err(e) => {
            crate::output::warn(format!(
                "cannot create log directory {}: {e} – agent logs to stderr only",
                config.logs.dir.display()
            ));
            false
        }
```

- [ ] **Step 4: README**

In `README.md`, after the "Select a specific profile" code block (ends line 497), insert:

```markdown
### Console theme

`rpi` output uses the brand theme: a raspberry `▸` marker on every message
line, site green/amber for success/warn. Set `PI_THEME=classic` for the
pre-brand look (cyan accent, `●` marker). `NO_COLOR`, piping, and non-TTY
output disable styling entirely, as before.
```

Also update the sample block at lines 481-483 to match the branded TTY output:

```text
▸ no projects deployed yet
```

- [ ] **Step 5: Run the full suite**

Run: `rtk cargo test --locked`
Expected: PASS (no test asserts the old literal lines — verified).

- [ ] **Step 6: Manual smoke check**

Run: `cargo run -p pi --bin rpi -- ls --help` (any TTY-attached invocation) and one real command if a profile exists, e.g. `cargo run -p pi --bin rpi -- ls`.
Expected: every message line starts with a raspberry `▸`; `PI_THEME=classic` restores the cyan `●` look; piping to a file yields plain text.

- [ ] **Step 7: Gate + commit**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
rtk git add crates/bin/src/cli/commands.rs crates/bin/src/cli/init.rs crates/bin/src/cli/setup.rs crates/bin/src/agent/setup.rs crates/bin/src/agent/run.rs README.md
rtk git commit -m "feat(cli): brand informational lines via themed info/status helpers"
```
