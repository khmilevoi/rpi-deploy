use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::DeploymentHistory;
use pi_domain::entities::{Deployment, DeploymentStatus};
use pi_domain::error::DomainError;
use rusqlite::{params, OptionalExtension};

use crate::sqlite::{storage_err, Db};

pub struct SqliteHistory {
    db: Db,
    keep_per_project: usize,
}

impl SqliteHistory {
    /// keep_per_project — retention (§18): newest N rows per project survive,
    /// older terminal rows are deleted after each insert.
    pub fn new(db: Db, keep_per_project: usize) -> Arc<SqliteHistory> {
        Arc::new(SqliteHistory {
            db,
            keep_per_project,
        })
    }
}

fn row_to_deployment(row: &rusqlite::Row<'_>) -> Result<Deployment, rusqlite::Error> {
    let status: String = row.get(4)?;
    Ok(Deployment {
        id: row.get(0)?,
        project: row.get(1)?,
        git_ref: row.get(2)?,
        commit_sha: row.get(3)?,
        status: status.parse::<DeploymentStatus>().map_err(|_| {
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
    async fn record_queued(&self, deployment: &Deployment) -> Result<(), DomainError> {
        let d = deployment.clone();
        let keep = self.keep_per_project;
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
                conn.execute(
                    "DELETE FROM deployments
                     WHERE project = ?1
                       AND status NOT IN ('queued', 'running')
                       AND id NOT IN (
                           SELECT id FROM deployments
                           WHERE project = ?1
                           ORDER BY started_at DESC, id DESC
                           LIMIT ?2
                       )",
                    params![d.project, keep as i64],
                )
                .map_err(storage_err)?;
                Ok(())
            })
            .await
    }

    async fn mark_running(&self, id: &str, started_at: i64) -> Result<(), DomainError> {
        let (id, id_for_error) = (id.to_string(), id.to_string());
        self.db
            .call(move |conn| {
                let rows = conn
                    .execute(
                        "UPDATE deployments SET status='running', started_at=?2 WHERE id=?1 AND status='queued'",
                        params![id, started_at],
                    )
                    .map_err(storage_err)?;
                if rows == 0 {
                    return Err(DomainError::NotFound(format!(
                        "queued deployment {id_for_error}"
                    )));
                }
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
        let (id, id_for_error, sha, tail) = (
            id.to_string(),
            id.to_string(),
            commit_sha.map(str::to_string),
            log_tail.to_string(),
        );
        self.db
            .call(move |conn| {
                let rows = conn
                    .execute(
                    "UPDATE deployments SET status=?2, commit_sha=COALESCE(?3, commit_sha), finished_at=?4, log_tail=?5 WHERE id=?1",
                    params![id, status.as_str(), sha, finished_at, tail],
                )
                .map_err(storage_err)?;
                if rows == 0 {
                    return Err(DomainError::NotFound(format!(
                        "deployment {id_for_error}"
                    )));
                }
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

    async fn active(&self, project: &str) -> Result<Vec<Deployment>, DomainError> {
        let project = project.to_string();
        self.db
            .call(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, project, git_ref, commit_sha, status, started_at, finished_at, log_tail
                         FROM deployments
                         WHERE project = ?1 AND status IN ('queued', 'running')
                         ORDER BY started_at DESC, id DESC",
                    )
                    .map_err(storage_err)?;
                let rows = stmt
                    .query_map(params![project], row_to_deployment)
                    .map_err(storage_err)?;
                rows.collect::<Result<Vec<_>, _>>().map_err(storage_err)
            })
            .await
    }

    async fn sweep_interrupted(&self, finished_at: i64) -> Result<u64, DomainError> {
        self.db
            .call(move |conn| {
                conn.execute(
                    "UPDATE deployments
                     SET status='interrupted', finished_at=?1
                     WHERE status IN ('queued', 'running')",
                    params![finished_at],
                )
                .map(|rows| rows as u64)
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
        SqliteHistory::new(Db::open(&dir.path().join("state.db")).unwrap(), 50)
    }

    fn queued(id: &str, started_at: i64) -> Deployment {
        Deployment {
            id: id.into(),
            project: "rateme".into(),
            git_ref: "main".into(),
            commit_sha: None,
            status: DeploymentStatus::Queued,
            started_at,
            finished_at: None,
            log_tail: String::new(),
        }
    }

    #[tokio::test]
    async fn queued_then_mark_running_updates_status_and_started_at() {
        let dir = tempfile::tempdir().unwrap();
        let h = history(&dir);
        h.record_queued(&queued("d1", 100)).await.unwrap();
        assert_eq!(
            h.get("d1").await.unwrap().unwrap().status,
            DeploymentStatus::Queued
        );

        h.mark_running("d1", 150).await.unwrap();
        let d = h.get("d1").await.unwrap().unwrap();
        assert_eq!(d.status, DeploymentStatus::Running);
        assert_eq!(d.project, "rateme");
        assert_eq!(d.started_at, 150, "started_at refreshed to actual start");
        assert!(d.finished_at.is_none());
    }

    #[tokio::test]
    async fn mark_running_missing_or_not_queued_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let h = history(&dir);
        assert!(matches!(
            h.mark_running("missing", 1).await.unwrap_err(),
            DomainError::NotFound(_)
        ));
        h.record_queued(&queued("d1", 100)).await.unwrap();
        h.mark_running("d1", 150).await.unwrap();
        assert!(
            matches!(
                h.mark_running("d1", 160).await.unwrap_err(),
                DomainError::NotFound(_)
            ),
            "already running -> cannot mark again"
        );
    }

    #[tokio::test]
    async fn record_finished_updates_status_sha_and_tail() {
        let dir = tempfile::tempdir().unwrap();
        let h = history(&dir);
        h.record_queued(&queued("d1", 100)).await.unwrap();
        h.mark_running("d1", 100).await.unwrap();
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
    async fn record_finished_missing_deployment_returns_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = history(&dir)
            .record_finished("missing", DeploymentStatus::Failed, None, 200, "tail")
            .await
            .unwrap_err();

        assert!(matches!(err, DomainError::NotFound(message) if message.contains("missing")));
    }

    #[tokio::test]
    async fn record_finished_with_none_preserves_existing_commit_sha() {
        let dir = tempfile::tempdir().unwrap();
        let h = history(&dir);
        h.record_queued(&queued("d1", 100)).await.unwrap();
        h.mark_running("d1", 100).await.unwrap();
        h.record_finished("d1", DeploymentStatus::Success, Some("abc123"), 200, "ok")
            .await
            .unwrap();
        h.record_finished("d1", DeploymentStatus::Failed, None, 300, "failed")
            .await
            .unwrap();

        let d = h.get("d1").await.unwrap().unwrap();
        assert_eq!(d.status, DeploymentStatus::Failed);
        assert_eq!(d.commit_sha.as_deref(), Some("abc123"));
        assert_eq!(d.finished_at, Some(300));
        assert_eq!(d.log_tail, "failed");
    }

    #[tokio::test]
    async fn get_unknown_status_returns_storage_error() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("state.db")).unwrap();
        db.call(|conn| {
            conn.execute(
                "INSERT INTO deployments
                 (id, project, git_ref, status, started_at, log_tail)
                 VALUES ('d1', 'rateme', 'main', 'bogus', 100, '')",
                [],
            )
            .map_err(storage_err)?;
            Ok(())
        })
        .await
        .unwrap();

        let h = SqliteHistory::new(db, 50);
        let err = h.get("d1").await.unwrap_err();
        assert!(matches!(err, DomainError::Storage(message) if message.contains("unknown status")));
    }

    #[tokio::test]
    async fn get_unknown_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(history(&dir).get("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn active_returns_queued_and_running_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let h = history(&dir);
        h.record_queued(&queued("d1", 100)).await.unwrap();
        h.mark_running("d1", 110).await.unwrap();
        h.record_queued(&queued("d2", 120)).await.unwrap();
        h.record_queued(&queued("d3", 90)).await.unwrap();
        h.record_finished("d3", DeploymentStatus::Failed, None, 95, "")
            .await
            .unwrap();

        let active = h.active("rateme").await.unwrap();
        let ids: Vec<&str> = active.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, vec!["d2", "d1"], "newest first, terminal excluded");
        assert!(h.active("ghost").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn sweep_marks_queued_and_running_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        let h = history(&dir);
        h.record_queued(&queued("d1", 100)).await.unwrap();
        h.mark_running("d1", 110).await.unwrap();
        h.record_queued(&queued("d2", 120)).await.unwrap();
        h.record_queued(&queued("d3", 90)).await.unwrap();
        h.record_finished("d3", DeploymentStatus::Success, Some("abc"), 95, "")
            .await
            .unwrap();

        let swept = h.sweep_interrupted(200).await.unwrap();
        assert_eq!(swept, 2);
        for id in ["d1", "d2"] {
            let d = h.get(id).await.unwrap().unwrap();
            assert_eq!(d.status, DeploymentStatus::Interrupted);
            assert_eq!(d.finished_at, Some(200));
        }
        assert_eq!(
            h.get("d3").await.unwrap().unwrap().status,
            DeploymentStatus::Success,
            "terminal rows untouched"
        );
        assert_eq!(h.sweep_interrupted(300).await.unwrap(), 0, "idempotent");
    }

    #[tokio::test]
    async fn retention_prunes_old_terminal_rows_but_never_active_ones() {
        let dir = tempfile::tempdir().unwrap();
        let h = SqliteHistory::new(Db::open(&dir.path().join("state.db")).unwrap(), 2);
        for (id, at) in [("d1", 10), ("d2", 20), ("d3", 30)] {
            h.record_queued(&queued(id, at)).await.unwrap();
        }
        h.record_finished("d1", DeploymentStatus::Success, None, 11, "")
            .await
            .unwrap();
        h.mark_running("d2", 21).await.unwrap();
        h.record_finished("d3", DeploymentStatus::Failed, None, 31, "")
            .await
            .unwrap();

        h.record_queued(&queued("d4", 40)).await.unwrap();

        assert!(h.get("d1").await.unwrap().is_none(), "old terminal pruned");
        assert!(
            h.get("d2").await.unwrap().is_some(),
            "running row survives retention even though it is old"
        );
        assert!(h.get("d3").await.unwrap().is_some());
        assert!(h.get("d4").await.unwrap().is_some());
    }
}
