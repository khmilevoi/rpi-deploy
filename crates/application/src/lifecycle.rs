use std::sync::Arc;

use pi_domain::contracts::{ContainerRuntime, DeploymentHistory, LogSink, ProjectRepository};
use pi_domain::entities::LifecycleAction;
use pi_domain::error::DomainError;

pub struct ControlLifecycle {
    projects: Arc<dyn ProjectRepository>,
    history: Arc<dyn DeploymentHistory>,
    runtime: Arc<dyn ContainerRuntime>,
}

impl ControlLifecycle {
    pub fn new(
        projects: Arc<dyn ProjectRepository>,
        history: Arc<dyn DeploymentHistory>,
        runtime: Arc<dyn ContainerRuntime>,
    ) -> Arc<ControlLifecycle> {
        Arc::new(ControlLifecycle {
            projects,
            history,
            runtime,
        })
    }

    pub async fn execute(
        &self,
        project: &str,
        action: LifecycleAction,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError> {
        if self.projects.get(project).await?.is_none() {
            return Err(DomainError::NotFound(format!("project {project}")));
        }
        let active = self.history.active(project).await?;
        if !active.is_empty() {
            return Err(DomainError::Conflict(format!(
                "project {project} has active deployment"
            )));
        }
        self.runtime.lifecycle(project, action, log).await
    }
}
