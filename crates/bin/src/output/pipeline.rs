use std::time::Duration;

use console::Emoji;

use super::{console_style, LogPane, Sem};

const MAX_VISIBLE: usize = 10;

static CHECK: Emoji<'_, '_> = Emoji("✓", "ok");
static CROSS: Emoji<'_, '_> = Emoji("✗", "x");
static DOT: Emoji<'_, '_> = Emoji("·", "-");
static MARKER: Emoji<'_, '_> = Emoji("▸", ">");

/// Deploy stream orchestrator (deploy-stages spec): starts as today's single
/// `deploy '<project>'` pane and, on the first stage event from the agent,
/// switches to pipeline mode — one collapsing pane per stage. Old agents never
/// send stage events, so legacy behaviour is preserved byte-for-byte.
pub struct Pipeline {
    pane: Option<LogPane>,
    /// Name of the currently open stage pane (None: legacy pane or between stages).
    current: Option<String>,
    staged_mode: bool,
    interactive: bool,
    services: Option<usize>,
    print_line: Box<dyn Fn(&str)>,
    #[cfg(test)]
    recording: Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>,
}

impl Pipeline {
    pub fn new(project: &str) -> Pipeline {
        let interactive = console::Term::stdout().features().is_attended();
        Pipeline {
            pane: Some(LogPane::new(format!("deploy '{project}'"), MAX_VISIBLE)),
            current: None,
            staged_mode: false,
            interactive,
            services: None,
            print_line: Box::new(|l: &str| println!("{l}")),
            #[cfg(test)]
            recording: None,
        }
    }

    #[cfg(test)]
    fn new_recording(
        project: &str,
        interactive: bool,
        printed: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> Pipeline {
        let sink = printed.clone();
        Pipeline {
            pane: Some(LogPane::new_recording(
                format!("deploy '{project}'"),
                MAX_VISIBLE,
                interactive,
                printed.clone(),
            )),
            current: None,
            staged_mode: false,
            interactive,
            services: None,
            print_line: Box::new(move |l: &str| sink.lock().unwrap().push(l.to_string())),
            #[cfg(test)]
            recording: Some(printed),
        }
    }

    fn open_pane(&self, label: &str) -> LogPane {
        #[cfg(test)]
        if let Some(printed) = &self.recording {
            return LogPane::new_recording(label, MAX_VISIBLE, self.interactive, printed.clone());
        }
        LogPane::new(label, MAX_VISIBLE)
    }

    pub fn push_line(&mut self, line: &str) {
        match &mut self.pane {
            Some(pane) => pane.push_line(line),
            None => (self.print_line)(line),
        }
    }

    pub fn summary(&mut self, services: usize) {
        self.services = Some(services);
    }

    pub fn services(&self) -> Option<usize> {
        self.services
    }

    fn elapsed_suffix(elapsed_ms: Option<u64>) -> String {
        elapsed_ms
            .map(|ms| {
                format!(
                    " ({})",
                    crate::duration::format_elapsed(Duration::from_millis(ms))
                )
            })
            .unwrap_or_default()
    }

    fn print_done(&self, stage: &str, status: &str, elapsed_ms: Option<u64>) {
        let elapsed = Self::elapsed_suffix(elapsed_ms);
        let line = if !self.interactive {
            format!("{MARKER} {stage} {status}{elapsed}")
        } else {
            match status {
                "ok" => format!(
                    "{} {stage}{}",
                    console_style(Sem::Success).apply_to(CHECK.to_string()),
                    console_style(Sem::Muted).apply_to(elapsed),
                ),
                "failed" => console_style(Sem::Error)
                    .apply_to(format!("{CROSS} {stage}{elapsed}"))
                    .to_string(),
                // skipped
                _ => console_style(Sem::Muted)
                    .apply_to(format!("{DOT} {stage} skipped{elapsed}"))
                    .to_string(),
            }
        };
        (self.print_line)(&line);
    }

    pub fn stage(&mut self, stage: &str, status: &str, elapsed_ms: Option<u64>) {
        match status {
            "started" => {
                // First stage event: silently collapse the legacy pane and
                // enter pipeline mode. Also collapses a stage pane whose
                // completion never arrived (defensive).
                self.staged_mode = true;
                if let Some(pane) = self.pane.take() {
                    pane.clear();
                }
                self.pane = Some(self.open_pane(stage));
                self.current = Some(stage.to_string());
            }
            "ok" | "skipped" => {
                self.staged_mode = true;
                if let Some(pane) = self.pane.take() {
                    pane.clear();
                }
                self.current = None;
                self.print_done(stage, status, elapsed_ms);
            }
            "failed" => {
                self.staged_mode = true;
                if let Some(pane) = self.pane.take() {
                    pane.abort(&format!("{stage} log"));
                }
                self.current = None;
                self.print_done(stage, status, elapsed_ms);
            }
            _ => {} // unknown status: forward compatibility, ignore
        }
    }

    pub fn finish_ok(self, stamp: &str) {
        match (self.staged_mode, self.pane) {
            (false, Some(pane)) => pane.finish_ok(stamp),
            (true, pane) => {
                if let Some(p) = pane {
                    p.clear();
                }
                crate::output::success(stamp);
            }
            (false, None) => crate::output::success(stamp),
        }
    }

    pub fn finish_neutral(self, stamp: &str) {
        match (self.staged_mode, self.pane) {
            (false, Some(pane)) => pane.finish_neutral(stamp),
            (true, pane) => {
                if let Some(p) = pane {
                    p.clear();
                }
                crate::output::note(stamp);
            }
            (false, None) => crate::output::note(stamp),
        }
    }

    pub fn finish_err(self, stamp: &str) {
        match (self.staged_mode, self.pane, self.current) {
            (false, Some(pane), _) => pane.finish_err(stamp),
            (true, Some(pane), current) => {
                let label = current.map(|s| format!("{s} log")).unwrap_or("log".into());
                pane.abort(&label);
                crate::output::error(stamp);
            }
            (_, None, _) => crate::output::error(stamp),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn recording(interactive: bool) -> (Pipeline, Arc<Mutex<Vec<String>>>) {
        let printed = Arc::new(Mutex::new(Vec::new()));
        (
            Pipeline::new_recording("myboard", interactive, printed.clone()),
            printed,
        )
    }

    #[test]
    fn legacy_stream_without_stage_events_streams_lines_as_today() {
        let (mut p, printed) = recording(false);
        p.push_line("project 'myboard': host port 8000");
        p.push_line("fetched abc");
        assert_eq!(
            *printed.lock().unwrap(),
            vec!["project 'myboard': host port 8000", "fetched abc"]
        );
    }

    #[test]
    fn interactive_stage_ok_collapses_to_a_summary_line() {
        let (mut p, printed) = recording(true);
        p.stage("build", "started", None);
        p.push_line("step 1/9");
        p.stage("build", "ok", Some(48_300));
        // Interactive pane lines are drawn on the terminal, not print_line;
        // only the collapse summary goes through print_line.
        assert_eq!(*printed.lock().unwrap(), vec!["✓ build (48.3s)"]);
    }

    #[test]
    fn failed_stage_dumps_only_its_own_lines() {
        let (mut p, printed) = recording(true);
        p.stage("fetch", "started", None);
        p.push_line("cloning");
        p.stage("fetch", "ok", Some(2_100));
        p.stage("build", "started", None);
        p.push_line("compile error");
        p.stage("build", "failed", Some(12_100));
        assert_eq!(
            *printed.lock().unwrap(),
            vec![
                "✓ fetch (2.1s)",
                "— build log —",
                "compile error",
                "✗ build (12.1s)"
            ]
        );
    }

    #[test]
    fn skipped_stage_prints_a_dim_note() {
        let (mut p, printed) = recording(true);
        p.stage("route", "started", None);
        p.stage("route", "skipped", Some(400));
        assert_eq!(*printed.lock().unwrap(), vec!["· route skipped (0.4s)"]);
    }

    #[test]
    fn non_interactive_prints_boundary_lines_on_completion_only() {
        let (mut p, printed) = recording(false);
        p.stage("build", "started", None);
        p.push_line("step 1/9");
        p.stage("build", "ok", Some(48_300));
        assert_eq!(
            *printed.lock().unwrap(),
            vec!["step 1/9", "▸ build ok (48.3s)"]
        );
    }

    #[test]
    fn lines_between_stages_print_plain() {
        let (mut p, printed) = recording(true);
        p.stage("fetch", "started", None);
        p.stage("fetch", "ok", Some(2_100));
        p.push_line("secrets injected (2 keys, 0 files)");
        assert_eq!(
            *printed.lock().unwrap(),
            vec!["✓ fetch (2.1s)", "secrets injected (2 keys, 0 files)"]
        );
    }

    #[test]
    fn unknown_status_is_ignored() {
        let (mut p, printed) = recording(true);
        p.stage("build", "paused", Some(10));
        assert!(printed.lock().unwrap().is_empty());
    }

    #[test]
    fn summary_is_stored_for_the_stamp() {
        let (mut p, _) = recording(true);
        assert_eq!(p.services(), None);
        p.summary(2);
        assert_eq!(p.services(), Some(2));
    }
}
