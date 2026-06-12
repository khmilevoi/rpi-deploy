use std::sync::Arc;

use pi_domain::contracts::{
    ContainerRuntime, DeploymentHistory, Ingress, LogSink, OverrideStore, ProjectRepository,
    SecretStore, Source,
};
use pi_domain::error::DomainError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveReport {
    pub project: String,
    pub hostname: Option<String>,
    pub volumes_removed: bool,
}

pub struct RemoveProject {
    projects: Arc<dyn ProjectRepository>,
    history: Arc<dyn DeploymentHistory>,
    runtime: Arc<dyn ContainerRuntime>,
    ingress: Arc<dyn Ingress>,
    source: Arc<dyn Source>,
    secrets: Arc<dyn SecretStore>,
    overrides: Arc<dyn OverrideStore>,
}

impl RemoveProject {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        projects: Arc<dyn ProjectRepository>,
        history: Arc<dyn DeploymentHistory>,
        runtime: Arc<dyn ContainerRuntime>,
        ingress: Arc<dyn Ingress>,
        source: Arc<dyn Source>,
        secrets: Arc<dyn SecretStore>,
        overrides: Arc<dyn OverrideStore>,
    ) -> Arc<RemoveProject> {
        Arc::new(RemoveProject {
            projects,
            history,
            runtime,
            ingress,
            source,
            secrets,
            overrides,
        })
    }

    pub async fn execute(
        &self,
        project: &str,
        remove_volumes: bool,
        log: Arc<dyn LogSink>,
    ) -> Result<RemoveReport, DomainError> {
        let existing = self
            .projects
            .get(project)
            .await?
            .ok_or_else(|| DomainError::NotFound(format!("project {project}")))?;
        let active = self.history.active(project).await?;
        if !active.is_empty() {
            return Err(DomainError::Conflict(format!(
                "project {project} has active deployment"
            )));
        }

        self.runtime
            .down(project, remove_volumes, Arc::clone(&log))
            .await?;
        if let Some(hostname) = &existing.config.hostname {
            self.ingress.remove(hostname, Arc::clone(&log)).await?;
        }
        self.source.cleanup(project).await?;
        self.secrets.remove(project).await?;
        self.overrides.remove(project).await?;
        self.history.remove_project(project).await?;
        self.projects.remove(project).await?;

        Ok(RemoveReport {
            project: project.to_string(),
            hostname: existing.config.hostname,
            volumes_removed: remove_volumes,
        })
    }
}
