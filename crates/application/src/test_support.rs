use std::sync::{Arc, Mutex};

use pi_domain::contracts::LogSink;
use pi_domain::entities::{DeploymentStatus, StageEvent};

/// Test sink: collects everything written to it.
pub struct CollectSink {
    pub lines: Mutex<Vec<String>>,
    pub finished: Mutex<Vec<DeploymentStatus>>,
    pub stages: Mutex<Vec<StageEvent>>,
    pub summaries: Mutex<Vec<usize>>,
}

impl CollectSink {
    pub fn new() -> Arc<CollectSink> {
        Arc::new(CollectSink {
            lines: Mutex::new(vec![]),
            finished: Mutex::new(vec![]),
            stages: Mutex::new(vec![]),
            summaries: Mutex::new(vec![]),
        })
    }
}

impl LogSink for CollectSink {
    fn line(&self, line: &str) {
        self.lines.lock().unwrap().push(line.to_string());
    }

    fn finished(&self, status: DeploymentStatus) {
        self.finished.lock().unwrap().push(status);
    }

    fn stage(&self, ev: &StageEvent) {
        self.stages.lock().unwrap().push(ev.clone());
    }

    fn summary(&self, services: usize) {
        self.summaries.lock().unwrap().push(services);
    }
}
