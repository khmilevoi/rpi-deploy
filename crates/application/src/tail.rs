use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use pi_domain::contracts::LogSink;
use pi_domain::entities::DeploymentStatus;

/// Wrapper over LogSink: forwards events and keeps the last `cap` lines.
pub struct TailSink {
    inner: Arc<dyn LogSink>,
    lines: Mutex<VecDeque<String>>,
    cap: usize,
}

impl TailSink {
    pub fn new(inner: Arc<dyn LogSink>, cap: usize) -> Arc<TailSink> {
        Arc::new(TailSink {
            inner,
            lines: Mutex::new(VecDeque::new()),
            cap,
        })
    }

    pub fn tail(&self) -> String {
        let lines = match self.lines.lock() {
            Ok(l) => l,
            Err(_) => return String::new(),
        };
        lines.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}

impl LogSink for TailSink {
    fn line(&self, line: &str) {
        if let Ok(mut lines) = self.lines.lock() {
            if self.cap > 0 {
                if lines.len() == self.cap {
                    lines.pop_front();
                }
                lines.push_back(line.to_string());
            }
        }
        self.inner.line(line);
    }

    fn finished(&self, status: DeploymentStatus) {
        self.inner.finished(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CollectSink;
    use pi_domain::entities::DeploymentStatus;

    #[test]
    fn forwards_lines_and_finished_to_inner_sink() {
        let inner = CollectSink::new();
        let tail = TailSink::new(inner.clone(), 10);

        tail.line("a");
        tail.finished(DeploymentStatus::Success);

        assert_eq!(*inner.lines.lock().unwrap(), vec!["a".to_string()]);
        assert_eq!(
            *inner.finished.lock().unwrap(),
            vec![DeploymentStatus::Success]
        );
    }

    #[test]
    fn keeps_only_last_n_lines_in_tail() {
        let tail = TailSink::new(CollectSink::new(), 2);

        tail.line("1");
        tail.line("2");
        tail.line("3");

        assert_eq!(tail.tail(), "2\n3");
    }

    #[test]
    fn zero_capacity_keeps_empty_tail_but_still_forwards() {
        let inner = CollectSink::new();
        let tail = TailSink::new(inner.clone(), 0);

        tail.line("discarded");

        assert_eq!(tail.tail(), "");
        assert_eq!(*inner.lines.lock().unwrap(), vec!["discarded".to_string()]);
    }
}
