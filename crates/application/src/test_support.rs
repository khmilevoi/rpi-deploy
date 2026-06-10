use std::sync::{Arc, Mutex};

use pi_domain::contracts::LogSink;
use pi_domain::entities::DeploymentStatus;

/// Тестовый sink: собирает всё, что в него пишут.
pub struct CollectSink {
    pub lines: Mutex<Vec<String>>,
    pub finished: Mutex<Vec<DeploymentStatus>>,
}

impl CollectSink {
    pub fn new() -> Arc<CollectSink> {
        Arc::new(CollectSink {
            lines: Mutex::new(vec![]),
            finished: Mutex::new(vec![]),
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
}
