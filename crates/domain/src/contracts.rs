use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
#[cfg(feature = "mocks")]
use mockall::automock;

use crate::entities::{
    ComposeStack, DeployRef, Deployment, DeploymentStatus, EnvBundle, FetchedSource, Project,
    ProjectConfig, ServiceState,
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
    /// Where this project's working copy lives on the agent host (used by
    /// `pi env send --apply` to re-inject .env without a fetch, §10).
    fn workdir(&self, project_name: &str) -> PathBuf;

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
    async fn get(&self, name: &str) -> Result<Option<Project>, DomainError>;
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

/// Store/retrieve the project EnvBundle, encrypted at rest (§6, §10).
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn save(&self, project: &str, bundle: &EnvBundle) -> Result<(), DomainError>;
    /// Empty bundle when nothing is stored for the project.
    async fn load(&self, project: &str) -> Result<EnvBundle, DomainError>;
}

/// Writes the decrypted bundle as `.env` into the project workdir (§10).
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait EnvFileWriter: Send + Sync {
    /// Fails with NotFound when the workdir does not exist (never deployed).
    async fn write(&self, workdir: &Path, bundle: &EnvBundle) -> Result<(), DomainError>;
}

/// Deploy gate (§8): hybrid docker healthcheck -> HTTP -> TCP.
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait HealthGate: Send + Sync {
    async fn check(
        &self,
        config: &ProjectConfig,
        host_port: u16,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError>;
}

/// Routes hostname -> 127.0.0.1:host_port on the edge (§6, §11).
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait Ingress: Send + Sync {
    async fn upsert(
        &self,
        hostname: &str,
        host_port: u16,
        log: Arc<dyn LogSink>,
    ) -> Result<(), DomainError>;
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
