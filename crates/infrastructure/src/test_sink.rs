//! Shared `LogSink` test double for this crate's unit tests.

use std::sync::{Arc, Mutex};

use pi_domain::contracts::LogSink;
use pi_domain::entities::DeploymentStatus;

pub(crate) struct CollectSink(pub(crate) Mutex<Vec<String>>);

impl CollectSink {
    pub(crate) fn new() -> Arc<CollectSink> {
        Arc::new(CollectSink(Mutex::new(vec![])))
    }
}

impl LogSink for CollectSink {
    fn line(&self, line: &str) {
        self.0.lock().unwrap().push(line.to_string());
    }
    fn finished(&self, _status: DeploymentStatus) {}
}
