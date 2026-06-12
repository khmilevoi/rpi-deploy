use thiserror::Error;

/// Domain errors. Infrastructure errors map to these variants at layer boundaries (§19).
#[derive(Debug, Error)]
pub enum DomainError {
    #[error("source error: {0}")]
    Source(String),
    #[error("container runtime error: {0}")]
    Runtime(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("secret store error: {0}")]
    Secrets(String),
    #[error("ingress error: {0}")]
    Ingress(String),
    #[error("health check failed: {0}")]
    HealthCheck(String),
    #[error("deployment canceled")]
    Canceled,
    #[error("timeout: {stage} after {secs}s")]
    Timeout { stage: String, secs: u64 },
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid input: {0}")]
    Invalid(String),
}
