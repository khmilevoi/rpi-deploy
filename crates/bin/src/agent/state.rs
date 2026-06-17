use std::sync::Arc;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use pi_application::deploy::DeployProject;
use pi_application::diagnostics::{AgentStatus, RunDiagnostics};
use pi_application::env::{ListEnvKeys, SendEnv};
use pi_application::gc::RunGc;
use pi_application::lifecycle::ControlLifecycle;
use pi_application::list::ListProjects;
use pi_application::logs::StreamLogs;
use pi_application::remove::RemoveProject;
use pi_application::scheduler::{DeployRunner, DeployScheduler};
use pi_application::stats::GetStats;
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
use pi_infrastructure::probe::{HostSystemProbe, SystemRunner};
use pi_infrastructure::repo::SqliteProjectRepo;
use pi_infrastructure::secrets::EncryptedFileStore;
use pi_infrastructure::sqlite::Db;
use pi_infrastructure::stats::CompositeStats;
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
    pub stream_logs: Arc<StreamLogs>,
    pub stats: Arc<GetStats>,
    pub lifecycle: Arc<ControlLifecycle>,
    pub remove: Arc<RemoveProject>,
    pub diagnostics: Arc<RunDiagnostics>,
    pub agent_status: Arc<AgentStatus>,
    pub log_dir: PathBuf,
}

pub fn build_state(config: &AgentConfig) -> anyhow::Result<AppState> {
    std::fs::create_dir_all(&config.data_dir)?;
    let db = Db::open(&config.data_dir.join("state.db")).map_err(|e| anyhow::anyhow!("{e}"))?;

    let projects = SqliteProjectRepo::new(db.clone(), config.port_min, config.port_max);
    let history: Arc<dyn DeploymentHistory> = SqliteHistory::new(db, config.history_keep);
    let source = GitSource::new(&config.data_dir);
    let runtime = DockerComposeRuntime::new();
    let disk = SysinfoDiskProbe::new(&config.data_dir);
    let gc = RunGc::new(
        runtime.clone(),
        disk.clone(),
        config.gc.disk_threshold_percent,
    );
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
        ingress.clone(),
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
    let stream_logs = StreamLogs::new(projects.clone(), secrets.clone(), runtime.clone());
    let stats_provider = CompositeStats::new(runtime.clone(), disk.clone());
    let stats = GetStats::new(projects.clone(), Arc::clone(&history), stats_provider);
    let lifecycle = ControlLifecycle::new(projects.clone(), Arc::clone(&history), runtime.clone());
    let remove = RemoveProject::new(
        projects.clone(),
        Arc::clone(&history),
        runtime.clone(),
        ingress.clone(),
        source.clone(),
        secrets.clone(),
        overrides.clone(),
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let probe = HostSystemProbe::new(
        Arc::new(SystemRunner),
        disk,
        projects.clone(),
        env!("CARGO_PKG_VERSION").to_string(),
        config.gc.disk_threshold_percent,
        config.cloudflared.is_some(),
        now,
    );
    let diagnostics = RunDiagnostics::new(probe.clone());
    let agent_status = AgentStatus::new(probe, projects.clone(), Arc::clone(&history));
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
        stream_logs,
        stats,
        lifecycle,
        remove,
        diagnostics,
        agent_status,
        log_dir: config.logs.dir.clone(),
    })
}
