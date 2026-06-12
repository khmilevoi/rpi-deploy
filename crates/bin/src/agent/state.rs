use std::sync::Arc;

use pi_application::deploy::DeployProject;
use pi_application::env::{ListEnvKeys, SendEnv};
use pi_application::gc::RunGc;
use pi_application::list::ListProjects;
use pi_application::scheduler::{DeployRunner, DeployScheduler};
use pi_domain::contracts::{DeploymentHistory, IdGen, Ingress};
use pi_infrastructure::cloudflared::{CloudflaredIngress, DisabledIngress};
use pi_infrastructure::disk::SysinfoDiskProbe;
use pi_infrastructure::docker::DockerComposeRuntime;
use pi_infrastructure::envfile::FsEnvFileWriter;
use pi_infrastructure::events::DeployEventsHub;
use pi_infrastructure::git::GitSource;
use pi_infrastructure::health::HybridHealthGate;
use pi_infrastructure::history::SqliteHistory;
use pi_infrastructure::overrides::FsOverrideStore;
use pi_infrastructure::repo::SqliteProjectRepo;
use pi_infrastructure::secrets::EncryptedFileStore;
use pi_infrastructure::sqlite::Db;
use pi_infrastructure::sys::{SystemClock, UuidGen};

use crate::agent::config::AgentConfig;

#[derive(Clone)]
pub struct AppState {
    pub scheduler: Arc<DeployScheduler>,
    pub list: Arc<ListProjects>,
    pub history: Arc<dyn DeploymentHistory>,
    pub hub: Arc<DeployEventsHub>,
    pub ids: Arc<dyn IdGen>,
    pub send_env: Arc<SendEnv>,
    pub env_keys: Arc<ListEnvKeys>,
    pub gc: Arc<RunGc>,
}

pub fn build_state(config: &AgentConfig) -> anyhow::Result<AppState> {
    std::fs::create_dir_all(&config.data_dir)?;
    let db = Db::open(&config.data_dir.join("state.db")).map_err(|e| anyhow::anyhow!("{e}"))?;

    let projects = SqliteProjectRepo::new(db.clone(), config.port_min, config.port_max);
    let history: Arc<dyn DeploymentHistory> = SqliteHistory::new(db, config.history_keep);
    let source = GitSource::new(&config.data_dir);
    let runtime = DockerComposeRuntime::new();
    let disk = SysinfoDiskProbe::new(&config.data_dir);
    let gc = RunGc::new(runtime.clone(), disk, config.gc.disk_threshold_percent);
    let overrides = FsOverrideStore::new(config.data_dir.join("overrides"));
    let secrets = EncryptedFileStore::open(&config.data_dir).map_err(|e| anyhow::anyhow!("{e}"))?;
    let env_files: Arc<dyn pi_domain::contracts::EnvFileWriter> = FsEnvFileWriter::new();
    let health = HybridHealthGate::new(runtime.clone());
    let ingress: Arc<dyn Ingress> = match &config.cloudflared {
        Some(cf) => {
            CloudflaredIngress::new(cf.config.clone(), cf.tunnel.clone(), cf.restart.clone())
        }
        None => DisabledIngress::new(),
    };

    let deploy = DeployProject::new(
        source.clone(),
        runtime.clone(),
        projects.clone(),
        Arc::clone(&history),
        overrides.clone(),
        secrets.clone(),
        Arc::clone(&env_files),
        health,
        ingress,
        SystemClock::new(),
        Arc::clone(&gc),
        config.stage_timeouts()?,
        config.build_concurrency,
    );
    let scheduler = DeployScheduler::new(
        deploy as Arc<dyn DeployRunner>,
        Arc::clone(&history),
        SystemClock::new(),
    );
    let list = ListProjects::new(projects.clone(), runtime.clone());
    let send_env = SendEnv::new(
        secrets.clone(),
        projects,
        source,
        env_files,
        overrides,
        runtime,
    );
    let env_keys = ListEnvKeys::new(secrets);

    Ok(AppState {
        scheduler,
        list,
        history,
        hub: DeployEventsHub::new(),
        ids: UuidGen::new(),
        send_env,
        env_keys,
        gc,
    })
}
