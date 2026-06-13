use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::{ContainerRuntime, DiskProbe, StatsProvider};
use pi_domain::entities::{HostStats, ProjectStats, StatsReport};
use pi_domain::error::DomainError;
use sysinfo::System;

pub struct CompositeStats {
    runtime: Arc<dyn ContainerRuntime>,
    disk: Arc<dyn DiskProbe>,
}

impl CompositeStats {
    pub fn new(
        runtime: Arc<dyn ContainerRuntime>,
        disk: Arc<dyn DiskProbe>,
    ) -> Arc<CompositeStats> {
        Arc::new(CompositeStats { runtime, disk })
    }
}

#[async_trait]
impl StatsProvider for CompositeStats {
    async fn report(&self, projects: Vec<String>) -> Result<StatsReport, DomainError> {
        let mut sys = System::new_all();
        sys.refresh_all();
        let host = HostStats {
            cpu_percent: f64::from(sys.global_cpu_usage()),
            mem_used_bytes: sys.used_memory(),
            mem_total_bytes: sys.total_memory(),
            disk_used_percent: self.disk.used_percent().unwrap_or(0),
            uptime_secs: System::uptime(),
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
        })
    }
}
