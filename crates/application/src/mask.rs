use std::sync::{Arc, Mutex};

use pi_domain::contracts::LogSink;
use pi_domain::entities::{DeploymentStatus, EnvBundle};

/// Secret values shorter than this are not masked — filters out false
/// positives like `true`/`3000` (§8.1, §22).
pub const MASK_MIN_LEN: usize = 6;

/// LogSink wrapper replacing armed secret values with ***KEY*** (§8.1).
/// Created empty and armed once the bundle is decrypted mid-deploy: values
/// cannot leak before the process knows them.
pub struct MaskingSink {
    inner: Arc<dyn LogSink>,
    /// (mask, value), longest values first so nested secrets mask fully.
    secrets: Mutex<Vec<(String, String)>>,
}

impl MaskingSink {
    pub fn new(inner: Arc<dyn LogSink>) -> Arc<MaskingSink> {
        Arc::new(MaskingSink {
            inner,
            secrets: Mutex::new(Vec::new()),
        })
    }

    pub fn arm(&self, bundle: &EnvBundle) {
        let mut secrets: Vec<(String, String)> = bundle
            .vars
            .iter()
            .filter(|(_, value)| value.len() >= MASK_MIN_LEN)
            .map(|(key, value)| (format!("***{key}***"), value.clone()))
            .collect();
        secrets.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
        if let Ok(mut held) = self.secrets.lock() {
            *held = secrets;
        }
    }

    fn masked(&self, line: &str) -> String {
        let held = match self.secrets.lock() {
            Ok(held) => held,
            Err(_) => return line.to_string(),
        };
        let mut out = line.to_string();
        for (mask, value) in held.iter() {
            if out.contains(value.as_str()) {
                out = out.replace(value.as_str(), mask);
            }
        }
        out
    }
}

impl LogSink for MaskingSink {
    fn line(&self, line: &str) {
        self.inner.line(&self.masked(line));
    }

    fn finished(&self, status: DeploymentStatus) {
        self.inner.finished(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CollectSink;

    fn bundle(pairs: &[(&str, &str)]) -> EnvBundle {
        let mut b = EnvBundle::default();
        for (k, v) in pairs {
            b.vars.insert(k.to_string(), v.to_string());
        }
        b
    }

    #[test]
    fn masks_armed_values_and_keeps_short_ones() {
        let inner = CollectSink::new();
        let mask = MaskingSink::new(inner.clone());
        mask.arm(&bundle(&[
            ("DB_PASSWORD", "hunter2-long"),
            ("PORT", "3000"),
        ]));

        mask.line("connecting with hunter2-long to db on 3000");

        assert_eq!(
            *inner.lines.lock().unwrap(),
            vec!["connecting with ***DB_PASSWORD*** to db on 3000".to_string()]
        );
    }

    #[test]
    fn masks_every_occurrence_and_longest_value_first() {
        let inner = CollectSink::new();
        let mask = MaskingSink::new(inner.clone());
        mask.arm(&bundle(&[
            ("TOKEN", "abc123"),
            ("URL", "https://u:abc123@host"),
        ]));

        mask.line("https://u:abc123@host then abc123 again abc123");

        assert_eq!(
            *inner.lines.lock().unwrap(),
            vec!["***URL*** then ***TOKEN*** again ***TOKEN***".to_string()]
        );
    }

    #[test]
    fn passthrough_before_arm_and_finished_forwarded() {
        let inner = CollectSink::new();
        let mask = MaskingSink::new(inner.clone());
        mask.line("raw hunter2-long");
        mask.finished(DeploymentStatus::Success);
        assert_eq!(
            *inner.lines.lock().unwrap(),
            vec!["raw hunter2-long".to_string()]
        );
        assert_eq!(
            *inner.finished.lock().unwrap(),
            vec![DeploymentStatus::Success]
        );
    }
}
