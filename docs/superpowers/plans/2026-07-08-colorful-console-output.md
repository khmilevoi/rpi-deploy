# Colorful, Structured Console Output Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give every `rpi` command a consistent, colorful, structured presentation (tables, semantic message styling, a bordered scrolling log pane for long operations), and fix the real bug behind glued-together docker log lines at its root.

**Architecture:** A new shared `crates/bin/src/output/` module (semantic print helpers, a `comfy-table` preset, an `indicatif` spinner, and a bordered `LogPane`) that every CLI command routes through instead of ad-hoc `println!`/`eprintln!`. Two small fixes in `crates/infrastructure` (force plain BuildKit progress output; sanitize captured subprocess lines) remove the actual cause of cursor-corrupted terminal output, independent of the styling work.

**Tech Stack:** `console` 0.15 (styled text, TTY/`CLICOLOR` detection, `Emoji` fallback), `indicatif` 0.17 (spinners), `comfy-table` 7 (tables). All new to this workspace.

Full design background: `docs/superpowers/specs/2026-07-08-colorful-console-output-design.md`.

## Global Constraints

- Pin exactly `console = "0.15"`, `indicatif = "0.17"`, `comfy-table = "7"` in workspace `Cargo.toml`, referenced via `{ workspace = true }` from `crates/bin/Cargo.toml` (existing repo convention).
- No new CLI flags (no `--no-color`/`--color`). Only the `NO_COLOR` env var (checked explicitly, since `console` does not check it itself) plus `console`'s own automatic TTY/`CLICOLOR` detection.
- `rpi logs` and `rpi agent logs` are never touched — they keep today's plain `println!(line)` streaming.
- `rpi stats --json` / `rpi status --json` code paths are never touched.
- `LogPane` max visible lines is hardcoded to `10`, no configurability.
- Before considering any task in this plan done, the change must build; before considering the **whole plan** done, run (per `CLAUDE.md`): `cargo fmt --all -- --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`. If `cargo fmt` reports a diff, run `cargo fmt --all` and commit the result rather than hand-editing formatting.
- Every source snippet below is the complete replacement for the shown range — apply it verbatim, don't paraphrase.

---

## Task 1: Add `console`, `indicatif`, `comfy-table` dependencies

**Files:**
- Modify: `Cargo.toml:9-31` (workspace root)
- Modify: `crates/bin/Cargo.toml:9-24`

**Interfaces:**
- Produces: `console::*`, `indicatif::*`, `comfy_table::*` become available to the `pi` (bin) crate.

- [ ] **Step 1: Add the three crates to the workspace dependency table**

In `Cargo.toml`, find this line inside `[workspace.dependencies]`:

```toml
shlex = "1"
```

Replace it with:

```toml
shlex = "1"
console = "0.15"
indicatif = "0.17"
comfy-table = "7"
```

- [ ] **Step 2: Add them as workspace deps of the `pi` bin crate**

In `crates/bin/Cargo.toml`, find this line in `[dependencies]`:

```toml
shlex = { workspace = true }
```

Replace it with:

```toml
shlex = { workspace = true }
console = { workspace = true }
indicatif = { workspace = true }
comfy-table = { workspace = true }
```

- [ ] **Step 3: Verify the workspace still resolves and builds**

Run: `cargo check -p pi --locked`
Expected: success (the crates aren't used by any code yet, so this only proves the version pins resolve and `Cargo.lock` updates cleanly).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock crates/bin/Cargo.toml
git commit -m "build: add console, indicatif, comfy-table dependencies"
```

---

## Task 2: Fix log-glueing at the source — sanitize captured subprocess lines

**Files:**
- Modify: `crates/infrastructure/src/process.rs:47-52`
- Test: `crates/infrastructure/src/process.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub fn sanitize_line(line: &str) -> String` in `pi_infrastructure::process`, reused later by `output::LogPane` (Task 7).

This is the defense-in-depth half of the log-glueing fix: strip any bare `\r` or ANSI CSI escape sequence that reaches a captured subprocess line, so a stray control sequence can never leave the terminal cursor in a corrupted position when the line is later printed.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `crates/infrastructure/src/process.rs` (after the existing `use` lines, alongside the other tests):

```rust
    #[test]
    fn sanitize_line_strips_bare_carriage_returns() {
        assert_eq!(sanitize_line("hello\rworld"), "helloworld");
    }

    #[test]
    fn sanitize_line_strips_ansi_csi_sequences() {
        // BuildKit-style cursor-up + erase-line sequence embedded mid-line.
        assert_eq!(sanitize_line("\x1b[1A\x1b[2Kstep 4/9"), "step 4/9");
    }

    #[test]
    fn sanitize_line_leaves_plain_text_untouched() {
        let plain = "Sending build context to Docker daemon  2.048kB";
        assert_eq!(sanitize_line(plain), plain);
    }
```

- [ ] **Step 2: Run the tests to verify they fail (function doesn't exist yet)**

Run: `cargo test -p pi-infrastructure --lib process::tests::sanitize_line`
Expected: FAIL with "cannot find function `sanitize_line`"

- [ ] **Step 3: Implement `sanitize_line` and wire it into `forward_lines`**

Replace `crates/infrastructure/src/process.rs:47-52`:

```rust
async fn forward_lines<R: AsyncRead + Unpin>(reader: R, log: Arc<dyn LogSink>) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        log.line(&line);
    }
}
```

with:

```rust
/// Strips bare CR and ANSI CSI escape sequences (`ESC '[' ... final byte`)
/// from a captured subprocess line. Defense in depth: BuildKit and other
/// interactive-terminal-UI subprocess output uses these to redraw progress
/// in place; forwarding them through unchanged corrupts the cursor position
/// of whatever prints the line later. Not a full VT100 parser — CSI final
/// bytes are technically `@`-`~`, but every real-world use (colors, cursor
/// movement, line clearing) ends in an ASCII letter, so that's the cutoff.
pub fn sanitize_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => continue,
            '\u{1b}' if chars.peek() == Some(&'[') => {
                chars.next(); // consume '['
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

async fn forward_lines<R: AsyncRead + Unpin>(reader: R, log: Arc<dyn LogSink>) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        log.line(&sanitize_line(&line));
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p pi-infrastructure --lib process::`
Expected: PASS — all tests in `process.rs`, including the three new ones and the pre-existing `run_streamed_forwards_output_lines` (unaffected: `"git version..."` has no `\r`/ANSI to strip).

- [ ] **Step 5: Commit**

```bash
git add crates/infrastructure/src/process.rs
git commit -m "fix(infrastructure): sanitize CR/ANSI control sequences from streamed subprocess lines"
```

---

## Task 3: Fix log-glueing at the source — force plain BuildKit progress output

**Files:**
- Modify: `crates/infrastructure/src/docker.rs:193-198`
- Test: `crates/infrastructure/src/docker.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: none new.
- Produces: every `docker compose ...` invocation built via `DockerComposeRuntime::compose` now sets `BUILDKIT_PROGRESS=plain`.

This is the root-cause half of the log-glueing fix: without this, BuildKit renders its interactive multi-line progress UI (bare `\r` + ANSI cursor movement) even though our stdout is a pipe, not a terminal.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/infrastructure/src/docker.rs` (near `compose_args_shape`):

```rust
    #[test]
    fn compose_sets_buildkit_progress_plain() {
        let dir = tempfile::tempdir().unwrap();
        let s = stack(dir.path());
        let runtime = DockerComposeRuntime;
        let cmd = runtime.compose(&s, &["build"]);
        let value = cmd
            .as_std()
            .get_envs()
            .find(|(k, _)| *k == std::ffi::OsStr::new("BUILDKIT_PROGRESS"))
            .and_then(|(_, v)| v);
        assert_eq!(value, Some(std::ffi::OsStr::new("plain")));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p pi-infrastructure --lib docker::tests::compose_sets_buildkit_progress_plain`
Expected: FAIL (assertion: `None != Some("plain")`)

- [ ] **Step 3: Set the env var in `compose()`**

Replace `crates/infrastructure/src/docker.rs:193-198`:

```rust
    fn compose(&self, stack: &ComposeStack, tail: &[&str]) -> Command {
        let mut cmd = Command::new("docker");
        cmd.args(compose_args(&stack.project_name, &file_chain(stack), tail));
        cmd.current_dir(&stack.workdir);
        cmd
    }
```

with:

```rust
    fn compose(&self, stack: &ComposeStack, tail: &[&str]) -> Command {
        let mut cmd = Command::new("docker");
        cmd.args(compose_args(&stack.project_name, &file_chain(stack), tail));
        cmd.current_dir(&stack.workdir);
        // BuildKit's fancy multi-line, cursor-redrawing progress UI corrupts
        // captured output even when stdout is a pipe, not a TTY — plain mode
        // emits ordinary newline-terminated lines instead. Applies uniformly
        // to every subcommand; a no-op for ones that don't build anything.
        cmd.env("BUILDKIT_PROGRESS", "plain");
        cmd
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p pi-infrastructure --lib docker::`
Expected: PASS — all tests in `docker.rs`, including the new one.

- [ ] **Step 5: Commit**

```bash
git add crates/infrastructure/src/docker.rs
git commit -m "fix(infrastructure): force BUILDKIT_PROGRESS=plain on every compose invocation"
```

---

## Task 4: `output` module skeleton — colors, `NO_COLOR`, semantic message helpers

**Files:**
- Create: `crates/bin/src/output/mod.rs`
- Modify: `crates/bin/src/main.rs:1-4`

**Interfaces:**
- Produces: `output::init_colors()`, `output::success/error/warn/note/heading(msg: impl Display)`, `output::styled_ok/styled_err(text: &str) -> String`.

- [ ] **Step 1: Create the `output` module directory and `mod.rs`**

Create `crates/bin/src/output/mod.rs`:

```rust
use console::{style, Emoji};

fn no_color_requested() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

/// Call once at process start. `console` implements the clicolors spec
/// (`CLICOLOR`/`CLICOLOR_FORCE`) and TTY detection on its own, but does not
/// check `NO_COLOR` — this stays consistent with clap's own `--help`
/// coloring, which is `NO_COLOR`-aware transitively via `anstream`.
pub fn init_colors() {
    if no_color_requested() {
        console::set_colors_enabled(false);
        console::set_colors_enabled_stderr(false);
    }
}

pub fn success(msg: impl std::fmt::Display) {
    println!("{} {msg}", style(Emoji("✓", "OK")).green());
}

pub fn error(msg: impl std::fmt::Display) {
    eprintln!(
        "{} {msg}",
        style(Emoji("✗", "ERR")).red().bold().for_stderr()
    );
}

pub fn warn(msg: impl std::fmt::Display) {
    eprintln!("{} {msg}", style(Emoji("⚠", "!")).yellow().for_stderr());
}

pub fn note(msg: impl std::fmt::Display) {
    eprintln!("{}", style(format!("note: {msg}")).dim().for_stderr());
}

pub fn heading(msg: impl std::fmt::Display) {
    println!("{}", style(msg).bold());
}

/// Pure, string-returning variants for callers that build up a `String`
/// instead of printing directly (e.g. `render_doctor`, which must stay a
/// testable pure function).
pub fn styled_ok(text: &str) -> String {
    style(text).green().to_string()
}

pub fn styled_err(text: &str) -> String {
    style(text).red().bold().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_color_env_var_is_detected() {
        std::env::set_var("NO_COLOR", "1");
        assert!(no_color_requested());
        std::env::remove_var("NO_COLOR");
        assert!(!no_color_requested());
    }

    #[test]
    fn styled_ok_and_err_are_plain_text_when_colors_disabled() {
        // Test stdout isn't a TTY, so `console` auto-disables styling: the
        // returned string must be exactly the input, no ANSI codes.
        assert_eq!(styled_ok("PASS"), "PASS");
        assert_eq!(styled_err("FAIL"), "FAIL");
    }
}
```

- [ ] **Step 2: Register the module and initialize colors in `main.rs`**

Replace `crates/bin/src/main.rs:1-4`:

```rust
mod agent;
mod cli;
mod duration;
mod proto;
```

with:

```rust
mod agent;
mod cli;
mod duration;
mod output;
mod proto;
```

(The `output::init_colors()` call itself is added in Task 8, when `main()` is restructured — this step only registers the module so it compiles.)

- [ ] **Step 3: Run the tests**

Run: `cargo test -p pi --lib output::`
Expected: PASS

- [ ] **Step 4: Confirm the crate still compiles**

Run: `cargo check -p pi --locked`
Expected: success. Do **not** run `cargo clippy -D warnings` yet — none of these new `pub fn`s (`init_colors`, `success`, `error`, `warn`, `note`, `heading`, `styled_ok`, `styled_err`) are called from `main()`'s reachable code until Tasks 8/9/10/11/16, so clippy's `dead_code` lint will legitimately flag them right now. That's expected and resolves itself as later tasks wire each function in; the full `-D warnings` gate belongs to Task 19.

- [ ] **Step 5: Commit**

```bash
git add crates/bin/src/output/mod.rs crates/bin/src/main.rs
git commit -m "feat(cli): add output module with colored semantic message helpers"
```

---

## Task 5: `output::table()` — shared table preset

**Files:**
- Create: `crates/bin/src/output/table.rs`
- Modify: `crates/bin/src/output/mod.rs` (add `pub mod table;`)

**Interfaces:**
- Produces: `output::table::table() -> comfy_table::Table`, re-exported as `output::table()`.

- [ ] **Step 1: Write the failing test**

Create `crates/bin/src/output/table.rs`:

```rust
pub fn table() -> comfy_table::Table {
    let mut t = comfy_table::Table::new();
    t.load_preset(comfy_table::presets::UTF8_FULL)
        .apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS)
        .set_content_arrangement(comfy_table::ContentArrangement::Dynamic);
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_renders_header_and_rows() {
        let mut t = table();
        t.set_header(vec!["NAME", "BRANCH"]);
        t.add_row(vec!["rateme", "main"]);
        let rendered = t.to_string();
        assert!(rendered.contains("NAME"), "{rendered}");
        assert!(rendered.contains("rateme"), "{rendered}");
        assert!(rendered.contains("main"), "{rendered}");
    }
}
```

- [ ] **Step 2: Register the submodule and re-export**

In `crates/bin/src/output/mod.rs`, add near the top (after the `use console::{style, Emoji};` line):

```rust
mod table;
pub use table::table;
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p pi --lib output::table::`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/bin/src/output/table.rs crates/bin/src/output/mod.rs
git commit -m "feat(cli): add output::table() shared table preset"
```

---

## Task 6: `output::spinner()` — shared spinner style

**Files:**
- Create: `crates/bin/src/output/spinner.rs`
- Modify: `crates/bin/src/output/mod.rs` (add `pub mod spinner;`)

**Interfaces:**
- Produces: `output::spinner(msg: impl Into<String>) -> indicatif::ProgressBar`.

- [ ] **Step 1: Write the failing test**

Create `crates/bin/src/output/spinner.rs`:

```rust
pub fn spinner(msg: impl Into<String>) -> indicatif::ProgressBar {
    let pb = indicatif::ProgressBar::new_spinner();
    pb.set_style(
        indicatif::ProgressStyle::with_template("{spinner} {msg}")
            .expect("static spinner template is valid"),
    );
    pb.set_message(msg.into());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_starts_unfinished_and_can_be_finished() {
        let pb = spinner("connecting...");
        assert!(!pb.is_finished());
        pb.finish_and_clear();
        assert!(pb.is_finished());
    }
}
```

- [ ] **Step 2: Register the submodule and re-export**

In `crates/bin/src/output/mod.rs`, add next to the `table` registration:

```rust
mod spinner;
pub use spinner::spinner;
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p pi --lib output::spinner::`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/bin/src/output/spinner.rs crates/bin/src/output/mod.rs
git commit -m "feat(cli): add output::spinner() shared spinner style"
```

---

## Task 7: `output::LogPane` — bordered scrolling log pane

**Files:**
- Create: `crates/bin/src/output/logpane.rs`
- Modify: `crates/bin/src/output/mod.rs` (add `pub mod logpane;` and re-export)

**Interfaces:**
- Consumes: `pi_infrastructure::process::sanitize_line` (Task 2), `output::success/note/error` (Task 4).
- Produces: `output::LogPane::new(label: impl Into<String>, max_visible: usize) -> LogPane`, `.push_line(&mut self, line: &str)`, `.finish_ok(self, summary: &str)`, `.finish_neutral(self, summary: &str)`, `.finish_err(self, summary: &str)`. Used by Task 17.

- [ ] **Step 1: Write the failing tests for the pure border-drawing helpers**

Create `crates/bin/src/output/logpane.rs`:

```rust
use console::Term;

fn top_border(label: &str, width: usize) -> String {
    let prefix = format!("╭─ {label} ");
    let prefix_len = prefix.chars().count();
    let fill = width.saturating_sub(prefix_len + 1); // +1 for the closing ╮
    format!("{prefix}{}╮", "─".repeat(fill))
}

fn side_line(content: &str, width: usize) -> String {
    let inner_width = width.saturating_sub(4); // "│ " + " │"
    let truncated: String = content.chars().take(inner_width).collect();
    let pad = inner_width.saturating_sub(truncated.chars().count());
    format!("│ {truncated}{} │", " ".repeat(pad))
}

fn bottom_border(width: usize) -> String {
    format!("╰{}╯", "─".repeat(width.saturating_sub(2)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_border_wraps_label_and_fills_width() {
        let line = top_border("build", 20);
        assert!(line.starts_with("╭─ build "), "{line}");
        assert!(line.ends_with('╮'), "{line}");
        assert_eq!(line.chars().count(), 20, "{line}");
    }

    #[test]
    fn side_line_pads_short_content_to_full_width() {
        let line = side_line("hi", 10);
        assert_eq!(line.chars().count(), 10, "{line}");
        assert!(line.starts_with("│ hi"), "{line}");
        assert!(line.ends_with(" │"), "{line}");
    }

    #[test]
    fn side_line_truncates_content_wider_than_the_box() {
        let line = side_line("this is way too long for the box", 10);
        assert_eq!(line.chars().count(), 10, "{line}");
        assert!(line.starts_with('│'), "{line}");
        assert!(line.ends_with('│'), "{line}");
    }

    #[test]
    fn bottom_border_matches_width() {
        let line = bottom_border(12);
        assert_eq!(line.chars().count(), 12, "{line}");
        assert!(line.starts_with('╰'), "{line}");
        assert!(line.ends_with('╯'), "{line}");
    }
}
```

- [ ] **Step 2: Run the border-helper tests**

Run: `cargo test -p pi --lib output::logpane::tests::top_border_wraps_label_and_fills_width output::logpane::tests::side_line_pads_short_content_to_full_width output::logpane::tests::side_line_truncates_content_wider_than_the_box output::logpane::tests::bottom_border_matches_width`
Expected: PASS

- [ ] **Step 3: Write the failing tests for `LogPane` itself**

Add to the same `#[cfg(test)] mod tests` block in `crates/bin/src/output/logpane.rs`:

```rust
    #[test]
    fn non_interactive_pane_streams_immediately_and_keeps_full_history() {
        let mut pane = LogPane::new_non_interactive("test", 3);
        pane.push_line("one");
        pane.push_line("two");
        pane.push_line("three");
        pane.push_line("four");
        assert_eq!(pane.full, vec!["one", "two", "three", "four"]);
        assert!(pane.visible.is_empty(), "interactive-only buffer untouched");
        assert_eq!(pane.rendered, 0, "no live redraw happened");
    }

    #[test]
    fn push_line_sanitizes_before_storing() {
        let mut pane = LogPane::new_non_interactive("test", 3);
        pane.push_line("step 4/9\r\x1b[2K");
        assert_eq!(pane.full, vec!["step 4/9"]);
    }
```

- [ ] **Step 4: Run to verify these fail (struct doesn't exist yet)**

Run: `cargo test -p pi --lib output::logpane::tests::non_interactive_pane_streams_immediately_and_keeps_full_history`
Expected: FAIL with "cannot find type `LogPane`"

- [ ] **Step 5: Implement `LogPane`**

Add to `crates/bin/src/output/logpane.rs`, above the `#[cfg(test)]` block:

```rust
use pi_infrastructure::process::sanitize_line;

pub struct LogPane {
    term: Term,
    interactive: bool,
    label: String,
    max_visible: usize,
    visible: std::collections::VecDeque<String>,
    full: Vec<String>,
    rendered: usize,
}

impl LogPane {
    pub fn new(label: impl Into<String>, max_visible: usize) -> Self {
        let term = Term::stdout();
        let interactive = term.features().is_attended();
        Self {
            term,
            interactive,
            label: label.into(),
            max_visible,
            visible: std::collections::VecDeque::new(),
            full: Vec::new(),
            rendered: 0,
        }
    }

    #[cfg(test)]
    fn new_non_interactive(label: impl Into<String>, max_visible: usize) -> Self {
        let mut pane = Self::new(label, max_visible);
        pane.interactive = false;
        pane
    }

    pub fn push_line(&mut self, line: &str) {
        let clean = sanitize_line(line);
        self.full.push(clean.clone());
        if !self.interactive {
            println!("{clean}");
            return;
        }
        self.visible.push_back(clean);
        if self.visible.len() > self.max_visible {
            self.visible.pop_front();
        }
        self.redraw();
    }

    fn redraw(&mut self) {
        let (_, cols) = self.term.size();
        let width = (cols as usize).max(20);
        let _ = self.term.clear_last_lines(self.rendered);
        let _ = self.term.write_line(&top_border(&self.label, width));
        for l in &self.visible {
            let _ = self.term.write_line(&side_line(l, width));
        }
        let _ = self.term.write_line(&bottom_border(width));
        self.rendered = self.visible.len() + 2; // + top and bottom border
    }

    pub fn finish_ok(self, summary: &str) {
        if self.interactive {
            let _ = self.term.clear_last_lines(self.rendered);
        }
        crate::output::success(summary);
    }

    /// Neutral outcome (e.g. deploy superseded) — same as success: clear, no dump.
    pub fn finish_neutral(self, summary: &str) {
        if self.interactive {
            let _ = self.term.clear_last_lines(self.rendered);
        }
        crate::output::note(summary);
    }

    pub fn finish_err(self, summary: &str) {
        if self.interactive {
            let _ = self.term.clear_last_lines(self.rendered);
        }
        // Plain, unframed scrollback dump — a permanent historical record,
        // not a live widget, so it must include everything, not just the
        // last N lines that happened to still be visible.
        for l in &self.full {
            println!("{l}");
        }
        crate::output::error(summary);
    }
}
```

- [ ] **Step 6: Run all `LogPane` tests**

Run: `cargo test -p pi --lib output::logpane::`
Expected: PASS (all 6 tests: 4 border-helper + 2 `LogPane`)

- [ ] **Step 7: Register the submodule and re-export**

In `crates/bin/src/output/mod.rs`, add next to the `spinner` registration:

```rust
mod logpane;
pub use logpane::LogPane;
```

- [ ] **Step 8: Confirm the crate still compiles**

Run: `cargo check -p pi --locked`
Expected: success. As with Task 4, skip the strict `cargo clippy -D warnings` gate here — `LogPane` and its methods aren't called from a real command yet (that's Task 17), so `dead_code` will legitimately fire until then. Task 19 runs the real gate.

- [ ] **Step 9: Commit**

```bash
git add crates/bin/src/output/logpane.rs crates/bin/src/output/mod.rs
git commit -m "feat(cli): add output::LogPane bordered scrolling log pane"
```

---

## Task 8: Restructure `main()` for colored top-level error output

**Files:**
- Modify: `crates/bin/src/main.rs:232-360`

**Interfaces:**
- Consumes: `output::init_colors()`, `output::error()` (Task 4).

- [ ] **Step 1: Split `main()` into `run()` + a thin `main()`**

Replace `crates/bin/src/main.rs:232-234` (the function signature and first line):

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
```

with:

```rust
#[tokio::main]
async fn main() -> std::process::ExitCode {
    output::init_colors();
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            output::error(&err);
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
```

Everything from the original `let cli = Cli::parse();` line through the end of the big `match cli.cmd { ... }` block (ending at the closing `}` of the old `main`, i.e. line 360 in the original file) is unchanged — it now lives inside `run()` instead of `main()`. Do not alter any of that body.

- [ ] **Step 2: Verify existing tests still pass**

Run: `cargo test -p pi --lib main::`
Expected: PASS — all the `Cli::try_parse_from` tests are unaffected (they test `Cli` parsing directly, not `main`/`run`).

- [ ] **Step 3: Confirm the crate still compiles**

Run: `cargo check -p pi --locked`
Expected: success. `init_colors` and `error` are now used from `main`, but `success`/`warn`/`note`/`heading`/`styled_ok`/`styled_err`/`table`/`spinner`/`LogPane` still aren't wired into any real call site yet (Tasks 9–18), so keep skipping the strict `cargo clippy -D warnings` gate until Task 19.

- [ ] **Step 4: Manual verification**

Run: `cargo run -p pi -- doctor` (with no server profile configured)
Expected: the connection failure prints as a colored `✗ ...` line (via `output::error`) instead of Rust's default `Error: ...` debug dump. Run the same with `NO_COLOR=1 cargo run -p pi -- doctor` and confirm the `✗` icon still prints but with no ANSI color codes around it.

- [ ] **Step 5: Commit**

```bash
git add crates/bin/src/main.rs
git commit -m "feat(cli): route top-level errors through output::error instead of the default Debug print"
```

---

## Task 9: Colorize `render_doctor`

**Files:**
- Modify: `crates/bin/src/cli/commands.rs:474-490`

**Interfaces:**
- Consumes: `output::styled_ok/styled_err` (Task 4).

- [ ] **Step 1: Replace the plain `PASS`/`FAIL` marks with styled ones**

Replace `crates/bin/src/cli/commands.rs:474-490`:

```rust
pub(crate) fn render_doctor(checks: &[DiagnosticCheckDto]) -> (String, bool) {
    let mut out = String::new();
    let mut ok = true;
    for c in checks {
        let mark = if c.passed {
            "PASS"
        } else {
            ok = false;
            "FAIL"
        };
        out.push_str(&format!("{mark}  {} - {}\n", c.name, c.detail));
        if let (false, Some(hint)) = (c.passed, &c.hint) {
            out.push_str(&format!("      hint: {hint}\n"));
        }
    }
    (out, ok)
}
```

with:

```rust
pub(crate) fn render_doctor(checks: &[DiagnosticCheckDto]) -> (String, bool) {
    let mut out = String::new();
    let mut ok = true;
    for c in checks {
        let mark = if c.passed {
            output::styled_ok("PASS")
        } else {
            ok = false;
            output::styled_err("FAIL")
        };
        out.push_str(&format!("{mark}  {} - {}\n", c.name, c.detail));
        if let (false, Some(hint)) = (c.passed, &c.hint) {
            out.push_str(&format!("      hint: {hint}\n"));
        }
    }
    (out, ok)
}
```

- [ ] **Step 2: Add the `output` import**

In `crates/bin/src/cli/commands.rs:1-12`, find:

```rust
use crate::cli::api::ApiClient;
```

Add immediately above it:

```rust
use crate::output;
```

- [ ] **Step 3: Run the existing test to verify it still passes unmodified**

Run: `cargo test -p pi --lib cli::commands::tests::render_doctor_marks_failures_and_hints`
Expected: PASS — `styled_ok("PASS")`/`styled_err("FAIL")` render as plain `"PASS"`/`"FAIL"` text because the test process's stdout isn't a TTY, so `console` auto-disables styling; the substring assertions (`out.contains("PASS  docker daemon")` etc.) still match.

- [ ] **Step 4: Commit**

```bash
git add crates/bin/src/cli/commands.rs
git commit -m "feat(cli): colorize PASS/FAIL marks in rpi doctor output"
```

---

## Task 10: `commands.rs` — warn/note/error message migration

**Files:**
- Modify: `crates/bin/src/cli/commands.rs` (several functions, see steps)

**Interfaces:**
- Consumes: `output::warn/note/error` (Task 4).

Every call site below already reports a warning, a fallback note, or a failure — this task only changes how the message is printed (routed through the styled helper), never the wording, except `version_mismatch_warning`'s own leading `"warning: "` text, which is dropped because `output::warn` now supplies that signal visually (an unchanged leading `"warning: "` would just be redundant text next to the ⚠ icon).

- [ ] **Step 1: `version_mismatch_warning` — drop the redundant text prefix**

Replace `crates/bin/src/cli/commands.rs:683-691`:

```rust
/// §9.1: differing CLI/agent binary versions are a warning, not an error.
fn version_mismatch_warning(cli_version: &str, agent_version: &str) -> Option<String> {
    (cli_version != agent_version).then(|| {
        format!(
            "warning: CLI v{cli_version} and agent v{agent_version} differ - \
rebuild/update the agent on the Pi (`rpi agent update` ships in v0.5)"
        )
    })
}
```

with:

```rust
/// §9.1: differing CLI/agent binary versions are a warning, not an error.
fn version_mismatch_warning(cli_version: &str, agent_version: &str) -> Option<String> {
    (cli_version != agent_version).then(|| {
        format!(
            "CLI v{cli_version} and agent v{agent_version} differ - \
rebuild/update the agent on the Pi (`rpi agent update` ships in v0.5)"
        )
    })
}
```

- [ ] **Step 2: `deploy()` — route the version-mismatch warning through `output::warn`**

Replace `crates/bin/src/cli/commands.rs:24-26`:

```rust
    if let Some(warning) = version_mismatch_warning(env!("CARGO_PKG_VERSION"), &version.version) {
        eprintln!("{warning}");
    }
```

with:

```rust
    if let Some(warning) = version_mismatch_warning(env!("CARGO_PKG_VERSION"), &version.version) {
        output::warn(warning);
    }
```

- [ ] **Step 3: `deploy_cancel()` — the per-item cancel failure becomes an error**

Replace `crates/bin/src/cli/commands.rs:75-83`:

```rust
    for d in active {
        match api.cancel_deployment(&d.id).await {
            Ok(decision) => eprintln!("deployment {} ({}): {decision}", d.id, d.status),
            Err(err) => {
                failures += 1;
                eprintln!("deployment {} ({}): cancel failed: {err}", d.id, d.status);
            }
        }
    }
```

with:

```rust
    for d in active {
        match api.cancel_deployment(&d.id).await {
            Ok(decision) => eprintln!("deployment {} ({}): {decision}", d.id, d.status),
            Err(err) => {
                failures += 1;
                output::error(format!(
                    "deployment {} ({}): cancel failed: {err}",
                    d.id, d.status
                ));
            }
        }
    }
```

- [ ] **Step 4: `command()` — the "undeployed commands" note**

Replace `crates/bin/src/cli/commands.rs:385-390`:

```rust
        if !undeployed.is_empty() {
            eprintln!(
                "note: local rpi.toml declares undeployed command(s): {} - run `rpi deploy`",
                undeployed.join(", ")
            );
        }
```

with:

```rust
        if !undeployed.is_empty() {
            output::note(format!(
                "local rpi.toml declares undeployed command(s): {} - run `rpi deploy`",
                undeployed.join(", ")
            ));
        }
```

- [ ] **Step 5: `rm()` — the DNS-record note**

Replace `crates/bin/src/cli/commands.rs:440-443`:

```rust
    if let Some(hostname) = resp.hostname {
        eprintln!("note: the DNS record for {hostname} may still exist;");
        eprintln!("delete it manually: Cloudflare dashboard -> your zone -> DNS -> remove the {hostname} CNAME");
    }
```

with:

```rust
    if let Some(hostname) = resp.hostname {
        output::note(format!(
            "the DNS record for {hostname} may still exist; delete it manually: Cloudflare dashboard -> your zone -> DNS -> remove the {hostname} CNAME"
        ));
    }
```

- [ ] **Step 6: `agent_status()` — unreachable warning + fallback note**

Replace `crates/bin/src/cli/commands.rs:585-590`:

```rust
        Err(err) => {
            eprintln!("agent API unreachable ({err})");
            eprintln!(
                "falling back to: ssh {}@{} systemctl status rpi-agent",
                profile.user, profile.host
            );
            SshExec { profile: &profile }
```

with:

```rust
        Err(err) => {
            output::warn(format!("agent API unreachable ({err})"));
            output::note(format!(
                "falling back to: ssh {}@{} systemctl status rpi-agent",
                profile.user, profile.host
            ));
            SshExec { profile: &profile }
```

- [ ] **Step 7: `agent_logs()` — unreachable warning + fallback note**

Replace `crates/bin/src/cli/commands.rs:651-664`:

```rust
        Err(err) => {
            eprintln!("agent API unreachable ({err})");
            let since_unix = since
                .as_deref()
                .and_then(|s| parse_duration_secs(s).ok())
                .map(|secs| now - secs as i64);
            let args = journalctl_args(follow, since_unix, tail);
            let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
            eprintln!(
                "falling back to: ssh {}@{} {}",
                profile.user,
                profile.host,
                args.join(" ")
            );
            SshExec { profile: &profile }.run(&args_ref).await
        }
```

with:

```rust
        Err(err) => {
            output::warn(format!("agent API unreachable ({err})"));
            let since_unix = since
                .as_deref()
                .and_then(|s| parse_duration_secs(s).ok())
                .map(|secs| now - secs as i64);
            let args = journalctl_args(follow, since_unix, tail);
            let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
            output::note(format!(
                "falling back to: ssh {}@{} {}",
                profile.user,
                profile.host,
                args.join(" ")
            ));
            SshExec { profile: &profile }.run(&args_ref).await
        }
```

- [ ] **Step 8: Verify the existing `version_mismatch_warning` test still passes**

Run: `cargo test -p pi --lib cli::commands::tests::version_mismatch_produces_warning_only_on_difference`
Expected: PASS — the test only checks that both version numbers appear in the string, not the literal word `"warning:"`.

- [ ] **Step 9: Run the whole `commands` test module**

Run: `cargo test -p pi --lib cli::commands::`
Expected: PASS

- [ ] **Step 10: Commit**

```bash
git add crates/bin/src/cli/commands.rs
git commit -m "feat(cli): route warning/note/error messages in commands.rs through output helpers"
```

---

## Task 11: `commands.rs` — success-confirmation migration

**Files:**
- Modify: `crates/bin/src/cli/commands.rs` (`secrets_send`, `gc`, `lifecycle`, `rm`)

**Interfaces:**
- Consumes: `output::success` (Task 4).

- [ ] **Step 1: `secrets_send()`**

Replace `crates/bin/src/cli/commands.rs:102-107`:

```rust
    let (n, m) = (vars.len(), files.len());
    let resp = api.send_secrets(&project_name, vars, files, apply).await?;
    eprintln!("saved {n} key(s) and {m} file(s) for project '{project_name}'");
    if resp.applied {
        eprintln!("secrets applied to running containers");
    }
```

with:

```rust
    let (n, m) = (vars.len(), files.len());
    let resp = api.send_secrets(&project_name, vars, files, apply).await?;
    output::success(format!(
        "saved {n} key(s) and {m} file(s) for project '{project_name}'"
    ));
    if resp.applied {
        output::success("secrets applied to running containers");
    }
```

- [ ] **Step 2: `gc()`**

Replace `crates/bin/src/cli/commands.rs:194-199`:

```rust
    let resp = api.gc().await?;
    eprintln!(
        "gc done: disk {}% used; build cache pruned: {}",
        resp.disk_used_percent,
        if resp.builder_pruned { "yes" } else { "no" }
    );
```

with:

```rust
    let resp = api.gc().await?;
    output::success(format!(
        "gc done: disk {}% used; build cache pruned: {}",
        resp.disk_used_percent,
        if resp.builder_pruned { "yes" } else { "no" }
    ));
```

- [ ] **Step 3: `lifecycle()`**

Replace `crates/bin/src/cli/commands.rs:337-344`:

```rust
pub async fn lifecycle(project: String, action: &str, connect: ConnectOpts) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());
    api.lifecycle(&project, action).await?;
    eprintln!("{action} '{project}': done");
    Ok(())
}
```

with:

```rust
pub async fn lifecycle(project: String, action: &str, connect: ConnectOpts) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());
    api.lifecycle(&project, action).await?;
    output::success(format!("{action} '{project}': done"));
    Ok(())
}
```

- [ ] **Step 4: `rm()` — the removed-confirmation line (leave the confirmation prompt above it untouched)**

Replace `crates/bin/src/cli/commands.rs:430-439`:

```rust
    let resp = api.remove_project(&project, volumes).await?;
    eprintln!(
        "project '{}' removed{}",
        resp.project,
        if resp.volumes_removed {
            " (volumes included)"
        } else {
            " (volumes kept)"
        }
    );
```

with:

```rust
    let resp = api.remove_project(&project, volumes).await?;
    output::success(format!(
        "project '{}' removed{}",
        resp.project,
        if resp.volumes_removed {
            " (volumes included)"
        } else {
            " (volumes kept)"
        }
    ));
```

- [ ] **Step 5: Run the whole `commands` test module**

Run: `cargo test -p pi --lib cli::commands::`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/bin/src/cli/commands.rs
git commit -m "feat(cli): route success confirmations in commands.rs through output::success"
```

---

## Task 12: `init.rs`, `setup.rs`, `keys.rs` message-helper migration

**Files:**
- Modify: `crates/bin/src/cli/init.rs:220-223`
- Modify: `crates/bin/src/cli/setup.rs:99-108`
- Modify: `crates/bin/src/cli/keys.rs:103-109`

**Interfaces:**
- Consumes: `output::success/error/note` (Task 4).

- [ ] **Step 1: `init.rs` — the "wrote rpi.toml" confirmation**

Replace `crates/bin/src/cli/init.rs:221-224`:

```rust
    std::fs::write(&path, &text)?;
    println!("wrote {}", path.display());
    println!("next: `rpi secrets send` (if you use secrets), then `rpi deploy`");
    Ok(())
```

with:

```rust
    std::fs::write(&path, &text)?;
    crate::output::success(format!("wrote {}", path.display()));
    println!("next: `rpi secrets send` (if you use secrets), then `rpi deploy`");
    Ok(())
```

(The `"aborted: ..."` line at `init.rs:216` and the `"next: ..."` hint stay unchanged — neither is a warning/error/success confirmation.)

- [ ] **Step 2: `setup.rs` — profile saved (success) and connection test (failure)**

Replace `crates/bin/src/cli/setup.rs:99-107`:

```rust
    let path = ClientConfig::save_merged(&name, profile.clone(), make_default)?;
    println!("saved profile '{name}' to {}", path.display());

    // Connectivity test reuses the existing doctor path against the new profile.
    println!("testing connection...");
    if let Err(e) = ssh.check().await {
        println!("ssh check failed: {e}");
        println!("fix SSH access, then run `rpi doctor --server {name}`");
        return Ok(());
    }
```

with:

```rust
    let path = ClientConfig::save_merged(&name, profile.clone(), make_default)?;
    crate::output::success(format!("saved profile '{name}' to {}", path.display()));

    // Connectivity test reuses the existing doctor path against the new profile.
    println!("testing connection...");
    if let Err(e) = ssh.check().await {
        crate::output::error(format!("ssh check failed: {e}"));
        crate::output::note(format!("fix SSH access, then run `rpi doctor --server {name}`"));
        return Ok(());
    }
```

- [ ] **Step 3: `keys.rs` — the manual-copy failure instructions**

Replace `crates/bin/src/cli/keys.rs:103-109`:

```rust
    if failed {
        eprintln!("{}", manual_copy_instructions(&pubkey_text, profile));
        anyhow::bail!(
            "failed to copy public key to {} — see instructions above",
            profile.host
        );
    }
```

with:

```rust
    if failed {
        crate::output::error(manual_copy_instructions(&pubkey_text, profile));
        anyhow::bail!(
            "failed to copy public key to {} — see instructions above",
            profile.host
        );
    }
```

- [ ] **Step 4: Run the affected test modules**

Run: `cargo test -p pi --lib cli::init:: cli::setup:: cli::keys::`
Expected: PASS — none of these functions' printed text is asserted on in existing tests (`init.rs`/`setup.rs`/`keys.rs` tests exercise `render_rpi_toml`, `resolve_init_fields`, `resolve_profile`, `detect_ssh_keys`, `manual_copy_instructions`, etc. — pure logic, not the `run()`/`run_cmd()` printing wrappers).

- [ ] **Step 5: Commit**

```bash
git add crates/bin/src/cli/init.rs crates/bin/src/cli/setup.rs crates/bin/src/cli/keys.rs
git commit -m "feat(cli): route init/setup/keys messages through output helpers"
```

---

## Task 13: `agent/setup.rs` and `agent/uninstall.rs` report-print migration

**Files:**
- Modify: `crates/bin/src/agent/setup.rs:140-159`
- Modify: `crates/bin/src/agent/uninstall.rs:80-95`

**Interfaces:**
- Consumes: `output::success/warn/error/note` (Task 4).

- [ ] **Step 1: `SetupReport::print`**

Replace `crates/bin/src/agent/setup.rs:139-160`:

```rust
impl SetupReport {
    pub fn print(&self) {
        for c in &self.created {
            println!("created: {c}");
        }
        for r in &self.repaired {
            println!("repaired: {r}");
        }
        for s in &self.skipped {
            println!("ok (already present): {s}");
        }
        for w in &self.warnings {
            println!("warning: {w}");
        }
        for e in &self.errors {
            println!("error: {e}");
        }
        if self.repaired.iter().any(|r| r.contains("/var/log/rpi")) {
            println!("note: run `sudo systemctl restart rpi-agent` to activate file logs");
        }
    }
}
```

with:

```rust
impl SetupReport {
    pub fn print(&self) {
        for c in &self.created {
            crate::output::success(format!("created: {c}"));
        }
        for r in &self.repaired {
            crate::output::success(format!("repaired: {r}"));
        }
        for s in &self.skipped {
            crate::output::success(format!("ok (already present): {s}"));
        }
        for w in &self.warnings {
            crate::output::warn(w);
        }
        for e in &self.errors {
            crate::output::error(e);
        }
        if self.repaired.iter().any(|r| r.contains("/var/log/rpi")) {
            crate::output::note("run `sudo systemctl restart rpi-agent` to activate file logs");
        }
    }
}
```

- [ ] **Step 2: `agent::uninstall::run_cmd`'s report print**

Replace `crates/bin/src/agent/uninstall.rs:80-95`:

```rust
    let report = uninstall(&HostSys, &UninstallOpts { purge }).await;
    for r in &report.removed {
        println!("removed: {r}");
    }
    for k in &report.kept {
        println!("kept: {k}");
    }
    for w in &report.warnings {
        println!("warning: {w}");
    }
    for e in &report.errors {
        println!("error: {e}");
    }
    if !report.kept.is_empty() {
        println!("note: data kept; re-run with `--purge` to delete it");
    }
```

with:

```rust
    let report = uninstall(&HostSys, &UninstallOpts { purge }).await;
    for r in &report.removed {
        crate::output::success(format!("removed: {r}"));
    }
    for k in &report.kept {
        crate::output::note(format!("kept: {k}"));
    }
    for w in &report.warnings {
        crate::output::warn(w);
    }
    for e in &report.errors {
        crate::output::error(e);
    }
    if !report.kept.is_empty() {
        crate::output::note("data kept; re-run with `--purge` to delete it");
    }
```

- [ ] **Step 3: Run the affected test modules**

Run: `cargo test -p pi --lib agent::setup:: agent::uninstall::`
Expected: PASS — every existing test inspects `SetupReport`/`UninstallReport` fields directly (`report.created`, `report.errors`, etc.), never the printed output of `.print()`/`run_cmd()`.

- [ ] **Step 4: Commit**

```bash
git add crates/bin/src/agent/setup.rs crates/bin/src/agent/uninstall.rs
git commit -m "feat(cli): route agent setup/uninstall report printing through output helpers"
```

---

## Task 14: `rpi ls` table

**Files:**
- Modify: `crates/bin/src/cli/commands.rs:251-286`

**Interfaces:**
- Consumes: `output::table()` (Task 5).

- [ ] **Step 1: Replace the manual fixed-width columns with a table**

Replace `crates/bin/src/cli/commands.rs:251-286`:

```rust
pub async fn ls(connect: ConnectOpts) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let projects = api.projects().await?;
    if projects.is_empty() {
        println!("no projects deployed yet");
        return Ok(());
    }
    println!(
        "{:<16} {:<10} {:<28} {:<6} {:<28} SERVICES",
        "NAME", "BRANCH", "HOSTNAME", "PORT", "EXPOSE"
    );
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
        let expose = expose_cell(&p.expose, p.lan_ip.as_deref(), p.host_port);
        println!(
            "{:<16} {:<10} {:<28} {:<6} {:<28} {services}",
            p.name,
            p.branch,
            p.hostname.unwrap_or_else(|| "-".into()),
            p.host_port,
            expose
        );
    }
    Ok(())
}
```

with:

```rust
pub async fn ls(connect: ConnectOpts) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let projects = api.projects().await?;
    if projects.is_empty() {
        println!("no projects deployed yet");
        return Ok(());
    }
    let mut table = output::table();
    table.set_header(vec![
        "NAME", "BRANCH", "HOSTNAME", "PORT", "EXPOSE", "SERVICES",
    ]);
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
        let expose = expose_cell(&p.expose, p.lan_ip.as_deref(), p.host_port);
        table.add_row(vec![
            p.name,
            p.branch,
            p.hostname.unwrap_or_else(|| "-".into()),
            p.host_port.to_string(),
            expose,
            services,
        ]);
    }
    println!("{table}");
    Ok(())
}
```

- [ ] **Step 2: Run the `commands` test module (no direct test of `ls()`, but confirms the crate still compiles and its neighbors still pass)**

Run: `cargo test -p pi --lib cli::commands::`
Expected: PASS

- [ ] **Step 3: Confirm the crate still compiles**

Run: `cargo check -p pi --locked`
Expected: success. `table()` is now used, but `heading`/`spinner`/`LogPane` still aren't (Tasks 16/18/17), so keep skipping the strict `cargo clippy -D warnings` gate until Task 19.

- [ ] **Step 4: Commit**

```bash
git add crates/bin/src/cli/commands.rs
git commit -m "feat(cli): render rpi ls as a table"
```

---

## Task 15: `rpi status`/`rpi stats` tables

**Files:**
- Modify: `crates/bin/src/cli/commands.rs:304-335` (`stats`)
- Modify: `crates/bin/src/cli/commands.rs:460-472` (`print_agent_status`)

**Interfaces:**
- Consumes: `output::table()` (Task 5). The `if json { ...; return Ok(()); }` branches in both functions are untouched.

- [ ] **Step 1: `stats()` non-JSON branch**

Replace `crates/bin/src/cli/commands.rs:317-334`:

```rust
    println!(
        "host: cpu {:.1}%, mem {}/{} bytes, disk {}%, uptime {}",
        resp.host.cpu_percent,
        resp.host.mem_used_bytes,
        resp.host.mem_total_bytes,
        resp.host.disk_used_percent,
        human_duration(resp.host.uptime_secs)
    );
    for p in resp.projects {
        println!("project {}", p.project);
        for s in p.services {
            println!(
                "  {}: cpu {:.1}%, mem {}/{} bytes",
                s.service, s.cpu_percent, s.mem_used_bytes, s.mem_limit_bytes
            );
        }
    }
    Ok(())
}
```

with:

```rust
    let mut host_table = output::table();
    host_table.set_header(vec!["CPU", "MEM", "DISK", "UPTIME"]);
    host_table.add_row(vec![
        format!("{:.1}%", resp.host.cpu_percent),
        format!(
            "{}/{} bytes",
            resp.host.mem_used_bytes, resp.host.mem_total_bytes
        ),
        format!("{}%", resp.host.disk_used_percent),
        human_duration(resp.host.uptime_secs),
    ]);
    println!("{host_table}");

    if !resp.projects.is_empty() {
        let mut services_table = output::table();
        services_table.set_header(vec!["PROJECT", "SERVICE", "CPU", "MEM"]);
        for p in resp.projects {
            let project_name = p.project.clone();
            for s in p.services {
                services_table.add_row(vec![
                    project_name.clone(),
                    s.service,
                    format!("{:.1}%", s.cpu_percent),
                    format!("{}/{} bytes", s.mem_used_bytes, s.mem_limit_bytes),
                ]);
            }
        }
        println!("{services_table}");
    }
    Ok(())
}
```

- [ ] **Step 2: `print_agent_status` (used by both `status()` and `agent_status()`)**

Replace `crates/bin/src/cli/commands.rs:460-472`:

```rust
fn print_agent_status(resp: &crate::proto::AgentOverviewDto) {
    println!(
        "agent v{} (cli v{})",
        resp.version,
        env!("CARGO_PKG_VERSION")
    );
    println!("uptime: {}", human_duration(resp.uptime_secs));
    println!("disk: {}% used", resp.disk_used_percent);
    println!(
        "projects: {}, active deployments: {}",
        resp.projects, resp.active_deployments
    );
}
```

with:

```rust
fn print_agent_status(resp: &crate::proto::AgentOverviewDto) {
    let mut table = output::table();
    table.set_header(vec!["FIELD", "VALUE"]);
    table.add_row(vec![
        "agent".to_string(),
        format!("v{} (cli v{})", resp.version, env!("CARGO_PKG_VERSION")),
    ]);
    table.add_row(vec!["uptime".to_string(), human_duration(resp.uptime_secs)]);
    table.add_row(vec![
        "disk".to_string(),
        format!("{}% used", resp.disk_used_percent),
    ]);
    table.add_row(vec!["projects".to_string(), resp.projects.to_string()]);
    table.add_row(vec![
        "active deployments".to_string(),
        resp.active_deployments.to_string(),
    ]);
    println!("{table}");
}
```

- [ ] **Step 3: Run the `commands` test module**

Run: `cargo test -p pi --lib cli::commands::`
Expected: PASS

- [ ] **Step 4: Confirm the crate still compiles**

Run: `cargo check -p pi --locked`
Expected: success. `heading`/`spinner`/`LogPane` still aren't wired in yet (Tasks 16/18/17) — keep skipping the strict `cargo clippy -D warnings` gate until Task 19.

- [ ] **Step 5: Commit**

```bash
git add crates/bin/src/cli/commands.rs
git commit -m "feat(cli): render rpi status/stats non-JSON output as tables"
```

---

## Task 16: `rpi secrets ls` heading

**Files:**
- Modify: `crates/bin/src/cli/commands.rs:216-227`

**Interfaces:**
- Consumes: `output::heading` (Task 4).

- [ ] **Step 1: Swap the bare section labels for headings**

Replace `crates/bin/src/cli/commands.rs:216-227`:

```rust
    if !resp.keys.is_empty() {
        println!("env keys:");
        for key in &resp.keys {
            println!("  {key}");
        }
    }
    if !resp.files.is_empty() {
        println!("files:");
        for file in &resp.files {
            println!("  {file}");
        }
    }
```

with:

```rust
    if !resp.keys.is_empty() {
        output::heading("env keys:");
        for key in &resp.keys {
            println!("  {key}");
        }
    }
    if !resp.files.is_empty() {
        output::heading("files:");
        for file in &resp.files {
            println!("  {file}");
        }
    }
```

- [ ] **Step 2: Run the `commands` test module**

Run: `cargo test -p pi --lib cli::commands::`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/bin/src/cli/commands.rs
git commit -m "feat(cli): use output::heading for rpi secrets ls section labels"
```

---

## Task 17: Wire `LogPane` into `deploy()` and `command()`

**Files:**
- Modify: `crates/bin/src/cli/commands.rs:14-57` (`deploy`)
- Modify: `crates/bin/src/cli/commands.rs:354-404` (`command`)

**Interfaces:**
- Consumes: `output::LogPane` (Task 7).

This is the centerpiece feature the whole plan was requested for: a fixed-height bordered log pane that scrolls live during `rpi deploy`/`rpi command`, collapses on success, and dumps the complete captured log on failure.

- [ ] **Step 1: `deploy()`**

Replace `crates/bin/src/cli/commands.rs:45-56`:

```rust
    let status = api
        .follow_logs(&accepted.deployment_id, |line| println!("{line}"))
        .await?;
    eprintln!("deploy finished: {status}");
    if status == "superseded" {
        eprintln!("note: a newer deploy request replaced this one - not an error");
    }
    if status != "success" && status != "superseded" {
        drop(tunnel);
        std::process::exit(1);
    }
    Ok(())
}
```

with:

```rust
    let mut pane = output::LogPane::new(format!("deploy '{}'", rpitoml.project.name), 10);
    let status = api
        .follow_logs(&accepted.deployment_id, |line| pane.push_line(line))
        .await?;
    match status.as_str() {
        "success" => pane.finish_ok(&format!("deploy finished: {status}")),
        "superseded" => pane.finish_neutral(
            "deploy finished: superseded (a newer deploy request replaced this one - not an error)",
        ),
        _ => {
            pane.finish_err(&format!("deploy finished: {status}"));
            drop(tunnel);
            std::process::exit(1);
        }
    }
    Ok(())
}
```

- [ ] **Step 2: `command()`**

Replace `crates/bin/src/cli/commands.rs:394-404`:

```rust
    let code = api
        .run_command(&project_name, &name, &args, |line| println!("{line}"))
        .await?;
    if code != 0 {
        eprintln!("command '{name}' exited with code {code}");
        drop(tunnel);
        std::process::exit(code);
    }
    eprintln!("command '{name}' finished (exit 0)");
    Ok(())
}
```

with:

```rust
    let mut pane = output::LogPane::new(format!("command '{name}'"), 10);
    let code = api
        .run_command(&project_name, &name, &args, |line| pane.push_line(line))
        .await?;
    if code != 0 {
        pane.finish_err(&format!("command '{name}' exited with code {code}"));
        drop(tunnel);
        std::process::exit(code);
    }
    pane.finish_ok(&format!("command '{name}' finished (exit 0)"));
    Ok(())
}
```

- [ ] **Step 3: Run the `commands` test module**

Run: `cargo test -p pi --lib cli::commands::`
Expected: PASS

- [ ] **Step 4: Confirm the crate still compiles**

Run: `cargo check -p pi --locked`
Expected: success. `LogPane` is now used, but `spinner()` still isn't (Task 18) — keep skipping the strict `cargo clippy -D warnings` gate until Task 19.

- [ ] **Step 5: Manual verification (requires a configured server profile and agent)**

Run `rpi deploy` against a real Pi and observe: while the build/up log streams, a bordered box labeled `deploy '<project>'` shows the last 10 lines, scrolling as new ones arrive; on success the box disappears and a single `✓ deploy finished: success` line remains; if you can force a failing deploy (e.g. a broken Dockerfile), confirm the box disappears and the **entire** captured log prints to scrollback before the `✗ deploy finished: ...` line. Repeat for `rpi command <name>` with a declared `[commands]` entry.

- [ ] **Step 6: Commit**

```bash
git add crates/bin/src/cli/commands.rs
git commit -m "feat(cli): stream deploy/command output through a bordered scrolling log pane"
```

---

## Task 18: Spinner for SSH tunnel setup

**Files:**
- Modify: `crates/bin/src/cli/tunnel.rs:13-47`

**Interfaces:**
- Consumes: `output::spinner` (Task 6).

Wrapping the spinner in `SshTunnel::open` (rather than at every one of its ~15 call sites across `commands.rs`) means every command benefits automatically from a single change.

- [ ] **Step 1: Wrap the port-wait with a spinner**

Replace `crates/bin/src/cli/tunnel.rs:38-46`:

```rust
        let child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("cannot spawn ssh: {e}"))?;

        wait_port(port, Duration::from_secs(10)).await?;
        Ok(SshTunnel {
            child: Some(child),
            base_url: format!("http://127.0.0.1:{port}"),
        })
    }
```

with:

```rust
        let child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("cannot spawn ssh: {e}"))?;

        let pb = crate::output::spinner("connecting to agent...");
        let result = wait_port(port, Duration::from_secs(10)).await;
        pb.finish_and_clear();
        result?;

        Ok(SshTunnel {
            child: Some(child),
            base_url: format!("http://127.0.0.1:{port}"),
        })
    }
```

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p pi --all-targets --locked -- -D warnings`
Expected: no warnings

- [ ] **Step 3: Manual verification**

Run any command against a real (or deliberately unreachable, to see the failure path) Pi profile, e.g. `rpi ls`, and confirm a spinner labeled "connecting to agent..." appears briefly before the command's own output, and disappears cleanly whether the connection succeeds or times out.

- [ ] **Step 4: Commit**

```bash
git add crates/bin/src/cli/tunnel.rs
git commit -m "feat(cli): show a spinner while the SSH tunnel comes up"
```

---

## Task 19: Final verification

**Files:** none (verification only)

- [ ] **Step 1: Full formatting check**

Run: `cargo fmt --all -- --check`
Expected: no diff. If there is one, run `cargo fmt --all` and commit the result as its own commit (`style: cargo fmt`) before proceeding.

- [ ] **Step 2: Full clippy check**

Run: `cargo clippy --all-targets --locked -- -D warnings`
Expected: no warnings across the whole workspace.

- [ ] **Step 3: Full test suite**

Run: `cargo test --locked`
Expected: all tests pass across `pi-domain`, `pi-application`, `pi-infrastructure`, and `pi`.

- [ ] **Step 4: Manual smoke test of the styling itself**

Run: `cargo run -p pi -- --help` and confirm it still renders clap's own colored help normally (proves adding `console`/`indicatif`/`comfy-table` didn't interfere with clap's existing `anstream`-based coloring).

Run: `NO_COLOR=1 cargo run -p pi -- doctor` (no profile configured) and confirm the `✗`/`⚠` icons still print but with no ANSI escape codes visible around them (pipe the output through `cat -A` or redirect to a file and inspect it if your terminal would otherwise hide raw escape codes).

- [ ] **Step 5: Confirm no leftover debug artifacts**

Run: `git status`
Expected: clean working tree (every task already committed its own changes).
