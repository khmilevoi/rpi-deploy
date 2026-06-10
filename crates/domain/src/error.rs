use thiserror::Error;

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("source error: {0}")]
    Source(String),
    #[error("container runtime error: {0}")]
    Runtime(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("deploy already in progress for project '{0}'")]
    DeployInProgress(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid input: {0}")]
    Invalid(String),
}
