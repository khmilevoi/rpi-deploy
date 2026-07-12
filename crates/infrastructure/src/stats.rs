use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::{ContainerRuntime, DiskProbe, HostMetricsStore, StatsProvider};
use pi_domain::entities::{HostStats, ProjectStats, StatsReport};
use pi_domain::error::DomainError;
use sysinfo::System;

pub struct CompositeStats {
    runtime: Arc<dyn ContainerRuntime>,
    disk: Arc<dyn DiskProbe>,
    metrics: Arc<dyn HostMetricsStore>,
}

impl CompositeStats {
    pub fn new(
        runtime: Arc<dyn ContainerRuntime>,
        disk: Arc<dyn DiskProbe>,
        metrics: Arc<dyn HostMetricsStore>,
    ) -> Arc<CompositeStats> {
        Arc::new(CompositeStats {
            runtime,
            disk,
            metrics,
        })
    }
}

#[async_trait]
impl StatsProvider for CompositeStats {
    async fn report(&self, projects: Vec<String>) -> Result<StatsReport, DomainError> {
        let host = match self.metrics.latest() {
            Some(latest) => HostStats {
                cpu_percent: latest.cpu_percent,
                mem_used_bytes: latest.mem_used_bytes,
                mem_total_bytes: latest.mem_total_bytes,
                disk_used_percent: self.disk.used_percent().unwrap_or(0),
                uptime_secs: System::uptime(),
                temp_celsius: latest.temp_celsius,
            },
            // Defensive: sampler pre-seeds one sample, so this is unexpected.
            None => {
                let mut sys = System::new();
                sys.refresh_memory();
                HostStats {
                    cpu_percent: 0.0,
                    mem_used_bytes: sys.used_memory(),
                    mem_total_bytes: sys.total_memory(),
                    disk_used_percent: self.disk.used_percent().unwrap_or(0),
                    uptime_secs: System::uptime(),
                    temp_celsius: None,
                }
            }
        };

        let mut project_stats = Vec::new();
        for project in projects {
            let services = self.runtime.stats(&project).await.unwrap_or_default();
            project_stats.push(ProjectStats {
                project,
                services,
                last_deploy: None,
            });
        }

        Ok(StatsReport {
            host,
            projects: project_stats,
            host_history: self.metrics.history(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_domain::contracts::{MockContainerRuntime, MockDiskProbe, MockHostMetricsStore};
    use pi_domain::entities::{HostSample, ServiceStats};

    fn sample() -> HostSample {
        HostSample {
            at_ms: 1_000,
            cpu_percent: 12.5,
            mem_used_bytes: 2048,
            mem_total_bytes: 8192,
            temp_celsius: Some(47.0),
        }
    }

    #[tokio::test]
    async fn host_is_assembled_from_latest_sample_plus_disk_and_uptime() {
        let mut runtime = MockContainerRuntime::new();
        runtime.expect_stats().returning(|_| Ok(vec![]));
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(37));
        let mut metrics = MockHostMetricsStore::new();
        metrics.expect_latest().returning(|| Some(sample()));
        metrics
            .expect_history()
            .returning(|| vec![sample(), sample()]);

        let stats = CompositeStats::new(Arc::new(runtime), Arc::new(disk), Arc::new(metrics));
        let report = stats.report(vec![]).await.unwrap();

        assert_eq!(report.host.cpu_percent, 12.5);
        assert_eq!(report.host.mem_used_bytes, 2048);
        assert_eq!(report.host.disk_used_percent, 37);
        assert_eq!(report.host.temp_celsius, Some(47.0));
        assert!(report.host.uptime_secs > 0);
        assert_eq!(report.host_history.len(), 2);
    }

    #[tokio::test]
    async fn per_service_zero_mem_limit_is_preserved() {
        let mut runtime = MockContainerRuntime::new();
        runtime.expect_stats().returning(|_| {
            Ok(vec![ServiceStats {
                service: "valkey".into(),
                cpu_percent: 0.2,
                mem_used_bytes: 0,
                mem_limit_bytes: 0,
            }])
        });
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(0));
        let mut metrics = MockHostMetricsStore::new();
        metrics.expect_latest().returning(|| Some(sample()));
        metrics.expect_history().returning(Vec::new);

        let stats = CompositeStats::new(Arc::new(runtime), Arc::new(disk), Arc::new(metrics));
        let report = stats.report(vec!["p".into()]).await.unwrap();

        let svc = &report.projects[0].services[0];
        assert_eq!(svc.mem_limit_bytes, 0);
        assert_eq!(svc.mem_used_bytes, 0);
    }
}
