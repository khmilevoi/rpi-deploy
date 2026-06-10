use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::DeploymentHistory;
use pi_domain::entities::{Deployment, DeploymentStatus};
use pi_domain::error::DomainError;
use rusqlite::{params, OptionalExtension};

use crate::sqlite::{storage_err, Db};

pub struct SqliteHistory {
    db: Db,
}

impl SqliteHistory {
    pub fn new(db: Db) -> Arc<SqliteHistory> {
        Arc::new(SqliteHistory { db })
    }
}

fn row_to_deployment(row: &rusqlite::Row<'_>) -> Result<Deployment, rusqlite::Error> {
    let status: String = row.get(4)?;
    Ok(Deployment {
        id: row.get(0)?,
        project: row.get(1)?,
        git_ref: row.get(2)?,
        commit_sha: row.get(3)?,
        status: DeploymentStatus::from_str(&status).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                format!("unknown status '{status}'").into(),
            )
        })?,
        started_at: row.get(5)?,
        finished_at: row.get(6)?,
        log_tail: row.get(7)?,
    })
}

#[async_trait]
impl DeploymentHistory for SqliteHistory {
    async fn record_started(&self, deployment: &Deployment) -> Result<(), DomainError> {
        let d = deployment.clone();
        self.db
            .call(move |conn| {
                conn.execute(
                    "INSERT INTO deployments (id, project, git_ref, commit_sha, status, started_at, finished_at, log_tail)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        d.id,
                        d.project,
                        d.git_ref,
                        d.commit_sha,
                        d.status.as_str(),
                        d.started_at,
                        d.finished_at,
                        d.log_tail
                    ],
                )
                .map_err(storage_err)?;
                Ok(())
            })
            .await
    }

    async fn record_finished<'a>(
        &self,
        id: &str,
        status: DeploymentStatus,
        commit_sha: Option<&'a str>,
        finished_at: i64,
        log_tail: &str,
    ) -> Result<(), DomainError> {
        let (id, sha, tail) = (
            id.to_string(),
            commit_sha.map(str::to_string),
            log_tail.to_string(),
        );
        self.db
            .call(move |conn| {
                conn.execute(
                    "UPDATE deployments SET status=?2, commit_sha=COALESCE(?3, commit_sha), finished_at=?4, log_tail=?5 WHERE id=?1",
                    params![id, status.as_str(), sha, finished_at, tail],
                )
                .map_err(storage_err)?;
                Ok(())
            })
            .await
    }

    async fn get(&self, id: &str) -> Result<Option<Deployment>, DomainError> {
        let id = id.to_string();
        self.db
            .call(move |conn| {
                conn.query_row(
                    "SELECT id, project, git_ref, commit_sha, status, started_at, finished_at, log_tail FROM deployments WHERE id = ?1",
                    params![id],
                    row_to_deployment,
                )
                .optional()
                .map_err(storage_err)
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::sqlite::Db;
    use pi_domain::contracts::DeploymentHistory;
    use pi_domain::entities::{Deployment, DeploymentStatus};

    fn history(dir: &tempfile::TempDir) -> Arc<SqliteHistory> {
        SqliteHistory::new(Db::open(&dir.path().join("state.db")).unwrap())
    }

    fn started(id: &str) -> Deployment {
        Deployment {
            id: id.into(),
            project: "rateme".into(),
            git_ref: "main".into(),
            commit_sha: None,
            status: DeploymentStatus::Running,
            started_at: 100,
            finished_at: None,
            log_tail: String::new(),
        }
    }

    #[tokio::test]
    async fn record_started_then_get_returns_running() {
        let dir = tempfile::tempdir().unwrap();
        let h = history(&dir);
        h.record_started(&started("d1")).await.unwrap();
        let d = h.get("d1").await.unwrap().unwrap();
        assert_eq!(d.status, DeploymentStatus::Running);
        assert_eq!(d.project, "rateme");
        assert!(d.finished_at.is_none());
    }

    #[tokio::test]
    async fn record_finished_updates_status_sha_and_tail() {
        let dir = tempfile::tempdir().unwrap();
        let h = history(&dir);
        h.record_started(&started("d1")).await.unwrap();
        h.record_finished(
            "d1",
            DeploymentStatus::Success,
            Some("abc123"),
            200,
            "line1\nline2",
        )
        .await
        .unwrap();
        let d = h.get("d1").await.unwrap().unwrap();
        assert_eq!(d.status, DeploymentStatus::Success);
        assert_eq!(d.commit_sha.as_deref(), Some("abc123"));
        assert_eq!(d.finished_at, Some(200));
        assert_eq!(d.log_tail, "line1\nline2");
    }

    #[tokio::test]
    async fn get_unknown_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(history(&dir).get("nope").await.unwrap().is_none());
    }
}
