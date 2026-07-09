use crate::sqlite::{storage_err, Db};
use pi_domain::error::DomainError;

/// Records which host-level migrations (§7) have been applied, in state.db.
#[derive(Clone)]
pub struct MigrationLedger {
    db: Db,
}

impl MigrationLedger {
    pub fn new(db: Db) -> MigrationLedger {
        MigrationLedger { db }
    }

    pub async fn is_applied(&self, id: &str) -> Result<bool, DomainError> {
        let id = id.to_string();
        self.db
            .call(move |c| {
                let n: i64 = c
                    .query_row(
                        "SELECT count(*) FROM applied_migrations WHERE id = ?1",
                        [&id],
                        |r| r.get(0),
                    )
                    .map_err(storage_err)?;
                Ok(n > 0)
            })
            .await
    }

    pub async fn mark_applied(&self, id: &str, at_unix: i64) -> Result<(), DomainError> {
        let id = id.to_string();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT OR IGNORE INTO applied_migrations (id, applied_at) VALUES (?1, ?2)",
                    rusqlite::params![id, at_unix],
                )
                .map_err(storage_err)?;
                Ok(())
            })
            .await
    }

    pub async fn applied(&self) -> Result<Vec<String>, DomainError> {
        self.db
            .call(|c| {
                let mut stmt = c
                    .prepare("SELECT id FROM applied_migrations ORDER BY applied_at")
                    .map_err(storage_err)?;
                let ids = stmt
                    .query_map([], |r| r.get::<_, String>(0))
                    .map_err(storage_err)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(storage_err)?;
                Ok(ids)
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn ledger() -> MigrationLedger {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("state.db")).unwrap();
        // keep tempdir alive by leaking it into the DB's lifetime for the test
        std::mem::forget(dir);
        MigrationLedger::new(db)
    }

    #[tokio::test]
    async fn unknown_id_is_not_applied() {
        let l = ledger().await;
        assert!(!l.is_applied("pi-to-rpi").await.unwrap());
    }

    #[tokio::test]
    async fn mark_then_is_applied_and_listed() {
        let l = ledger().await;
        l.mark_applied("pi-to-rpi", 100).await.unwrap();
        assert!(l.is_applied("pi-to-rpi").await.unwrap());
        assert_eq!(l.applied().await.unwrap(), vec!["pi-to-rpi".to_string()]);
    }

    #[tokio::test]
    async fn mark_is_idempotent() {
        let l = ledger().await;
        l.mark_applied("pi-to-rpi", 100).await.unwrap();
        l.mark_applied("pi-to-rpi", 200).await.unwrap();
        assert_eq!(l.applied().await.unwrap().len(), 1);
    }
}
