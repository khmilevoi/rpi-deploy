use console::{Emoji, Style};

mod table;
pub use table::{cell, cell_sem, header, table};

mod spinner;
pub use spinner::spinner;

mod logpane;
pub use logpane::LogPane;

mod theme;

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
    Frame,
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
        Sem::Frame => s.black().bright(),
    }
}

/// One stderr status line: a bold coloured marker + colour-tinted text.
fn stderr_line(sem: Sem, msg: &str) -> String {
    let base = console_style(sem).for_stderr();
    format!(
        "{} {}",
        base.clone().bold().apply_to(MARKER),
        base.apply_to(msg)
    )
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
}
