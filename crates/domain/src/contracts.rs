use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
#[cfg(feature = "mocks")]
use mockall::automock;

use crate::entities::{
    ComposeStack, Deployment, DeploymentStatus, DeployRef, FetchedSource, Project, ProjectConfig,
    ServiceState,
};
use crate::error::DomainError;

/// Приёмник построчных логов деплоя + терминального события.
/// Реализации: SSE-хаб агента, TailSink в application, заглушки в тестах.
pub trait LogSink: Send + Sync {
    fn line(&self, line: &str);
    fn finished(&self, status: DeploymentStatus);
}

/// Получить код по DeployRef (§6). v1-адаптер — GitSource.
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

/// Абстракция контейнерного бэкенда (§6). v1 — DockerComposeRuntime.
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait ContainerRuntime: Send + Sync {
    async fn build(&self, stack: &ComposeStack, log: Arc<dyn LogSink>) -> Result<(), DomainError>;
    async fn up(&self, stack: &ComposeStack, log: Arc<dyn LogSink>) -> Result<(), DomainError>;
    async fn ps(&self, project_name: &str) -> Result<Vec<ServiceState>, DomainError>;
}

/// Реестр проектов + порт-аллокация (§6).
#[cfg_attr(feature = "mocks", automock)]
#[async_trait]
pub trait ProjectRepository: Send + Sync {
    /// Создаёт проект (с аллокацией host-порта) или обновляет конфиг,
    /// сохраняя уже выданный host-порт.
    async fn upsert(&self, config: &ProjectConfig) -> Result<Project, DomainError>;
    async fn list(&self) -> Result<Vec<Project>, DomainError>;
}

/// История деплоев (§6).
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

/// Пишет compose-override с маппингом 127.0.0.1:<host> → <container> (§12.1).
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

/// Детерминизм времени в тестах (§6).
#[cfg_attr(feature = "mocks", automock)]
pub trait Clock: Send + Sync {
    fn now_unix(&self) -> i64;
}

/// Детерминизм идентификаторов в тестах (§6).
#[cfg_attr(feature = "mocks", automock)]
pub trait IdGen: Send + Sync {
    fn new_id(&self) -> String;
}
