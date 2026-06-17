use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::ProjectRepository;
use pi_domain::entities::{
    ExposeMode, HealthcheckConfig, Project, ProjectConfig, StageTimeoutOverrides,
};
use pi_domain::error::DomainError;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::sqlite::{storage_err, Db};

const SELECT: &str = "SELECT name, repo, branch, compose_path, service, container_port, hostname, host_port, created_at, expose FROM projects";

pub struct SqliteProjectRepo {
    db: Db,
    port_min: u16,
    port_max: u16,
}

impl SqliteProjectRepo {
    pub fn new(db: Db, port_min: u16, port_max: u16) -> Arc<SqliteProjectRepo> {
        Arc::new(SqliteProjectRepo {
            db,
            port_min,
            port_max,
        })
    }
}

fn row_to_project(row: &rusqlite::Row<'_>) -> Result<Project, rusqlite::Error> {
    Ok(Project {
        config: ProjectConfig {
            name: row.get(0)?,
            repo: row.get(1)?,
            branch: row.get(2)?,
            compose_path: row.get(3)?,
            service: row.get(4)?,
            container_port: row.get(5)?,
            hostname: row.get(6)?,
            expose: ExposeMode::parse(&row.get::<_, String>(9)?).unwrap_or_default(),
            healthcheck: HealthcheckConfig::default(), // per-deploy input, not stored
            timeouts: StageTimeoutOverrides::default(), // per-deploy input, not stored
        },
        host_port: row.get(7)?,
        created_at: row.get(8)?,
    })
}

fn allocate_port(conn: &Connection, min: u16, max: u16) -> Result<u16, DomainError> {
    let mut stmt = conn
        .prepare("SELECT host_port FROM projects ORDER BY host_port")
        .map_err(storage_err)?;
    let used: Vec<u16> = stmt
        .query_map([], |r| r.get(0))
        .map_err(storage_err)?
        .collect::<Result<_, _>>()
        .map_err(storage_err)?;

    let mut candidate = min;
    for port in used {
        if port == candidate {
            candidate = candidate.saturating_add(1);
        } else if port > candidate {
            break;
        }
    }
    if candidate > max {
        return Err(DomainError::Invalid(format!(
            "no free host ports in {min}-{max}"
        )));
    }
    Ok(candidate)
}

#[async_trait]
impl ProjectRepository for SqliteProjectRepo {
    async fn upsert(&self, config: &ProjectConfig) -> Result<Project, DomainError> {
        let config = config.clone();
        let (min, max) = (self.port_min, self.port_max);
        self.db
            .call(move |conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(storage_err)?;
                let exists: Option<u16> = tx
                    .query_row(
                        "SELECT host_port FROM projects WHERE name = ?1",
                        params![&config.name],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(storage_err)?;
                if exists.is_some() {
                    tx.execute(
                        "UPDATE projects SET repo=?2, branch=?3, compose_path=?4, service=?5, container_port=?6, hostname=?7, expose=?8 WHERE name=?1",
                        params![
                            &config.name,
                            &config.repo,
                            &config.branch,
                            &config.compose_path,
                            &config.service,
                            config.container_port,
                            &config.hostname,
                            config.expose.as_str()
                        ],
                    )
                    .map_err(storage_err)?;
                } else {
                    let port = allocate_port(&tx, min, max)?;
                    tx.execute(
                        "INSERT INTO projects (name, repo, branch, compose_path, service, container_port, hostname, host_port, created_at, expose)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, unixepoch(), ?9)",
                        params![
                            &config.name,
                            &config.repo,
                            &config.branch,
                            &config.compose_path,
                            &config.service,
                            config.container_port,
                            &config.hostname,
                            port,
                            config.expose.as_str()
                        ],
                    )
                    .map_err(storage_err)?;
                }
                let project = tx
                    .query_row(
                        &format!("{SELECT} WHERE name = ?1"),
                        params![&config.name],
                        row_to_project,
                    )
                    .map_err(storage_err)?;
                tx.commit().map_err(storage_err)?;
                Ok(project)
            })
            .await
    }

    async fn get(&self, name: &str) -> Result<Option<Project>, DomainError> {
        let name = name.to_string();
        self.db
            .call(move |conn| {
                conn.query_row(
                    &format!("{SELECT} WHERE name = ?1"),
                    params![name],
                    row_to_project,
                )
                .optional()
                .map_err(storage_err)
            })
            .await
    }

    async fn list(&self) -> Result<Vec<Project>, DomainError> {
        self.db
            .call(|conn| {
                let mut stmt = conn
                    .prepare(&format!("{SELECT} ORDER BY name"))
                    .map_err(storage_err)?;
                let projects = stmt
                    .query_map([], row_to_project)
                    .map_err(storage_err)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(storage_err)?;
                Ok(projects)
            })
            .await
    }

    async fn remove(&self, name: &str) -> Result<(), DomainError> {
        let name = name.to_string();
        self.db
            .call(move |conn| {
                conn.execute("DELETE FROM projects WHERE name = ?1", params![name])
                    .map(|_| ())
                    .map_err(storage_err)
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::Db;
    use pi_domain::contracts::ProjectRepository;
    use pi_domain::entities::{
        ExposeMode, HealthcheckConfig, ProjectConfig, StageTimeoutOverrides,
    };
    use pi_domain::error::DomainError;
    use std::sync::Arc;

    fn cfg(name: &str) -> ProjectConfig {
        ProjectConfig {
            name: name.into(),
            repo: format!("git@github.com:x/{name}.git"),
            branch: "main".into(),
            compose_path: "docker-compose.yml".into(),
            service: "web".into(),
            container_port: 3000,
            hostname: None,
            expose: ExposeMode::default(),
            healthcheck: HealthcheckConfig::default(),
            timeouts: StageTimeoutOverrides::default(),
        }
    }

    fn repo(dir: &tempfile::TempDir, min: u16, max: u16) -> Arc<SqliteProjectRepo> {
        let db = Db::open(&dir.path().join("state.db")).unwrap();
        SqliteProjectRepo::new(db, min, max)
    }

    #[tokio::test]
    async fn allocates_sequential_ports_from_range() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir, 8000, 8999);
        assert_eq!(repo.upsert(&cfg("a")).await.unwrap().host_port, 8000);
        assert_eq!(repo.upsert(&cfg("b")).await.unwrap().host_port, 8001);
    }

    #[tokio::test]
    async fn upsert_existing_keeps_port_and_updates_config() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir, 8000, 8999);
        repo.upsert(&cfg("a")).await.unwrap();

        let mut updated = cfg("a");
        updated.branch = "develop".into();
        let project = repo.upsert(&updated).await.unwrap();

        assert_eq!(
            project.host_port, 8000,
            "host port is stable across re-deploys"
        );
        assert_eq!(project.config.branch, "develop");
        assert_eq!(repo.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn get_returns_upserted_project_and_none_for_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir, 8000, 8999);
        repo.upsert(&cfg("a")).await.unwrap();
        let found = repo.get("a").await.unwrap().unwrap();
        assert_eq!(found.config.name, "a");
        assert_eq!(found.host_port, 8000);
        assert!(repo.get("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn roundtrips_expose_mode() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir, 8000, 8999);
        let mut c = cfg("a");
        c.expose = pi_domain::entities::ExposeMode::Lan;
        repo.upsert(&c).await.unwrap();
        let got = repo.get("a").await.unwrap().unwrap();
        assert_eq!(got.config.expose, pi_domain::entities::ExposeMode::Lan);
    }

    #[tokio::test]
    async fn port_range_exhaustion_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir, 8000, 8000);
        repo.upsert(&cfg("a")).await.unwrap();
        let err = repo.upsert(&cfg("b")).await.unwrap_err();
        assert!(matches!(err, DomainError::Invalid(_)), "got: {err}");
    }

    #[tokio::test]
    async fn allocates_first_gap_in_port_range() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("state.db")).unwrap();
        db.call(|conn| {
            conn.execute(
                "INSERT INTO projects
                 (name, repo, branch, compose_path, service, container_port, host_port, created_at)
                 VALUES ('a', 'repo-a', 'main', 'docker-compose.yml', 'web', 3000, 8000, 1)",
                [],
            )
            .map_err(storage_err)?;
            conn.execute(
                "INSERT INTO projects
                 (name, repo, branch, compose_path, service, container_port, host_port, created_at)
                 VALUES ('c', 'repo-c', 'main', 'docker-compose.yml', 'web', 3000, 8002, 1)",
                [],
            )
            .map_err(storage_err)?;
            Ok(())
        })
        .await
        .unwrap();

        let repo = SqliteProjectRepo::new(db, 8000, 8999);
        assert_eq!(repo.upsert(&cfg("b")).await.unwrap().host_port, 8001);
    }

    #[tokio::test]
    async fn list_is_sorted_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir, 8000, 8999);
        repo.upsert(&cfg("zeta")).await.unwrap();
        repo.upsert(&cfg("alpha")).await.unwrap();
        let names: Vec<String> = repo
            .list()
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.config.name)
            .collect();
        assert_eq!(names, vec!["alpha".to_string(), "zeta".to_string()]);
    }
}
