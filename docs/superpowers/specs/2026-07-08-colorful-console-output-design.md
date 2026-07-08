# Colorful, structured console output

Date: 2026-07-08

## Problem

Every user-facing `rpi` command prints plain, unstyled text via ad-hoc
`println!`/`eprintln!` calls, all inlined in `crates/bin/src/cli/commands.rs`
(~50 call sites) plus `init.rs`, `setup.rs`, `keys.rs`, `agent/setup.rs`,
`agent/uninstall.rs`, `agent/run.rs`. There is no shared formatting layer, no
color, no tables (`rpi ls` hand-pads `{:<16}` columns), no progress
indication, and no `NO_COLOR`/TTY awareness anywhere in the workspace.

Separately, and more importantly, streamed subprocess output (`rpi deploy`,
`rpi command`) has a real correctness bug, not just a cosmetic one:
`crates/infrastructure/src/process.rs::forward_lines` reads a child process's
stdout/stderr with tokio's `BufReader::lines()`, which only splits on `\n`.
`docker compose build` (`crates/infrastructure/src/docker.rs:203-208`) is
invoked without `--progress=plain` / `BUILDKIT_PROGRESS=plain`, so BuildKit
renders its interactive multi-line progress UI (bare `\r` plus ANSI
cursor-movement sequences like `\x1b[1A`, `\x1b[2K`) even though stdout is a
pipe, not a real terminal. Our reader buffers all of that as a single "line"
until a real `\n` eventually arrives, then that whole blob — cursor-movement
codes included — is forwarded through `LogSink::line`, over SSE, and printed
with one `println!("{line}")` on the CLI side. The embedded control
sequences move the terminal cursor mid-print, so the next thing printed
(another docker line, or an unrelated `eprintln!` status message) visually
lands on top of / glued to whatever was already there instead of starting a
fresh line.

## Goal

1. Give every `rpi` command a consistent, colorful, structured presentation:
   semantic message styling (success/error/warning/note/heading), real
   tables for list/status output, and icon+color status markers, all via one
   shared module.
2. Fix the log-glueing bug at its root (force plain docker progress output,
   sanitize stray control characters at the point we read subprocess output),
   not just paper over it with prettier printing.
3. For `rpi deploy` and `rpi command`, render the streamed subprocess log in
   a fixed-height scrolling pane; on success, collapse it away; on failure,
   dump the complete captured log so nothing needed for debugging is lost.
4. Respect `NO_COLOR` and non-TTY output (redirected/piped/CI) automatically,
   with no new CLI flags.

Non-goals:
- `rpi logs` and `rpi agent logs` are unaffected — their entire purpose is
  showing a live, complete stream, so they keep today's plain
  `println!(line)` behavior.
- `rpi stats --json` / `rpi status --json` are unaffected — untouched code
  path, still `serde_json::to_string_pretty`.
- No new `--no-color` / `--color` flag; no README rewrite (README only shows
  command invocations, no captured sample output, so nothing there goes
  stale).
- No change to `domain`/`application` crates beyond what's needed for the
  docker fix in `infrastructure` — they never print directly today and still
  won't.

## Dependencies

Add to `crates/bin/Cargo.toml` (pinned in workspace root `Cargo.toml`,
referenced as `{ workspace = true }` per existing convention):

- `console = "0.15"` — styled text (`console::style`), `Emoji` with automatic
  ASCII fallback, `Term` for cursor control, automatic TTY/`CLICOLOR`
  detection.
- `indicatif = "0.17"` — spinners for waits that have no streamed output of
  their own (SSH tunnel setup, health-check polling).
- `comfy-table = "7"` — tables for `ls`/`status`/`stats`.

## Module layout: `crates/bin/src/output/`

New top-level module (`mod output;` in `main.rs`, alongside `agent`, `cli`,
`duration`, `proto`), since it's shared by both `cli::commands` and
`agent::*`:

```
output/
  mod.rs      // init_colors(), success/error/warn/note/heading, Emoji icons
  table.rs    // fn table() -> comfy_table::Table (shared preset)
  spinner.rs  // fn spinner(msg) -> indicatif::ProgressBar (shared style)
  logpane.rs  // LogPane (scrolling window + full dump on error)
```

### `output::init_colors()`

Called once at the top of `main()`. `console` implements the clicolors spec
(`CLICOLOR`/`CLICOLOR_FORCE`) and TTY detection on its own, but does **not**
check `NO_COLOR` — so we do that explicitly, to stay consistent with clap's
own `--help` coloring (already `NO_COLOR`-aware transitively via `anstream`):

```rust
pub fn init_colors() {
    if std::env::var_os("NO_COLOR").is_some() {
        console::set_colors_enabled(false);
        console::set_colors_enabled_stderr(false);
    }
}
```

Everything else (auto-disable when piped/redirected) is `console`'s default
behavior and needs no extra code — this is also why existing tests keep
passing unmodified: captured test output isn't a TTY, so
`console::style(...)` renders as plain text, and substring assertions like
`out.contains("PASS  docker daemon")` still match.

### Semantic helpers (`output/mod.rs`)

Preserve the existing stdout/stderr split (data/success on stdout,
warnings/errors/notes on stderr) so piping `rpi ls > file` doesn't leak
diagnostics into the captured data:

```rust
use console::{style, Emoji};

pub fn success(msg: impl std::fmt::Display) { // stdout
    println!("{} {msg}", style(Emoji("✓", "OK")).green());
}
pub fn error(msg: impl std::fmt::Display) { // stderr
    eprintln!("{} {msg}", style(Emoji("✗", "ERR")).red().bold().for_stderr());
}
pub fn warn(msg: impl std::fmt::Display) { // stderr
    eprintln!("{} {msg}", style(Emoji("⚠", "!")).yellow().for_stderr());
}
pub fn note(msg: impl std::fmt::Display) { // stderr
    eprintln!("{}", style(format!("note: {msg}")).dim().for_stderr());
}
pub fn heading(msg: impl std::fmt::Display) { // stdout
    println!("{}", style(msg).bold());
}
```

The key constraint is `error`/`warn`/`note` call `.for_stderr()` on the
`StyledObject` so `NO_COLOR`/non-TTY detection is evaluated against stderr,
not stdout — the two can differ, e.g. `rpi ls 2>/dev/null`.

Call-site migration (no behavior change beyond styling):
- `commands.rs`: every `eprintln!("warning: ...")` → `output::warn(...)`,
  `"note: ..."` → `output::note(...)`, `anyhow::bail!`/failure eprintln's →
  `output::error(...)`.
- `agent/setup.rs::SetupReport::print`: `created`/`repaired`/`skipped` →
  `output::success`, `warnings` → `output::warn`, `errors` → `output::error`.
- `init.rs`, `setup.rs`, `keys.rs`, `agent/uninstall.rs`: same mechanical
  swap of literal `"warning:"/"error:"/"note:"` prefixes for the helpers.

### Tables (`output/table.rs`)

```rust
pub fn table() -> comfy_table::Table {
    let mut t = comfy_table::Table::new();
    t.load_preset(comfy_table::presets::UTF8_FULL)
        .apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS)
        .set_content_arrangement(comfy_table::ContentArrangement::Dynamic);
    t
}
```

- `rpi ls` (`commands.rs:261-283`): replace the manual `{:<16} {:<10} ...`
  `println!` with `output::table()` + `set_header([...])` +
  `add_row([...])` per project, columns unchanged (NAME/BRANCH/HOSTNAME/
  PORT/EXPOSE/SERVICES).
- `rpi status`/`rpi stats` non-JSON branch only (`commands.rs:317-333`,
  `:461-468`): host summary as one table, per-project/service rows as a
  second table, instead of nested `println!`. The `if json { ...; return
  Ok(()); }` early-return branches are untouched.
- `rpi secrets ls` (`commands.rs:203-229`): keep the two flat lists (env
  keys / files) but under `output::heading()` instead of a bare
  `println!("env keys:")`.
- `render_doctor` (`commands.rs:474-490`): keep the literal `"PASS"`/`"FAIL"`
  text (the existing test `render_doctor_marks_failures_and_hints` asserts
  on it) but wrap each in `output::success`/`output::error` styling so it's
  colored, not just a bare table.

### Top-level error rendering (`main.rs`)

Today, `#[tokio::main] async fn main() -> anyhow::Result<()>` relies on
Rust's default `Termination` impl for a top-level `Err`, which prints
`Error: {:?}` via `Debug` — plain, and bypasses our styling entirely. Split
into an inner `run()` with today's exact body, and a thin `main()`:

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

async fn run() -> anyhow::Result<()> { /* unchanged existing main() body */ }
```

## Fixing the log-glueing bug

### `crates/infrastructure/src/docker.rs`

In `compose()` (the single builder every compose invocation goes through),
force plain BuildKit output so it never emits cursor-redraw sequences,
regardless of whether stdout is a TTY:

```rust
fn compose(&self, stack: &ComposeStack, tail: &[&str]) -> Command {
    let mut cmd = Command::new("docker");
    cmd.args(compose_args(&stack.project_name, &file_chain(stack), tail));
    cmd.current_dir(&stack.workdir);
    cmd.env("BUILDKIT_PROGRESS", "plain");
    cmd
}
```

This covers `build`, `up` (which can trigger an implicit build), and any
other compose subcommand added later — it's a no-op for subcommands that
don't build anything.

### `crates/infrastructure/src/process.rs`

Defense in depth for anything else we stream (git, cloudflared, arbitrary
`rpi command` scripts) that might still emit `\r`/ANSI control sequences:
sanitize each captured line before handing it to `LogSink::line`:

```rust
fn sanitize_line(line: &str) -> String {
    // Strip bare CR and ANSI CSI sequences (ESC '[' ... final byte).
    // Implementation detail for the plan; must be allocation-light since
    // this runs per line for every streamed subprocess.
}

async fn forward_lines<R: AsyncRead + Unpin>(reader: R, log: Arc<dyn LogSink>) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        log.line(&sanitize_line(&line));
    }
}
```

## Scrolling log pane for `deploy` and `command`

Scope: `commands.rs::deploy()` (`:46`) and `commands.rs::command()` (`:395`)
only — both stream subprocess-derived lines *and* know success/failure only
at the end. `rpi logs` (`:299`) and `rpi agent logs` (`:647`) are explicitly
excluded: their entire purpose is showing the complete live stream, so they
keep plain `println!(line)`. `rpi gc` doesn't stream at all.

### `output/logpane.rs`

```rust
pub struct LogPane {
    term: console::Term,
    interactive: bool,      // detected once at construction
    max_visible: usize,     // 10, fixed for now — no CLI flag
    visible: std::collections::VecDeque<String>,
    full: Vec<String>,      // entire history, for the failure dump
    rendered: usize,        // lines currently drawn, for clear_last_lines
}

impl LogPane {
    pub fn new(max_visible: usize) -> Self {
        let term = console::Term::stdout();
        let interactive = term.features().is_attended();
        Self { term, interactive, max_visible, visible: Default::default(), full: Vec::new(), rendered: 0 }
    }

    pub fn push_line(&mut self, line: &str) {
        self.full.push(line.to_string());
        if !self.interactive {
            println!("{line}"); // non-TTY: unchanged plain streaming
            return;
        }
        self.visible.push_back(truncate_to_width(line, &self.term));
        if self.visible.len() > self.max_visible {
            self.visible.pop_front();
        }
        let _ = self.term.clear_last_lines(self.rendered);
        for l in &self.visible {
            let _ = self.term.write_line(&console::style(l).dim().to_string());
        }
        self.rendered = self.visible.len();
    }

    pub fn finish_ok(mut self, summary: &str) {
        if self.interactive { let _ = self.term.clear_last_lines(self.rendered); }
        output::success(summary);
    }

    /// Neutral outcome (e.g. deploy superseded) — same as success: clear, no dump.
    pub fn finish_neutral(mut self, summary: &str) {
        if self.interactive { let _ = self.term.clear_last_lines(self.rendered); }
        output::note(summary);
    }

    pub fn finish_err(mut self, summary: &str) {
        if self.interactive { let _ = self.term.clear_last_lines(self.rendered); }
        for l in &self.full { println!("{l}"); } // full history, not just the last N
        output::error(summary);
    }
}
```

Notes for the implementation plan:
- `max_visible = 10`, hardcoded, no new CLI flag (matches the answered
  clarifying question).
- Non-TTY fallback (`!interactive`, e.g. output redirected to a file, or no
  real terminal in CI) reverts to exactly today's behavior: full plain
  streaming, nothing collapsed, nothing hidden.
- Lines wider than the terminal are truncated for the *live* rendering only
  (`truncate_to_width`, using `term.size()`) so `clear_last_lines`'s
  line-count bookkeeping stays accurate even with wrapping-prone long
  docker output; the untruncated line still goes into `full` for the error
  dump.
- `push_line` also runs `sanitize_line` (shared with `process.rs`, or a
  second defensive pass) so a mismatched old-agent/new-CLI pair over SSE
  can't corrupt the pane.

### Call-site changes

`commands.rs::deploy()`:
```rust
let mut pane = output::LogPane::new(10);
let status = api.follow_logs(&accepted.deployment_id, |line| pane.push_line(&line)).await?;
match status.as_str() {
    "success" => pane.finish_ok(&format!("deploy finished: {status}")),
    "superseded" => pane.finish_neutral("deploy finished: superseded (a newer deploy request replaced this one)"),
    _ => {
        pane.finish_err(&format!("deploy finished: {status}"));
        drop(tunnel);
        std::process::exit(1);
    }
}
```

`commands.rs::command()`:
```rust
let mut pane = output::LogPane::new(10);
let code = api.run_command(&project_name, &name, &args, |line| pane.push_line(&line)).await?;
if code != 0 {
    pane.finish_err(&format!("command '{name}' exited with code {code}"));
    drop(tunnel);
    std::process::exit(code);
}
pane.finish_ok(&format!("command '{name}' finished (exit 0)"));
```

## Spinners for non-streamed waits (`output/spinner.rs`)

For waits that produce no line-by-line output of their own — SSH tunnel
setup (`SshTunnel::open`), health-check polling — a shared spinner style:

```rust
pub fn spinner(msg: impl Into<String>) -> indicatif::ProgressBar {
    let pb = indicatif::ProgressBar::new_spinner();
    pb.set_style(indicatif::ProgressStyle::with_template("{spinner} {msg}").unwrap());
    pb.set_message(msg.into());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}
```

Caller calls `.finish_with_message(...)` (styled ✓/✗) when the wait ends.
Non-TTY behavior is `indicatif`'s own default (it already no-ops cleanly
when not attached to a terminal), so no extra handling needed here.

## Testing

- Existing tests are unaffected: `render_doctor_marks_failures_and_hints`
  and any other string-matching test keep passing because `console` styling
  auto-disables on non-TTY captured output.
- `process.rs`: add a test for `sanitize_line` (strips bare `\r`, strips an
  ANSI CSI sequence, leaves plain text untouched) alongside the existing
  `run_streamed_forwards_output_lines` test.
- `docker.rs`: extend the existing `compose()`-related test(s) to assert
  `BUILDKIT_PROGRESS=plain` is set on the built `Command`.
- `output::logpane`: unit-test `LogPane` in non-interactive mode (force
  `interactive = false` via a constructor variant used only in tests) —
  assert `push_line` prints immediately and `full` accumulates everything;
  interactive/cursor-control behavior is not unit-testable and is covered by
  manual verification instead (see plan's verification section).
- Workspace: `cargo fmt --all -- --check`, `cargo clippy --all-targets
  --locked -- -D warnings`, `cargo test --locked` per `CLAUDE.md`.
