use std::sync::Arc;

use pi_domain::contracts::{ContainerRuntime, DiskProbe, LogSink};
use pi_domain::error::DomainError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcReport {
    pub disk_used_percent: u8,
    pub builder_pruned: bool,
}

/// Disk GC (§8.1): dangling images always; build cache only above the disk
/// threshold. Runs as a post-success deploy stage and behind `pi gc`.
pub struct RunGc {
    runtime: Arc<dyn ContainerRuntime>,
    disk: Arc<dyn DiskProbe>,
    disk_threshold_percent: u8,
}

impl RunGc {
    pub fn new(
        runtime: Arc<dyn ContainerRuntime>,
        disk: Arc<dyn DiskProbe>,
        disk_threshold_percent: u8,
    ) -> Arc<RunGc> {
        Arc::new(RunGc {
            runtime,
            disk,
            disk_threshold_percent,
        })
    }

    pub async fn execute(&self, log: Arc<dyn LogSink>) -> Result<GcReport, DomainError> {
        self.runtime.prune_images(Arc::clone(&log)).await?;
        let used = self.disk.used_percent()?;
        let builder_pruned = used >= self.disk_threshold_percent;
        if builder_pruned {
            log.line(&format!(
                "disk {used}% >= {}% threshold - pruning build cache",
                self.disk_threshold_percent
            ));
            self.runtime.prune_builder(Arc::clone(&log)).await?;
        } else {
            log.line(&format!(
                "disk {used}% < {}% threshold - keeping build cache",
                self.disk_threshold_percent
            ));
        }
        Ok(GcReport {
            disk_used_percent: used,
            builder_pruned,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CollectSink;
    use pi_domain::contracts::{MockContainerRuntime, MockDiskProbe};

    #[tokio::test]
    async fn below_threshold_prunes_images_only() {
        let mut runtime = MockContainerRuntime::new();
        runtime
            .expect_prune_images()
            .times(1)
            .returning(|_| Ok(()));
        runtime.expect_prune_builder().times(0);
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(60));

        let report = RunGc::new(Arc::new(runtime), Arc::new(disk), 85)
            .execute(CollectSink::new())
            .await
            .unwrap();
        assert_eq!(
            report,
            GcReport {
                disk_used_percent: 60,
                builder_pruned: false
            }
        );
    }

    #[tokio::test]
    async fn at_or_above_threshold_also_prunes_builder() {
        let mut runtime = MockContainerRuntime::new();
        runtime
            .expect_prune_images()
            .times(1)
            .returning(|_| Ok(()));
        runtime
            .expect_prune_builder()
            .times(1)
            .returning(|_| Ok(()));
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(85));

        let sink = CollectSink::new();
        let report = RunGc::new(Arc::new(runtime), Arc::new(disk), 85)
            .execute(sink.clone())
            .await
            .unwrap();
        assert!(report.builder_pruned);
        let lines = sink.lines.lock().unwrap();
        assert!(
            lines.iter().any(|l| l.contains("85%")),
            "threshold decision must be logged: {lines:?}"
        );
    }

    #[tokio::test]
    async fn prune_images_error_propagates() {
        let mut runtime = MockContainerRuntime::new();
        runtime
            .expect_prune_images()
            .returning(|_| Err(DomainError::Runtime("docker down".into())));
        runtime.expect_prune_builder().times(0);
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().times(0);

        let err = RunGc::new(Arc::new(runtime), Arc::new(disk), 85)
            .execute(CollectSink::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Runtime(_)));
    }
}
