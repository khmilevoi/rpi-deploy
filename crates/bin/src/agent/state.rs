use std::sync::Arc;

use pi_application::deploy::DeployProject;
use pi_application::list::ListProjects;
use pi_domain::contracts::{DeploymentHistory, IdGen};
use pi_infrastructure::docker::DockerComposeRuntime;
use pi_infrastructure::events::DeployEventsHub;
use pi_infrastructure::git::GitSource;
use pi_infrastructure::history::SqliteHistory;
use pi_infrastructure::overrides::FsOverrideStore;
use pi_infrastructure::repo::SqliteProjectRepo;
use pi_infrastructure::sqlite::Db;
use pi_infrastructure::sys::{SystemClock, UuidGen};

use crate::agent::config::AgentConfig;

#[derive(Clone)]
pub struct AppState {
    pub deploy: Arc<DeployProject>,
    pub list: Arc<ListProjects>,
    pub history: Arc<dyn DeploymentHistory>,
    pub hub: Arc<DeployEventsHub>,
    pub ids: Arc<dyn IdGen>,
}

pub fn build_state(config: &AgentConfig) -> anyhow::Result<AppState> {
    std::fs::create_dir_all(&config.data_dir)?;
    let db = Db::open(&config.data_dir.join("state.db")).map_err(|e| anyhow::anyhow!("{e}"))?;

    let projects = SqliteProjectRepo::new(db.clone(), config.port_min, config.port_max);
    let history: Arc<dyn DeploymentHistory> = SqliteHistory::new(db);
    let source = GitSource::new(&config.data_dir);
    let runtime = DockerComposeRuntime::new();
    let overrides = FsOverrideStore::new(config.data_dir.join("overrides"));

    let deploy = DeployProject::new(
        source,
        runtime.clone(),
        projects.clone(),
        Arc::clone(&history),
        overrides,
        SystemClock::new(),
    );
    let list = ListProjects::new(projects, runtime);

    Ok(AppState { deploy, list, history, hub: DeployEventsHub::new(), ids: UuidGen::new() })
}
