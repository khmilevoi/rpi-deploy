use console::Term;

fn top_border(label: &str, width: usize) -> String {
    // "╭─ " + " " + "╮" = 5 chars of fixed decoration around the label.
    let max_label_width = width.saturating_sub(5);
    let label: String = label.chars().take(max_label_width).collect();
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

use pi_infrastructure::process::sanitize_line;

pub struct LogPane {
    term: Term,
    interactive: bool,
    label: String,
    max_visible: usize,
    visible: std::collections::VecDeque<String>,
    full: Vec<String>,
    rendered: usize,
    print_line: Box<dyn Fn(&str)>,
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
            print_line: Box::new(|l: &str| println!("{l}")),
        }
    }

    #[cfg(test)]
    fn new_non_interactive(label: impl Into<String>, max_visible: usize) -> Self {
        let mut pane = Self::new(label, max_visible);
        pane.interactive = false;
        pane
    }

    #[cfg(test)]
    fn new_recording(
        label: impl Into<String>,
        max_visible: usize,
        interactive: bool,
        printed: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> Self {
        let mut pane = Self::new(label, max_visible);
        pane.interactive = interactive;
        pane.print_line = Box::new(move |l: &str| printed.lock().unwrap().push(l.to_string()));
        pane
    }

    pub fn push_line(&mut self, line: &str) {
        let clean = sanitize_line(line);
        self.full.push(clean.clone());
        if !self.interactive {
            (self.print_line)(&clean);
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
            // Plain, unframed scrollback dump — a permanent historical record,
            // not a live widget, so it must include everything, not just the
            // last N lines that happened to still be visible. In
            // non-interactive mode push_line already streamed every line live,
            // so dumping again here would duplicate the whole log.
            for l in &self.full {
                (self.print_line)(l);
            }
        }
        crate::output::error(summary);
    }
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
    fn top_border_truncates_a_label_wider_than_the_box() {
        let line = top_border("command 'run-full-integration-suite'", 40);
        assert_eq!(line.chars().count(), 40, "{line}");
        assert!(line.starts_with("╭─ "), "{line}");
        assert!(line.ends_with('╮'), "{line}");
    }

    #[test]
    fn bottom_border_matches_width() {
        let line = bottom_border(12);
        assert_eq!(line.chars().count(), 12, "{line}");
        assert!(line.starts_with('╰'), "{line}");
        assert!(line.ends_with('╯'), "{line}");
    }

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
        pane.push_line("\x1b[2K\rstep 4/9");
        assert_eq!(pane.full, vec!["step 4/9"]);
    }

    #[test]
    fn finish_err_does_not_reprint_lines_already_streamed_live() {
        let printed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut pane = LogPane::new_recording("test", 3, false, printed.clone());
        pane.push_line("one");
        pane.push_line("two");
        pane.finish_err("boom");
        assert_eq!(*printed.lock().unwrap(), vec!["one", "two"]);
    }

    #[test]
    fn finish_err_dumps_full_history_when_interactive() {
        let printed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut pane = LogPane::new_recording("test", 3, true, printed.clone());
        pane.push_line("one");
        pane.push_line("two");
        pane.finish_err("boom");
        assert_eq!(*printed.lock().unwrap(), vec!["one", "two"]);
    }
}
