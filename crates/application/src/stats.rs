use std::sync::Arc;

use pi_domain::contracts::{DeploymentHistory, ProjectRepository, StatsProvider};
use pi_domain::entities::StatsReport;
use pi_domain::error::DomainError;

pub struct GetStats {
    projects: Arc<dyn ProjectRepository>,
    history: Arc<dyn DeploymentHistory>,
    stats: Arc<dyn StatsProvider>,
}

impl GetStats {
    pub fn new(
        projects: Arc<dyn ProjectRepository>,
        history: Arc<dyn DeploymentHistory>,
        stats: Arc<dyn StatsProvider>,
    ) -> Arc<GetStats> {
        Arc::new(GetStats {
            projects,
            history,
            stats,
        })
    }

    pub async fn execute(&self, project: Option<String>) -> Result<StatsReport, DomainError> {
        let names = match project {
            Some(name) => {
                if self.projects.get(&name).await?.is_none() {
                    return Err(DomainError::NotFound(format!("project {name}")));
                }
                vec![name]
            }
            None => self
                .projects
                .list()
                .await?
                .into_iter()
                .map(|p| p.config.name)
                .collect(),
        };

        let mut report = self.stats.report(names).await?;
        for p in &mut report.projects {
            p.last_deploy = self.history.latest(&p.project).await?;
        }
        Ok(report)
    }
}
