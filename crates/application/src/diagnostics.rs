use std::sync::Arc;

use pi_domain::contracts::{DeploymentHistory, ProjectRepository, SystemProbe};
use pi_domain::entities::{AgentOverview, DiagnosticReport};
use pi_domain::error::DomainError;

pub struct RunDiagnostics {
    probe: Arc<dyn SystemProbe>,
}

impl RunDiagnostics {
    pub fn new(probe: Arc<dyn SystemProbe>) -> Arc<RunDiagnostics> {
        Arc::new(RunDiagnostics { probe })
    }

    pub async fn execute(&self) -> DiagnosticReport {
        self.probe.diagnostics().await
    }
}

pub struct AgentStatus {
    probe: Arc<dyn SystemProbe>,
    projects: Arc<dyn ProjectRepository>,
    history: Arc<dyn DeploymentHistory>,
}

impl AgentStatus {
    pub fn new(
        probe: Arc<dyn SystemProbe>,
        projects: Arc<dyn ProjectRepository>,
        history: Arc<dyn DeploymentHistory>,
    ) -> Arc<AgentStatus> {
        Arc::new(AgentStatus {
            probe,
            projects,
            history,
        })
    }

    pub async fn execute(&self) -> Result<AgentOverview, DomainError> {
        let mut overview = self.probe.overview().await?;
        let projects = self.projects.list().await?;
        overview.projects = projects.len();
        let mut active_deployments = 0;
        for project in projects {
            active_deployments += self.history.active(&project.config.name).await?.len();
        }
        overview.active_deployments = active_deployments;
        Ok(overview)
    }
}
