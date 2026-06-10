use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// In-memory per-project локи (§8.1: живут только в памяти).
pub struct DeployLocks {
    inner: Mutex<HashSet<String>>,
}

impl DeployLocks {
    pub fn new() -> Arc<DeployLocks> {
        Arc::new(DeployLocks {
            inner: Mutex::new(HashSet::new()),
        })
    }

    /// None — деплой этого проекта уже идёт.
    pub fn try_acquire(self: &Arc<Self>, project: &str) -> Option<DeployPermit> {
        let mut held = self.inner.lock().ok()?;
        if !held.insert(project.to_string()) {
            return None;
        }
        Some(DeployPermit {
            locks: Arc::clone(self),
            project: project.to_string(),
        })
    }
}

/// RAII-пермит: освобождает лок при Drop (в т.ч. при панике деплой-таски).
pub struct DeployPermit {
    locks: Arc<DeployLocks>,
    project: String,
}

impl Drop for DeployPermit {
    fn drop(&mut self) {
        if let Ok(mut held) = self.locks.inner.lock() {
            held.remove(&self.project);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_for_same_project_fails_until_permit_dropped() {
        let locks = DeployLocks::new();
        let permit = locks.try_acquire("rateme");
        assert!(permit.is_some());
        assert!(
            locks.try_acquire("rateme").is_none(),
            "same project must be busy"
        );
        assert!(
            locks.try_acquire("other").is_some(),
            "other projects unaffected"
        );
        drop(permit);
        assert!(locks.try_acquire("rateme").is_some(), "released after drop");
    }
}
