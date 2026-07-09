use async_trait::async_trait;
use pi_infrastructure::migrations::MigrationLedger;

/// Thin async seam over MigrationLedger so the runner is unit-testable with a fake.
#[async_trait]
pub trait LedgerHandle: Send + Sync {
    async fn is_applied(&self, id: &str) -> bool;
    async fn mark_applied(&self, id: &str);
}

pub struct DbLedger {
    inner: MigrationLedger,
}

impl DbLedger {
    pub fn new(inner: MigrationLedger) -> DbLedger {
        DbLedger { inner }
    }
}

#[async_trait]
impl LedgerHandle for DbLedger {
    async fn is_applied(&self, id: &str) -> bool {
        self.inner.is_applied(id).await.unwrap_or(false)
    }
    async fn mark_applied(&self, id: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let _ = self.inner.mark_applied(id, now).await;
    }
}
