use console::{style, Emoji};

mod table;
pub use table::table;

mod spinner;
pub use spinner::spinner;

mod logpane;
pub use logpane::LogPane;

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
    eprintln!("{} {msg}", style(Emoji("✓", "OK")).green().for_stderr());
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
