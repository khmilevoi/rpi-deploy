# Console Colour Expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `rpi` output visibly colourful — dot markers, whole-line tinting, a coloured log-pane frame, coloured tables and spinner — all routed through one semantic palette.

**Architecture:** A single `Sem` (semantic role) enum in `crates/bin/src/output/mod.rs` is the source of truth for what a colour *means*. It renders to `console::Style` for messages and the pane, and to `comfy_table::Color` for tables. Every surface references it, so colours stay consistent and retunable in one place.

**Tech Stack:** Rust; `console` 0.15 (styling, TTY/NO_COLOR detection), `comfy-table` 7 (tables), `indicatif` 0.17 (spinner). All already in the workspace.

## Global Constraints

- No new dependencies. `console`, `comfy-table`, `indicatif` are already workspace deps.
- Respect `NO_COLOR` and non-TTY/redirected output — must auto-disable, exactly as today. `console`/`indicatif` handle this themselves; tables additionally call `force_no_tty()` when `NO_COLOR` is set.
- No new CLI flags; no configurable themes.
- Do not touch streamed docker/subprocess log *content* colour, `rpi logs`, or `rpi agent logs`.
- One semantic palette (`Sem`) is the single source of truth for colour meaning.
- Marker glyph is `console::Emoji("●", "*")` (ASCII fallback `*`).
- Palette: success=green, error=red, warn=yellow, muted=dim, accent=cyan, neutral pane border=bright-black (grey).
- Finish-of-work gate (from `CLAUDE.md`, run on Linux CI too): `rtk cargo fmt --all -- --check`, `rtk cargo clippy --all-targets --locked -- -D warnings`, `rtk cargo test --locked`.
- Colours are only emitted on a real attended terminal; unit tests run non-TTY, so styled output renders as plain text there (this is what keeps substring assertions valid).

---

## File Structure

- `crates/bin/src/output/mod.rs` — **modify.** Add `Sem` enum, `console_style`, `MARKER`, semantic classifiers (`status_sem`, `usage_sem`, `services_sem`); rewrite `success`/`error`/`warn`/`heading`; re-export table cell helpers.
- `crates/bin/src/output/logpane.rs` — **modify.** Thread a frame style through `top_border`/`side_line`/`bottom_border`/`render_frame`; grey border + cyan label while streaming; red frame + full dump on failure.
- `crates/bin/src/output/table.rs` — **modify.** `force_no_tty()` under `NO_COLOR`; add `header`, `cell`, `cell_sem` helpers mapping `Sem` → `comfy_table::Color`.
- `crates/bin/src/output/spinner.rs` — **modify.** Cyan spinner glyph.
- `crates/bin/src/cli/commands.rs` — **modify.** Colour `ls` (SERVICES) and `stats` (CPU/MEM) cells; cyan headers on the `ls`/`stats`/`status` tables.

---

## Task 1: Semantic palette + coloured messages

**Files:**
- Modify: `crates/bin/src/output/mod.rs` (imports line 1; helpers lines 26–59; tests 61–80)

**Interfaces:**
- Produces:
  - `pub enum Sem { Success, Error, Warn, Muted, Accent, Neutral }` (derives `Clone, Copy, PartialEq, Eq, Debug`)
  - `pub(crate) fn console_style(sem: Sem) -> console::Style`
  - unchanged public API: `success`, `error`, `warn`, `note`, `heading`, `styled_ok`, `styled_err`, `init_colors`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/bin/src/output/mod.rs`:

```rust
    #[test]
    fn stderr_line_is_plain_text_when_colours_disabled() {
        // Captured test output is not a TTY, so console styling is disabled:
        // the composed line must carry the text and no ANSI escape bytes.
        let line = stderr_line(Sem::Error, "boom");
        assert!(line.contains("boom"), "{line:?}");
        assert!(!line.contains('\u{1b}'), "no ANSI when disabled: {line:?}");
    }

    #[test]
    fn console_style_neutral_is_a_no_op() {
        assert_eq!(console_style(Sem::Neutral).apply_to("x").to_string(), "x");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `rtk cargo test -p pi-bin stderr_line_is_plain_text_when_colours_disabled`
Expected: FAIL — compile error `cannot find function 'stderr_line'` / `cannot find type 'Sem'`.

- [ ] **Step 3: Write minimal implementation**

Replace the top of `crates/bin/src/output/mod.rs`. Change the import line 1 from `use console::{style, Emoji};` to:

```rust
use console::{Emoji, Style};
```

Then replace the helpers block (current lines 26–59, from `pub fn success` through `styled_err`) with:

```rust
/// Marker glyph for semantic messages; degrades to `*` on terminals without
/// unicode/emoji support.
const MARKER: Emoji<'_, '_> = Emoji("●", "*");

/// Semantic role — the single source of truth for what a colour *means*.
/// Rendered to `console::Style` here and to `comfy_table::Color` in `table.rs`,
/// so one role stays consistent across messages, the pane, and tables.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sem {
    Success,
    Error,
    Warn,
    Muted,
    Accent,
    Neutral,
}

/// Role -> terminal style for `console`-rendered output (messages, pane).
pub(crate) fn console_style(sem: Sem) -> Style {
    let s = Style::new();
    match sem {
        Sem::Success => s.green(),
        Sem::Error => s.red(),
        Sem::Warn => s.yellow(),
        Sem::Accent => s.cyan(),
        Sem::Muted => s.dim(),
        Sem::Neutral => s,
    }
}

/// One stderr status line: a bold coloured marker + colour-tinted text.
fn stderr_line(sem: Sem, msg: &str) -> String {
    let base = console_style(sem).for_stderr();
    format!("{} {}", base.clone().bold().apply_to(MARKER), base.apply_to(msg))
}

pub fn success(msg: impl std::fmt::Display) {
    eprintln!("{}", stderr_line(Sem::Success, &msg.to_string()));
}

pub fn error(msg: impl std::fmt::Display) {
    eprintln!("{}", stderr_line(Sem::Error, &msg.to_string()));
}

pub fn warn(msg: impl std::fmt::Display) {
    eprintln!("{}", stderr_line(Sem::Warn, &msg.to_string()));
}

pub fn note(msg: impl std::fmt::Display) {
    eprintln!(
        "{}",
        console_style(Sem::Muted)
            .for_stderr()
            .apply_to(format!("note: {msg}"))
    );
}

pub fn heading(msg: impl std::fmt::Display) {
    println!("{}", console_style(Sem::Accent).bold().apply_to(msg));
}

/// Pure, string-returning variants for callers that build up a `String`
/// instead of printing directly (e.g. `render_doctor`).
pub fn styled_ok(text: &str) -> String {
    console_style(Sem::Success).apply_to(text).to_string()
}

pub fn styled_err(text: &str) -> String {
    console_style(Sem::Error).bold().apply_to(text).to_string()
}
```

Leave `no_color_requested` and `init_colors` (lines 12–25) unchanged.

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test -p pi-bin output`
Expected: PASS — the two new tests plus the existing `no_color_env_var_is_detected` and `styled_ok_and_err_are_plain_text_when_colors_disabled`.

- [ ] **Step 5: Lint and commit**

```bash
rtk cargo clippy -p pi-bin --all-targets -- -D warnings
rtk git add crates/bin/src/output/mod.rs
rtk git commit -m "feat(output): semantic palette + coloured dot-marker messages"
```

---

## Task 2: Coloured log-pane frame

**Files:**
- Modify: `crates/bin/src/output/logpane.rs` (helpers lines 3–97; `redraw` 162–171; `finish_err` 188–201; tests)

**Interfaces:**
- Consumes: `super::Sem`, `super::console_style` (from Task 1)
- Produces: internal `FrameStyle { border, label }`; `render_frame(..., style: &FrameStyle)`; no change to `LogPane`'s public surface (`new`, `push_line`, `finish_ok`, `finish_neutral`, `finish_err`).

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/bin/src/output/logpane.rs`:

```rust
    #[test]
    fn borders_are_plain_when_colours_disabled() {
        // Non-TTY test env => console styling disabled => the styled frame
        // pieces must be byte-identical to the unstyled box (no ANSI).
        let fs = neutral_frame();
        assert!(!top_border("build", 20, &fs).contains('\u{1b}'), "no ANSI in top");
        assert!(!bottom_border(12, &fs).contains('\u{1b}'), "no ANSI in border");
        assert!(!side_line("hi", 10, &fs).contains('\u{1b}'), "no ANSI in side");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `rtk cargo test -p pi-bin borders_are_plain_when_colours_disabled`
Expected: FAIL — compile error: `neutral_frame` not found and the border helpers take a different arity.

- [ ] **Step 3: Write minimal implementation**

In `crates/bin/src/output/logpane.rs`, add near the top (after `use console::Term;`):

```rust
use console::Style;

use super::{console_style, Sem};

/// Colours for one rendered frame: the box glyphs and the label can differ
/// (grey box + cyan label while streaming; all-red on failure).
struct FrameStyle {
    border: Style,
    label: Style,
}

/// Neutral streaming frame: bright-black (grey) border, cyan label.
fn neutral_frame() -> FrameStyle {
    FrameStyle {
        border: Style::new().black().bright(),
        label: console_style(Sem::Accent),
    }
}

/// Failure frame: border and label both red.
fn err_frame() -> FrameStyle {
    let red = console_style(Sem::Error);
    FrameStyle {
        border: red.clone(),
        label: red,
    }
}
```

Replace `top_border` (lines 3–11) with:

```rust
fn top_border(label: &str, width: usize, style: &FrameStyle) -> String {
    // "╭─ " + label + " " + fill + "╮" — visible width == `width`.
    let max_label_width = width.saturating_sub(5);
    let label: String = label.chars().take(max_label_width).collect();
    let prefix_len = 3 + label.chars().count() + 1; // "╭─ " + label + " "
    let fill = width.saturating_sub(prefix_len + 1); // +1 for the closing ╮
    format!(
        "{}{}{}",
        style.border.apply_to("╭─ "),
        style.label.apply_to(&label),
        style.border.apply_to(format!(" {}╮", "─".repeat(fill))),
    )
}
```

Replace `side_line` (lines 13–48) with the same truncation logic, wrapping only the side bars:

```rust
fn side_line(content: &str, width: usize, style: &FrameStyle) -> String {
    // Truncate by *visible* columns: ANSI CSI escape sequences (colours) carry
    // no width, so they pass through without spending the budget. This keeps
    // streamed colour intact while still fitting the box.
    let inner_width = width.saturating_sub(4); // "│ " + " │"
    let mut truncated = String::new();
    let mut visible = 0;
    let mut had_escape = false;
    let mut chars = content.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            had_escape = true;
            truncated.push(c);
            truncated.push(chars.next().unwrap()); // '['
            for c in chars.by_ref() {
                truncated.push(c);
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            if visible == inner_width {
                break;
            }
            truncated.push(c);
            visible += 1;
        }
    }
    // If colour was used and truncation dropped its reset, close it ourselves
    // so it can't bleed onto the padding, the border, or the next line.
    if had_escape && !truncated.ends_with("\x1b[0m") {
        truncated.push_str("\x1b[0m");
    }
    let pad = " ".repeat(inner_width.saturating_sub(visible));
    format!(
        "{}{truncated}{pad}{}",
        style.border.apply_to("│ "),
        style.border.apply_to(" │"),
    )
}
```

Replace `bottom_border` (lines 50–52) with:

```rust
fn bottom_border(width: usize, style: &FrameStyle) -> String {
    style
        .border
        .apply_to(format!("╰{}╯", "─".repeat(width.saturating_sub(2))))
        .to_string()
}
```

In `render_frame` (lines 62–97) add a `style: &FrameStyle` parameter and pass it to the three helpers. Change the signature and the three `rows.push(...)` calls:

```rust
fn render_frame(
    label: &str,
    visible: &std::collections::VecDeque<String>,
    width: usize,
    prev_rendered: usize,
    style: &FrameStyle,
) -> String {
    let mut rows = Vec::with_capacity(visible.len() + 2);
    rows.push(top_border(label, width, style));
    for l in visible {
        rows.push(side_line(l, width, style));
    }
    rows.push(bottom_border(width, style));

    let mut buf = String::new();
    // Return to the top of the previously drawn block (nothing on first paint).
    if prev_rendered > 0 {
        buf.push_str(&format!("\x1b[{prev_rendered}A"));
    }
    for row in &rows {
        buf.push_str("\r\x1b[2K"); // column 0 + erase the old row in place
        buf.push_str(row);
        buf.push_str("\r\n");
    }
    // If the block shrank, wipe stale rows below it, then move back up so the
    // cursor ends just beneath the smaller block.
    let shrink = prev_rendered.saturating_sub(rows.len());
    for _ in 0..shrink {
        buf.push_str("\r\x1b[2K\r\n");
    }
    if shrink > 0 {
        buf.push_str(&format!("\x1b[{shrink}A"));
    }
    buf
}
```

Update `redraw` (lines 162–171) to pass the neutral frame:

```rust
    fn redraw(&mut self) {
        let (_, cols) = self.term.size();
        let width = (cols as usize).max(20);
        let frame = render_frame(&self.label, &self.visible, width, self.rendered, &neutral_frame());
        let _ = self.term.write_str(&frame);
        let _ = self.term.flush();
        self.rendered = self.visible.len() + 2; // + top and bottom border
    }
```

Replace `finish_err` (lines 188–201) with — recolour the final frame red and leave it, then dump the full log under a dim separator:

```rust
    pub fn finish_err(self, summary: &str) {
        if self.interactive {
            let (_, cols) = self.term.size();
            let width = (cols as usize).max(20);
            // Recolour the final frame red in place and leave it on screen as
            // the "here it stopped" marker (no clear).
            let frame =
                render_frame(&self.label, &self.visible, width, self.rendered, &err_frame());
            let _ = self.term.write_str(&frame);
            let _ = self.term.flush();
            // Full captured log below the framed tail — the complete record,
            // since there is no log file. A dim separator marks it as such.
            (self.print_line)(
                &console_style(Sem::Muted)
                    .apply_to("— full log —")
                    .to_string(),
            );
            for l in &self.full {
                (self.print_line)(l);
            }
        }
        crate::output::error(summary);
    }
```

Leave `finish_ok` (173–178) and `finish_neutral` (180–186) unchanged.

- [ ] **Step 4: Fix the existing pane tests for the new arity/behaviour**

The pure-helper tests (`top_border_wraps_label_and_fills_width`, `side_line_*`, `bottom_border_matches_width`, `top_border_truncates_*`, `render_frame_*`) call the helpers without a style. Update each call to pass `&neutral_frame()` as the final argument. The colour-aware assertions still hold because styling is disabled in tests (plain output). For example:

```rust
    #[test]
    fn side_line_ignores_color_codes_when_measuring_width() {
        let line = side_line("\x1b[31mhello\x1b[0m", 10, &neutral_frame());
        assert_eq!(line, "│ \x1b[31mhello\x1b[0m  │", "{line:?}");
    }
```

And update `render_frame_first_paint_does_not_move_the_cursor` / `render_frame_overwrites_previous_block_in_place` to pass `&neutral_frame()`:

```rust
        let frame = render_frame("build", &visible, 20, 0, &neutral_frame());
        // ...
        let frame = render_frame("build", &visible, 20, 3, &neutral_frame());
```

Update `finish_err_dumps_full_history_when_interactive` to expect the dim separator ahead of the dump (styling is disabled in tests, so the separator is the plain text `— full log —`):

```rust
    #[test]
    fn finish_err_dumps_full_history_when_interactive() {
        let printed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut pane = LogPane::new_recording("test", 3, true, printed.clone());
        pane.push_line("one");
        pane.push_line("two");
        pane.finish_err("boom");
        assert_eq!(*printed.lock().unwrap(), vec!["— full log —", "one", "two"]);
    }
```

`finish_err_does_not_reprint_lines_already_streamed_live` is non-interactive, so its path is unchanged — leave it asserting `vec!["one", "two"]`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `rtk cargo test -p pi-bin logpane`
Expected: PASS — all `output::logpane::tests`, including the new `borders_are_plain_when_colours_disabled`.

- [ ] **Step 6: Lint and commit**

```bash
rtk cargo clippy -p pi-bin --all-targets -- -D warnings
rtk git add crates/bin/src/output/logpane.rs
rtk git commit -m "feat(output): grey/cyan pane frame, red frame + dump on failure"
```

---

## Task 3: Coloured tables

**Files:**
- Modify: `crates/bin/src/output/mod.rs` (add classifiers + re-exports)
- Modify: `crates/bin/src/output/table.rs` (helpers)
- Modify: `crates/bin/src/cli/commands.rs` (`ls` 271–294; `stats` 328–355; `print_agent_status` 486–502)

**Interfaces:**
- Consumes: `Sem` (Task 1)
- Produces:
  - `pub fn status_sem(state: &str, health: Option<&str>) -> Sem`
  - `pub fn usage_sem(percent: f64) -> Sem`
  - `pub fn services_sem(states: &[&str]) -> Sem`
  - `pub fn header<const N: usize>(cols: [&str; N]) -> Vec<comfy_table::Cell>`
  - `pub fn cell(text: impl Into<String>) -> comfy_table::Cell`
  - `pub fn cell_sem(text: impl Into<String>, sem: Sem) -> comfy_table::Cell`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/bin/src/output/mod.rs`:

```rust
    #[test]
    fn usage_sem_thresholds() {
        assert_eq!(usage_sem(10.0), Sem::Neutral);
        assert_eq!(usage_sem(70.0), Sem::Warn);
        assert_eq!(usage_sem(89.9), Sem::Warn);
        assert_eq!(usage_sem(90.0), Sem::Error);
    }

    #[test]
    fn status_sem_maps_states_and_health() {
        assert_eq!(status_sem("running", None), Sem::Success);
        assert_eq!(status_sem("running", Some("unhealthy")), Sem::Warn);
        assert_eq!(status_sem("restarting", None), Sem::Warn);
        assert_eq!(status_sem("exited", None), Sem::Error);
    }

    #[test]
    fn services_sem_takes_the_worst_state() {
        assert_eq!(services_sem(&["running", "running"]), Sem::Success);
        assert_eq!(services_sem(&["running", "restarting"]), Sem::Warn);
        assert_eq!(services_sem(&["running", "exited"]), Sem::Error);
        assert_eq!(services_sem(&[]), Sem::Neutral);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `rtk cargo test -p pi-bin sem`
Expected: FAIL — `cannot find function 'usage_sem'` / `status_sem` / `services_sem`.

- [ ] **Step 3: Write the classifiers**

Append to `crates/bin/src/output/mod.rs` (below the message helpers, above the `tests` module):

```rust
/// Container state (+ optional health) -> semantic role.
/// running/healthy = success; restarting/paused/created or a non-healthy
/// healthcheck = warn; everything else (exited, dead, unknown) = error.
pub fn status_sem(state: &str, health: Option<&str>) -> Sem {
    if matches!(health, Some("unhealthy") | Some("starting")) {
        return Sem::Warn;
    }
    match state {
        "running" => Sem::Success,
        "restarting" | "created" | "paused" => Sem::Warn,
        _ => Sem::Error,
    }
}

/// Resource-usage percentage -> semantic role: >=90 error, >=70 warn, else none.
pub fn usage_sem(percent: f64) -> Sem {
    if percent >= 90.0 {
        Sem::Error
    } else if percent >= 70.0 {
        Sem::Warn
    } else {
        Sem::Neutral
    }
}

/// Aggregate of a project's service states -> the worst role among them
/// (Error worse than Warn worse than Success); empty = Neutral.
pub fn services_sem(states: &[&str]) -> Sem {
    fn rank(sem: Sem) -> u8 {
        match sem {
            Sem::Error => 3,
            Sem::Warn => 2,
            Sem::Success => 1,
            _ => 0,
        }
    }
    states
        .iter()
        .map(|s| status_sem(s, None))
        .max_by_key(|s| rank(*s))
        .unwrap_or(Sem::Neutral)
}
```

Add the table-helper re-export next to the existing `pub use table::table;` (line 4):

```rust
pub use table::{cell, cell_sem, header, table};
```

- [ ] **Step 4: Implement the table helpers**

Replace the whole non-test body of `crates/bin/src/output/table.rs` with:

```rust
use comfy_table::{Attribute, Cell, Color};

use crate::output::Sem;

pub fn table() -> comfy_table::Table {
    let mut t = comfy_table::Table::new();
    t.load_preset(comfy_table::presets::UTF8_FULL)
        .apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS)
        .set_content_arrangement(comfy_table::ContentArrangement::Dynamic);
    // comfy-table auto-suppresses styling off a TTY, but doesn't honour
    // NO_COLOR itself — do it here so tables match the rest of the CLI.
    if std::env::var_os("NO_COLOR").is_some() {
        t.force_no_tty();
    }
    t
}

/// Cyan + bold header cells.
pub fn header<const N: usize>(cols: [&str; N]) -> Vec<Cell> {
    cols.iter()
        .map(|c| Cell::new(c).fg(Color::Cyan).add_attribute(Attribute::Bold))
        .collect()
}

fn sem_colour(sem: Sem) -> Option<Color> {
    match sem {
        Sem::Success => Some(Color::Green),
        Sem::Error => Some(Color::Red),
        Sem::Warn => Some(Color::Yellow),
        Sem::Accent => Some(Color::Cyan),
        Sem::Muted | Sem::Neutral => None,
    }
}

/// Uncoloured value cell.
pub fn cell(text: impl Into<String>) -> Cell {
    Cell::new(text.into())
}

/// Value cell coloured by semantic role (Neutral/Muted = no colour).
pub fn cell_sem(text: impl Into<String>, sem: Sem) -> Cell {
    let c = Cell::new(text.into());
    match sem_colour(sem) {
        Some(col) => c.fg(col),
        None => c,
    }
}
```

Keep the existing `#[cfg(test)] mod tests { ... table_renders_header_and_rows ... }` — it still compiles (`set_header(vec!["NAME","BRANCH"])` accepts `Vec<&str>`).

- [ ] **Step 5: Colour the `ls` table (`commands.rs` 271–294)**

Replace the header and row construction:

```rust
    let mut table = output::table();
    table.set_header(output::header([
        "NAME", "BRANCH", "HOSTNAME", "PORT", "EXPOSE", "SERVICES",
    ]));
    for p in projects {
        let services = if p.services.is_empty() {
            "-".to_string()
        } else {
            p.services
                .iter()
                .map(|s| format!("{}:{}", s.service, s.state))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let services_sem = output::services_sem(
            &p.services.iter().map(|s| s.state.as_str()).collect::<Vec<_>>(),
        );
        let expose = expose_cell(&p.expose, p.lan_ip.as_deref(), p.host_port);
        table.add_row(vec![
            output::cell(p.name),
            output::cell(p.branch),
            output::cell(p.hostname.unwrap_or_else(|| "-".into())),
            output::cell(p.host_port.to_string()),
            output::cell(expose),
            output::cell_sem(services, services_sem),
        ]);
    }
    println!("{table}");
```

- [ ] **Step 6: Colour the `stats` tables (`commands.rs` 328–355)**

Replace the host table + services table blocks:

```rust
    let mut host_table = output::table();
    host_table.set_header(output::header(["CPU", "MEM", "DISK", "UPTIME"]));
    let host_mem_pct = if resp.host.mem_total_bytes > 0 {
        resp.host.mem_used_bytes as f64 / resp.host.mem_total_bytes as f64 * 100.0
    } else {
        0.0
    };
    host_table.add_row(vec![
        output::cell_sem(
            format!("{:.1}%", resp.host.cpu_percent),
            output::usage_sem(resp.host.cpu_percent),
        ),
        output::cell_sem(
            format!("{}/{} bytes", resp.host.mem_used_bytes, resp.host.mem_total_bytes),
            output::usage_sem(host_mem_pct),
        ),
        output::cell_sem(
            format!("{}%", resp.host.disk_used_percent),
            output::usage_sem(resp.host.disk_used_percent as f64),
        ),
        output::cell(human_duration(resp.host.uptime_secs)),
    ]);
    println!("{host_table}");

    if !resp.projects.is_empty() {
        let mut services_table = output::table();
        services_table.set_header(output::header(["PROJECT", "SERVICE", "CPU", "MEM"]));
        for p in resp.projects {
            let project_name = p.project.clone();
            for s in p.services {
                let mem_pct = if s.mem_limit_bytes > 0 {
                    s.mem_used_bytes as f64 / s.mem_limit_bytes as f64 * 100.0
                } else {
                    0.0
                };
                services_table.add_row(vec![
                    output::cell(project_name.clone()),
                    output::cell(s.service),
                    output::cell_sem(
                        format!("{:.1}%", s.cpu_percent),
                        output::usage_sem(s.cpu_percent),
                    ),
                    output::cell_sem(
                        format!("{}/{} bytes", s.mem_used_bytes, s.mem_limit_bytes),
                        output::usage_sem(mem_pct),
                    ),
                ]);
            }
        }
        println!("{services_table}");
    }
```

If `resp.host.disk_used_percent` is already an `f64`, drop the `as f64` cast on that line (clippy `unnecessary_cast` will flag it — remove if warned).

- [ ] **Step 7: Cyan header on the `status` table (`commands.rs` 486–487)**

In `print_agent_status`, change only the header (rows stay plain strings):

```rust
    let mut table = output::table();
    table.set_header(output::header(["FIELD", "VALUE"]));
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `rtk cargo test -p pi-bin`
Expected: PASS — new `usage_sem`/`status_sem`/`services_sem` tests, existing table test, and the whole `pi-bin` suite compiles with the `commands.rs` changes.

- [ ] **Step 9: Lint and commit**

```bash
rtk cargo clippy -p pi-bin --all-targets -- -D warnings
rtk git add crates/bin/src/output/mod.rs crates/bin/src/output/table.rs crates/bin/src/cli/commands.rs
rtk git commit -m "feat(output): cyan table headers, status/usage-coloured cells"
```

---

## Task 4: Cyan spinner

**Files:**
- Modify: `crates/bin/src/output/spinner.rs` (line 4)

**Interfaces:**
- No signature change; `spinner(msg) -> indicatif::ProgressBar`.

- [ ] **Step 1: Change the template to colour the glyph**

In `crates/bin/src/output/spinner.rs`, change the template string on line 4:

```rust
        indicatif::ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .expect("static spinner template is valid"),
```

- [ ] **Step 2: Run the existing test to verify it still passes**

Run: `rtk cargo test -p pi-bin spinner`
Expected: PASS — `spinner_starts_unfinished_and_can_be_finished` (the `.cyan` style token is valid, so `with_template(...).expect(...)` does not panic).

- [ ] **Step 3: Lint and commit**

```bash
rtk cargo clippy -p pi-bin --all-targets -- -D warnings
rtk git add crates/bin/src/output/spinner.rs
rtk git commit -m "feat(output): cyan spinner glyph"
```

---

## Task 5: Verify & finish

**Files:** none (verification only)

- [ ] **Step 1: Run the full workspace gate (matches CI)**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --all-targets --locked -- -D warnings
rtk cargo test --locked
```
Expected: fmt clean, clippy clean, all tests pass. If fmt reports a diff, run `rtk cargo fmt --all` and amend/commit.

- [ ] **Step 2: Manual visual check on a real terminal**

Colours only render on an attended TTY, so unit tests can't confirm the look. Build and eyeball:

```bash
rtk cargo build -p pi-bin
# an error path (red dot + red text), no config in an empty dir:
./target/debug/rpi.exe deploy
```
Expected: a red `●` followed by red message text. Confirm against the agreed mockup (`scratchpad/color-mockup.ps1`) — dot markers, whole-line tint, cyan headings. Table/pane colouring needs a live agent connection; if unavailable, rely on the mockup as the reference.

- [ ] **Step 3: Confirm NO_COLOR still strips everything**

```bash
NO_COLOR=1 ./target/debug/rpi.exe deploy
```
Expected: plain `* cannot read rpi.toml ...` (fallback marker, no ANSI). This proves the palette respects `NO_COLOR` end to end.

- [ ] **Step 4: Final commit if fmt changed anything**

```bash
rtk git add -A
rtk git commit -m "style: cargo fmt" || echo "nothing to format"
```
