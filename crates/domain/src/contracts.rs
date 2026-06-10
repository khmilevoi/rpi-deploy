use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
#[cfg(feature = "mocks")]
use mockall::automock;

use crate::entities::{
    ComposeStack, DeployRef, Deployment, DeploymentStatus, FetchedSource, Project, ProjectConfig,
    ServiceState,
};
use crate::error::DomainError;

/// Receiver for line-by-line deployment logs + terminal event.
/// Implementations: SSE hub of the agent, TailSink in application, stubs in tests.
pub trait LogSink: Send + Sync {
    fn line(&self, line: &str);
    fn finished(&self, status: DeploymentStatus);
}

/// Fetch code by DeployRef (§6). v1 adapter — GitSource.
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait Source: Send + Sync {
    async fn fetch(
        &self,
        project: &ProjectConfig,
        git_ref: &DeployRef,
        log: Arc<dyn LogSink>,
    ) -> Result<FetchedSource, DomainError>;
}

/// Abstraction of container backend (§6). v1 — DockerComposeRuntime.
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait ContainerRuntime: Send + Sync {
    async fn build(&self, stack: &ComposeStack, log: Arc<dyn LogSink>) -> Result<(), DomainError>;
    async fn up(&self, stack: &ComposeStack, log: Arc<dyn LogSink>) -> Result<(), DomainError>;
    async fn ps(&self, project_name: &str) -> Result<Vec<ServiceState>, DomainError>;
}

/// Project registry + port allocation (§6).
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait ProjectRepository: Send + Sync {
    /// Creates a project (with host port allocation) or updates the config,
    /// preserving the already-allocated host port.
    async fn upsert(&self, config: &ProjectConfig) -> Result<Project, DomainError>;
    async fn list(&self) -> Result<Vec<Project>, DomainError>;
}

/// Deployment history (§6).
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait DeploymentHistory: Send + Sync {
    async fn record_started(&self, deployment: &Deployment) -> Result<(), DomainError>;
    async fn record_finished<'a>(
        &self,
        id: &str,
        status: DeploymentStatus,
        commit_sha: Option<&'a str>,
        finished_at: i64,
        log_tail: &str,
    ) -> Result<(), DomainError>;
    async fn get(&self, id: &str) -> Result<Option<Deployment>, DomainError>;
}

/// Writes compose-override with mapping 127.0.0.1:<host> → <container> (§12.1).
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait OverrideStore: Send + Sync {
    async fn write(
        &self,
        project: &str,
        service: &str,
        host_port: u16,
        container_port: u16,
    ) -> Result<PathBuf, DomainError>;
}

/// Time determinism in tests (§6).
#[cfg_attr(feature = "mocks", automock)]
pub trait Clock: Send + Sync {
    fn now_unix(&self) -> i64;
}

/// Identifier determinism in tests (§6).
#[cfg_attr(feature = "mocks", automock)]
pub trait IdGen: Send + Sync {
    fn new_id(&self) -> String;
}
