use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use base64::Engine as _;
use pi_application::logs::DEFAULT_LOG_TAIL;
use pi_domain::entities::{
    DeployRef, DeploymentStatus, LifecycleAction, ProjectConfig, SecretsBundle,
};
use pi_domain::error::DomainError;
use pi_infrastructure::events::DeployEvent;
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::agent::logfile;
use crate::agent::state::AppState;
use crate::proto::{
    AgentOverviewDto, CommandRunRequest, CommandsResponse, DeployAccepted, DeployRequest,
    DeploymentDto, DiagnosticReportDto, EnvKeysResponse, EnvSendRequest, EnvSendResponse,
    GcResponse, LifecycleResponse, ProjectViewDto, RemoveResponse, SecretsListResponse,
    SecretsSendRequest, SecretsSendResponse, StatsReportDto, VersionInfo,
};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/version", get(version))
        .route("/v1/gc", post(run_gc))
        .route("/v1/stats", get(stats))
        .route("/v1/status", get(agent_status))
        .route("/v1/doctor", get(doctor))
        .route("/v1/agent/logs", get(agent_logs))
        .route("/v1/deployments", post(create_deployment))
        .route(
            "/v1/deployments/{id}",
            get(get_deployment).delete(cancel_deployment),
        )
        .route("/v1/deployments/{id}/logs", get(deployment_logs))
        .route("/v1/projects", get(list_projects))
        .route("/v1/projects/{name}", delete(remove_project))
        .route("/v1/projects/{name}/logs", get(project_logs))
        .route("/v1/projects/{name}/lifecycle/{action}", post(lifecycle))
        .route("/v1/projects/{name}/commands", get(list_commands))
        .route("/v1/projects/{name}/commands/{command}", post(run_command))
        .route(
            "/v1/projects/{name}/deployments/active",
            get(active_deployments),
        )
        .route(
            "/v1/projects/{name}/env",
            put(send_env_handler).get(env_keys_handler),
        )
        .route(
            "/v1/projects/{name}/secrets",
            put(send_secrets_handler).get(list_secrets_handler),
        )
        // base64 inflates the 8 MiB bundle limit by ~4/3; leave headroom
        .layer(DefaultBodyLimit::max(12 * 1024 * 1024))
        .with_state(state)
}

pub struct ApiError(pub DomainError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            DomainError::Conflict(_) => StatusCode::CONFLICT,
            DomainError::NotFound(_) => StatusCode::NOT_FOUND,
            DomainError::Invalid(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(serde_json::json!({ "error": self.0.to_string() })),
        )
            .into_response()
    }
}

async fn version() -> Json<VersionInfo> {
    Json(VersionInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        api: "v1".to_string(),
    })
}

fn is_valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some('a'..='z' | '0'..='9'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_' | '-'))
}

async fn create_deployment(
    State(state): State<AppState>,
    Json(req): Json<DeployRequest>,
) -> Result<Response, ApiError> {
    let config: ProjectConfig = req.project.into();
    if !is_valid_name(&config.name) {
        return Err(ApiError(DomainError::Invalid(
            "project.name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    if !is_valid_name(&config.service) {
        return Err(ApiError(DomainError::Invalid(
            "project.service must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    if config.container_port == 0 {
        return Err(ApiError(DomainError::Invalid(
            "project.port must be > 0".into(),
        )));
    }
    for (cmd_name, spec) in &config.commands {
        if !is_valid_name(cmd_name) {
            return Err(ApiError(DomainError::Invalid(format!(
                "command name '{cmd_name}' must match ^[a-z0-9][a-z0-9_-]*$"
            ))));
        }
        if spec.argv.is_empty() || spec.argv.iter().any(|a| a.is_empty()) {
            return Err(ApiError(DomainError::Invalid(format!(
                "command '{cmd_name}' must have a non-empty argv"
            ))));
        }
    }
    let git_ref = DeployRef::parse(req.git_ref.as_deref().unwrap_or(&config.branch));

    let deployment_id = state.ids.new_id();
    let sink = state.hub.register(&deployment_id);
    let outcome = state
        .scheduler
        .submit(deployment_id.clone(), config, git_ref, sink)
        .await
        .map_err(ApiError)?;
    let queued = !matches!(outcome, pi_application::scheduler::SubmitOutcome::Started);

    Ok((
        StatusCode::ACCEPTED,
        Json(DeployAccepted {
            deployment_id,
            queued,
        }),
    )
        .into_response())
}

const GC_TIMEOUT_SECS: u64 = 300;

/// POST /v1/gc (§8.1): same RunGc as the post-deploy stage, on demand.
async fn run_gc(State(state): State<AppState>) -> Result<Json<GcResponse>, ApiError> {
    let report = tokio::time::timeout(
        std::time::Duration::from_secs(GC_TIMEOUT_SECS),
        state.gc.execute(Arc::new(TracingSink)),
    )
    .await
    .map_err(|_| {
        ApiError(DomainError::Timeout {
            stage: "gc".to_string(),
            secs: GC_TIMEOUT_SECS,
        })
    })?
    .map_err(ApiError)?;
    Ok(Json(GcResponse {
        disk_used_percent: report.disk_used_percent,
        builder_pruned: report.builder_pruned,
    }))
}

#[derive(Debug, Deserialize)]
struct StatsQuery {
    project: Option<String>,
}

async fn stats(
    State(state): State<AppState>,
    Query(q): Query<StatsQuery>,
) -> Result<Json<StatsReportDto>, ApiError> {
    Ok(Json(
        state
            .stats
            .execute(q.project)
            .await
            .map_err(ApiError)?
            .into(),
    ))
}

async fn agent_status(State(state): State<AppState>) -> Result<Json<AgentOverviewDto>, ApiError> {
    Ok(Json(
        state.agent_status.execute().await.map_err(ApiError)?.into(),
    ))
}

async fn doctor(State(state): State<AppState>) -> Json<DiagnosticReportDto> {
    Json(state.diagnostics.execute().await.into())
}

async fn lifecycle(
    State(state): State<AppState>,
    Path((name, action)): Path<(String, String)>,
) -> Result<Json<LifecycleResponse>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    let action = action
        .parse::<LifecycleAction>()
        .map_err(|_| ApiError(DomainError::Invalid("invalid lifecycle action".into())))?;
    state
        .lifecycle
        .execute(&name, action, Arc::new(TracingSink))
        .await
        .map_err(ApiError)?;
    Ok(Json(LifecycleResponse {
        project: name,
        action: action.as_str().to_string(),
    }))
}

async fn list_commands(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<CommandsResponse>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    let commands = state.commands.list(&name).await.map_err(ApiError)?;
    Ok(Json(CommandsResponse { commands }))
}

async fn run_command(
    State(state): State<AppState>,
    Path((name, command)): Path<(String, String)>,
    Json(req): Json<CommandRunRequest>,
) -> Result<Response, ApiError> {
    if !is_valid_name(&name) || !is_valid_name(&command) {
        return Err(ApiError(DomainError::Invalid(
            "project and command names must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    // 404 with a JSON error before the SSE stream opens.
    state
        .commands
        .resolve(&name, &command)
        .await
        .map_err(ApiError)?;

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let run = state.commands.clone();
    let args = req.args;
    let (task_name, task_cmd, task_args) = (name.clone(), command.clone(), args.clone());
    let started = std::time::Instant::now();
    let handle = tokio::spawn(async move {
        run.execute(&task_name, &task_cmd, &task_args, Arc::new(ChannelSink(tx)))
            .await
    });
    let stream = async_stream::stream! {
        // Client disconnect drops this stream -> guard aborts the task ->
        // the exec future is dropped -> kill_on_drop kills `docker compose
        // exec` (best effort; the in-container process may survive).
        let mut guard = AbortOnDrop(handle);
        while let Some(line) = rx.recv().await {
            yield sse_log(line);
        }
        let code = match (&mut guard.0).await {
            Ok(Ok(code)) => code,
            Ok(Err(e)) => {
                yield sse_log(format!("error: {e}"));
                1
            }
            Err(_) => 1,
        };
        tracing::info!(
            "command run: project={name} command={command} args_count={} exit={code} duration={}s",
            args.len(),
            started.elapsed().as_secs()
        );
        yield sse_exit(code);
    };
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

#[derive(Debug, Deserialize)]
struct RemoveQuery {
    #[serde(default)]
    volumes: bool,
}

async fn remove_project(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<RemoveQuery>,
) -> Result<Json<RemoveResponse>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    Ok(Json(
        state
            .remove
            .execute(&name, q.volumes, Arc::new(TracingSink))
            .await
            .map_err(ApiError)?
            .into(),
    ))
}

async fn get_deployment(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DeploymentDto>, ApiError> {
    match state.history.get(&id).await.map_err(ApiError)? {
        Some(d) => Ok(Json(d.into())),
        None => Err(ApiError(DomainError::NotFound(format!("deployment {id}")))),
    }
}

/// DELETE /v1/deployments/{id} (§8.1, §9.1): queued — removed immediately,
/// running — the cancel token is signalled and the runner records `canceled`.
async fn cancel_deployment(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use pi_application::scheduler::CancelOutcome;
    match state.scheduler.cancel(&id).await.map_err(ApiError)? {
        CancelOutcome::CanceledQueued => Ok(Json(serde_json::json!({ "status": "canceled" }))),
        CancelOutcome::CancelRequested => Ok(Json(serde_json::json!({ "status": "canceling" }))),
        CancelOutcome::NotActive => match state.history.get(&id).await.map_err(ApiError)? {
            Some(d) if d.status.is_terminal() => Err(ApiError(DomainError::Conflict(format!(
                "deployment {id} already finished ({})",
                d.status.as_str()
            )))),
            // DB row is queued/running but the scheduler does not know it:
            // either it is finishing this instant or a restart orphaned the row
            // (the startup sweep will mark it interrupted).
            Some(d) => Err(ApiError(DomainError::Conflict(format!(
                "deployment {id} is recorded as {} but is not active in the scheduler; \
it may be finishing right now or was orphaned by an agent restart",
                d.status.as_str()
            )))),
            None => Err(ApiError(DomainError::NotFound(format!("deployment {id}")))),
        },
    }
}

async fn list_projects(
    State(state): State<AppState>,
) -> Result<Json<Vec<ProjectViewDto>>, ApiError> {
    let views = state.list.execute().await.map_err(ApiError)?;
    Ok(Json(views.into_iter().map(Into::into).collect()))
}

/// Active (queued/running) deployments of a project — used by `rpi deploy --cancel`.
async fn active_deployments(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Vec<DeploymentDto>>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    let list = state.history.active(&name).await.map_err(ApiError)?;
    Ok(Json(list.into_iter().map(Into::into).collect()))
}

fn sse_log(line: String) -> Result<Event, Infallible> {
    Ok(Event::default().event("log").data(line))
}

struct ChannelSink(mpsc::UnboundedSender<String>);

impl pi_domain::contracts::LogSink for ChannelSink {
    fn line(&self, line: &str) {
        let _ = self.0.send(line.to_string());
    }

    fn finished(&self, _status: DeploymentStatus) {}
}

struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);
impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Debug, Deserialize)]
struct LogsQuery {
    #[serde(default = "default_tail")]
    tail: usize,
    #[serde(default)]
    follow: bool,
    since: Option<i64>,
}

fn default_tail() -> usize {
    DEFAULT_LOG_TAIL
}

async fn project_logs(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<LogsQuery>,
) -> Result<Response, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    state
        .stream_logs
        .ensure_project(&name)
        .await
        .map_err(ApiError)?;
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let logs = state.stream_logs.clone();
    let handle = tokio::spawn(async move {
        let _ = logs
            .execute(&name, q.tail, q.follow, Arc::new(ChannelSink(tx)))
            .await;
    });
    let stream = async_stream::stream! {
        let _guard = AbortOnDrop(handle);
        while let Some(line) = rx.recv().await {
            yield sse_log(line);
        }
    };
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

async fn agent_logs(
    State(state): State<AppState>,
    Query(q): Query<LogsQuery>,
) -> Result<Response, ApiError> {
    if !state.log_dir_available {
        return Err(ApiError(DomainError::NotFound(
            "agent file logging is disabled/unavailable".into(),
        )));
    }
    let tail = if q.since.is_some() {
        None
    } else {
        Some(q.tail)
    };
    let initial = logfile::read(&state.log_dir, tail, q.since)
        .map_err(|e| ApiError(DomainError::Storage(format!("agent logs: {e}"))))?;
    if !q.follow {
        let stream = async_stream::stream! {
            for line in initial {
                yield sse_log(line);
            }
        };
        return Ok(Sse::new(stream).into_response());
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    for line in initial {
        let _ = tx.send(line);
    }
    let dir = state.log_dir.clone();
    let handle = tokio::spawn(async move {
        let _ = logfile::follow(dir, q.since, |line| tx.send(line).is_ok()).await;
    });
    let stream = async_stream::stream! {
        let _guard = AbortOnDrop(handle);
        while let Some(line) = rx.recv().await {
            yield sse_log(line);
        }
    };
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

fn sse_finished(status: &str) -> Result<Event, Infallible> {
    Ok(Event::default().event("finished").data(status))
}

/// Terminal event of a command run: the in-container exit code.
fn sse_exit(code: i32) -> Result<Event, Infallible> {
    Ok(Event::default().event("exit").data(code.to_string()))
}

async fn deployment_logs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    if let Some(sub) = state.hub.subscribe(&id) {
        let stream = async_stream::stream! {
            for line in sub.backlog {
                yield sse_log(line);
            }
            let mut live = sub.live;
            loop {
                match live.recv().await {
                    Ok(DeployEvent::Line(line)) => yield sse_log(line),
                    Ok(DeployEvent::Finished(status)) => {
                        yield sse_finished(status.as_str());
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        };
        return Ok(Sse::new(stream)
            .keep_alive(KeepAlive::default())
            .into_response());
    }

    match state.history.get(&id).await.map_err(ApiError)? {
        Some(d) => {
            let stream = async_stream::stream! {
                for line in d.log_tail.lines().map(str::to_string) {
                    yield sse_log(line);
                }
                let status_str = if d.status.is_terminal() { d.status.as_str() } else { "interrupted" };
                yield sse_finished(status_str);
            };
            Ok(Sse::new(stream).into_response())
        }
        None => Err(ApiError(DomainError::NotFound(format!("deployment {id}")))),
    }
}

/// `rpi secrets send --apply` runs `up -d` synchronously; its output goes to
/// the agent log (journald), the CLI gets a compact JSON summary.
struct TracingSink;

impl pi_domain::contracts::LogSink for TracingSink {
    fn line(&self, line: &str) {
        tracing::info!("{line}");
    }
    fn finished(&self, _status: DeploymentStatus) {}
}

async fn send_env_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<EnvSendRequest>,
) -> Result<Json<EnvSendResponse>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    for (key, value) in &req.vars {
        if !pi_infrastructure::dotenv::is_valid_key(key) {
            return Err(ApiError(DomainError::Invalid(format!(
                "invalid env key '{key}'"
            ))));
        }
        if value.contains('\n') {
            return Err(ApiError(DomainError::Invalid(format!(
                "value of '{key}' contains a newline (multi-line values are unsupported)"
            ))));
        }
    }
    let bundle = SecretsBundle {
        vars: req.vars,
        files: std::collections::BTreeMap::new(),
    };
    let saved = state
        .send_secrets
        .execute(&name, bundle, req.apply, Arc::new(TracingSink))
        .await
        .map_err(ApiError)?;
    Ok(Json(EnvSendResponse {
        saved_keys: saved.keys,
        applied: saved.applied,
    }))
}

async fn env_keys_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<EnvKeysResponse>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    let stored = state.list_secrets.execute(&name).await.map_err(ApiError)?;
    Ok(Json(EnvKeysResponse { keys: stored.keys }))
}

async fn send_secrets_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<SecretsSendRequest>,
) -> Result<Json<SecretsSendResponse>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    for (key, value) in &req.vars {
        if !pi_infrastructure::dotenv::is_valid_key(key) {
            return Err(ApiError(DomainError::Invalid(format!(
                "invalid env key '{key}'"
            ))));
        }
        if value.contains('\n') {
            return Err(ApiError(DomainError::Invalid(format!(
                "value of '{key}' contains a newline (multi-line values are unsupported)"
            ))));
        }
    }
    let mut files = std::collections::BTreeMap::new();
    let mut total: usize = 0;
    for (path, b64) in &req.files {
        pi_infrastructure::secretpath::validate_rel_path(path)
            .map_err(|e| ApiError(DomainError::Invalid(format!("secret file '{path}': {e}"))))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|_| {
                ApiError(DomainError::Invalid(format!(
                    "secret file '{path}': contents are not valid base64"
                )))
            })?;
        if bytes.len() > crate::proto::MAX_SECRET_FILE_BYTES {
            return Err(ApiError(DomainError::Invalid(format!(
                "secret file '{path}' is {} bytes; max is 1 MiB",
                bytes.len()
            ))));
        }
        total += bytes.len();
        if total > crate::proto::MAX_SECRETS_BUNDLE_BYTES {
            return Err(ApiError(DomainError::Invalid(
                "secret files exceed 8 MiB total".into(),
            )));
        }
        files.insert(path.clone(), bytes);
    }
    let bundle = SecretsBundle {
        vars: req.vars,
        files,
    };
    let saved = state
        .send_secrets
        .execute(&name, bundle, req.apply, Arc::new(TracingSink))
        .await
        .map_err(ApiError)?;
    Ok(Json(SecretsSendResponse {
        saved_keys: saved.keys,
        saved_files: saved.files,
        applied: saved.applied,
    }))
}

async fn list_secrets_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<SecretsListResponse>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError(DomainError::Invalid(
            "project name must match ^[a-z0-9][a-z0-9_-]*$".into(),
        )));
    }
    let stored = state.list_secrets.execute(&name).await.map_err(ApiError)?;
    Ok(Json(SecretsListResponse {
        keys: stored.keys,
        files: stored.files,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::state::AppState;
    use http_body_util::BodyExt;
    use pi_application::deploy::DeployProject;
    use pi_application::diagnostics::{AgentStatus, RunDiagnostics};
    use pi_application::gc::RunGc;
    use pi_application::lifecycle::ControlLifecycle;
    use pi_application::list::ListProjects;
    use pi_application::logs::StreamLogs;
    use pi_application::remove::RemoveProject;
    use pi_application::secrets::{ListSecrets, SendSecrets};
    use pi_application::stats::GetStats;
    use pi_domain::contracts::{
        ContainerRuntime, LogSink, MockContainerRuntime, MockDiskProbe, MockSource, Source,
    };
    use pi_domain::entities::{FetchedSource, StageTimeouts};
    use pi_infrastructure::events::DeployEventsHub;
    use pi_infrastructure::history::SqliteHistory;
    use pi_infrastructure::overrides::FsOverrideStore;
    use pi_infrastructure::probe::{HostSystemProbe, SystemRunner};
    use pi_infrastructure::repo::SqliteProjectRepo;
    use pi_infrastructure::sqlite::Db;
    use pi_infrastructure::stats::CompositeStats;
    use pi_infrastructure::sys::{SystemClock, UuidGen};
    use tower::ServiceExt;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn ok_source() -> MockSource {
        let mut source = MockSource::new();
        source.expect_fetch().returning(|p, _, _| {
            Ok(FetchedSource {
                workdir: std::env::temp_dir().join(&p.name),
                commit_sha: SHA.into(),
            })
        });
        source
            .expect_workdir()
            .returning(|name| std::env::temp_dir().join(name));
        source
    }

    fn ok_runtime() -> MockContainerRuntime {
        let mut runtime = MockContainerRuntime::new();
        runtime.expect_build().returning(|_, _| Ok(()));
        runtime.expect_up().returning(|_, _| Ok(()));
        runtime.expect_prune_images().returning(|_| Ok(()));
        runtime.expect_prune_builder().returning(|_| Ok(()));
        runtime.expect_ps().returning(|_| {
            Ok(vec![pi_domain::entities::ServiceState {
                service: "web".into(),
                state: "running".into(),
                health: Some("healthy".into()),
            }])
        });
        runtime
    }

    fn state_with(
        dir: &std::path::Path,
        source: Arc<dyn Source>,
        runtime: Arc<dyn ContainerRuntime>,
    ) -> AppState {
        use pi_infrastructure::cloudflared::DisabledIngress;
        use pi_infrastructure::health::HybridHealthGate;
        use pi_infrastructure::hostnet::UdpHostNetwork;
        use pi_infrastructure::secrets::EncryptedFileStore;
        use pi_infrastructure::secretsfile::FsSecretsWriter;

        let db = Db::open(&dir.join("state.db")).unwrap();
        let projects = SqliteProjectRepo::new(db.clone(), 8000, 8999);
        let history: Arc<dyn pi_domain::contracts::DeploymentHistory> = SqliteHistory::new(db, 50);
        let overrides = FsOverrideStore::new(dir.join("overrides"));
        let secrets = EncryptedFileStore::open(dir).unwrap();
        let health = HybridHealthGate::with_interval(
            Arc::clone(&runtime),
            std::time::Duration::from_millis(10),
        );
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(10));
        let disk = Arc::new(disk);
        let gc = RunGc::new(Arc::clone(&runtime), disk.clone(), 85);
        let deploy = DeployProject::new(
            source.clone(),
            Arc::clone(&runtime),
            projects.clone(),
            Arc::clone(&history),
            overrides.clone(),
            secrets.clone(),
            FsSecretsWriter::new(),
            health,
            DisabledIngress::new(),
            Arc::new(UdpHostNetwork::new()),
            SystemClock::new(),
            Arc::clone(&gc),
            StageTimeouts::default(),
            1,
        );
        let scheduler = pi_application::scheduler::DeployScheduler::new(
            deploy as Arc<dyn pi_application::scheduler::DeployRunner>,
            Arc::clone(&history),
            SystemClock::new(),
        );
        let list = ListProjects::new(
            projects.clone(),
            Arc::clone(&runtime),
            Arc::new(UdpHostNetwork::new()),
        );
        let stream_logs = StreamLogs::new(projects.clone(), secrets.clone(), Arc::clone(&runtime));
        let stats_provider = CompositeStats::new(Arc::clone(&runtime), disk.clone());
        let stats = GetStats::new(projects.clone(), Arc::clone(&history), stats_provider);
        let lifecycle = ControlLifecycle::new(
            projects.clone(),
            Arc::clone(&runtime),
            source.clone(),
            overrides.clone(),
        );
        let commands = pi_application::command::RunCommand::new(
            projects.clone(),
            Arc::clone(&runtime),
            source.clone(),
            overrides.clone(),
        );
        let remove = RemoveProject::new(
            projects.clone(),
            Arc::clone(&history),
            Arc::clone(&runtime),
            DisabledIngress::new(),
            source.clone(),
            secrets.clone(),
            overrides.clone(),
        );
        let probe = HostSystemProbe::new(
            Arc::new(SystemRunner),
            disk,
            projects.clone(),
            env!("CARGO_PKG_VERSION").to_string(),
            85,
            false,
            100,
        );
        let diagnostics = RunDiagnostics::new(probe.clone());
        let agent_status = AgentStatus::new(probe, projects.clone(), Arc::clone(&history));
        let send_secrets = SendSecrets::new(
            secrets.clone(),
            projects,
            source,
            FsSecretsWriter::new(),
            overrides,
            runtime,
        );
        let list_secrets = ListSecrets::new(secrets);
        AppState {
            scheduler,
            list,
            history,
            hub: DeployEventsHub::new(),
            ids: UuidGen::new(),
            send_secrets,
            list_secrets,
            gc,
            stream_logs,
            stats,
            lifecycle,
            commands,
            remove,
            diagnostics,
            agent_status,
            host_network: Arc::new(UdpHostNetwork::new()),
            log_dir: dir.join("logs"),
            log_dir_available: true,
        }
    }

    fn deploy_body(name: &str) -> serde_json::Value {
        serde_json::json!({
            "project": {
                "name": name,
                "repo": "https://github.com/x/y.git",
                "branch": "main",
                "compose": "docker-compose.yml",
                "service": "web",
                "port": 3000,
                "hostname": null
            },
            "ref": null
        })
    }

    async fn request(
        app: Router,
        req: axum::http::Request<axum::body::Body>,
    ) -> (StatusCode, serde_json::Value) {
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, json)
    }

    fn post_json(uri: &str, body: &serde_json::Value) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::post(uri)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    }

    fn get_req(uri: &str) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::get(uri)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    fn post_empty(uri: &str) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::post(uri)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    fn delete_req(uri: &str) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::delete(uri)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn version_handshake() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let (status, json) = request(app, get_req("/v1/version")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["api"], "v1");
    }

    #[tokio::test]
    async fn deploy_end_to_end_with_mocked_docker() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));

        let (status, json) = request(
            app.clone(),
            post_json("/v1/deployments", &deploy_body("rateme")),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{json}");
        let id = json["deployment_id"].as_str().unwrap().to_string();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            assert!(
                tokio::time::Instant::now() < deadline,
                "deploy did not finish in time"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let (status, json) =
                request(app.clone(), get_req(&format!("/v1/deployments/{id}"))).await;
            // 404 is OK briefly while the async deploy task is still starting.
            if status == StatusCode::NOT_FOUND {
                continue;
            }
            assert_eq!(status, StatusCode::OK);
            if json["status"] == "success" {
                assert_eq!(json["commit_sha"], SHA);
                break;
            }
        }

        let (status, json) = request(app.clone(), get_req("/v1/projects")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json[0]["name"], "rateme");
        assert_eq!(json[0]["host_port"], 8000);
        assert!(dir.path().join("overrides").join("rateme.yml").exists());
    }

    #[tokio::test]
    async fn concurrent_deploys_queue_with_latest_wins() {
        struct GatedSource(Arc<tokio::sync::Notify>);

        #[async_trait::async_trait]
        impl Source for GatedSource {
            fn workdir(&self, project_name: &str) -> std::path::PathBuf {
                std::env::temp_dir().join(project_name)
            }

            async fn fetch(
                &self,
                p: &ProjectConfig,
                _r: &DeployRef,
                _l: Arc<dyn LogSink>,
            ) -> Result<FetchedSource, DomainError> {
                self.0.notified().await;
                Ok(FetchedSource {
                    workdir: std::env::temp_dir().join(&p.name),
                    commit_sha: SHA.into(),
                })
            }

            async fn cleanup(&self, _project_name: &str) -> Result<(), DomainError> {
                Ok(())
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let gate = Arc::new(tokio::sync::Notify::new());
        let app = router(state_with(
            dir.path(),
            Arc::new(GatedSource(Arc::clone(&gate))),
            Arc::new(ok_runtime()),
        ));

        let (status, json) = request(
            app.clone(),
            post_json("/v1/deployments", &deploy_body("rateme")),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(json["queued"], false);

        let (status, json) = request(
            app.clone(),
            post_json("/v1/deployments", &deploy_body("rateme")),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{json}");
        assert_eq!(json["queued"], true);
        let superseded_id = json["deployment_id"].as_str().unwrap().to_string();

        let (status, json) = request(
            app.clone(),
            post_json("/v1/deployments", &deploy_body("rateme")),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(json["queued"], true);

        let (status, json) = request(
            app.clone(),
            get_req(&format!("/v1/deployments/{superseded_id}")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "superseded");

        gate.notify_one();
        gate.notify_one();
    }

    #[tokio::test]
    async fn gc_endpoint_reports_disk_and_prune_decision() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let (status, json) = request(app, post_empty("/v1/gc")).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["disk_used_percent"], 10);
        assert_eq!(json["builder_pruned"], false);
    }

    #[tokio::test]
    async fn unknown_deployment_is_404() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let (status, _) = request(app, get_req("/v1/deployments/nope")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn cancel_unknown_deployment_is_404() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let (status, _) = request(app, delete_req("/v1/deployments/nope")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn cancel_finished_deployment_is_409() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let (status, json) = request(
            app.clone(),
            post_json("/v1/deployments", &deploy_body("rateme")),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let id = json["deployment_id"].as_str().unwrap().to_string();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            assert!(tokio::time::Instant::now() < deadline, "deploy hung");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let (_, json) = request(app.clone(), get_req(&format!("/v1/deployments/{id}"))).await;
            if json["status"] == "success" {
                break;
            }
        }

        let (status, json) =
            request(app.clone(), delete_req(&format!("/v1/deployments/{id}"))).await;
        assert_eq!(status, StatusCode::CONFLICT, "{json}");
    }

    #[tokio::test]
    async fn cancel_orphaned_db_row_explains_scheduler_mismatch() {
        // A queued row in the DB with no scheduler entry (e.g. after an agent
        // restart) must produce a 409 that does not claim "already finished".
        let dir = tempfile::tempdir().unwrap();
        let state = state_with(dir.path(), Arc::new(ok_source()), Arc::new(ok_runtime()));
        state
            .history
            .record_queued(&pi_domain::entities::Deployment {
                id: "ghost-q".into(),
                project: "rateme".into(),
                git_ref: "main".into(),
                commit_sha: None,
                status: DeploymentStatus::Queued,
                started_at: 1,
                finished_at: None,
                log_tail: String::new(),
            })
            .await
            .unwrap();
        let app = router(state);

        let (status, json) = request(app, delete_req("/v1/deployments/ghost-q")).await;
        assert_eq!(status, StatusCode::CONFLICT, "{json}");
        let msg = json["error"].as_str().unwrap();
        assert!(msg.contains("not active in the scheduler"), "{msg}");
        assert!(!msg.contains("already finished"), "{msg}");
    }

    #[tokio::test]
    async fn cancel_running_deployment_marks_it_canceled() {
        struct HangingSource;

        #[async_trait::async_trait]
        impl Source for HangingSource {
            fn workdir(&self, project_name: &str) -> std::path::PathBuf {
                std::env::temp_dir().join(project_name)
            }

            async fn fetch(
                &self,
                _p: &ProjectConfig,
                _r: &DeployRef,
                _l: Arc<dyn LogSink>,
            ) -> Result<FetchedSource, DomainError> {
                std::future::pending().await
            }

            async fn cleanup(&self, _project_name: &str) -> Result<(), DomainError> {
                Ok(())
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(HangingSource),
            Arc::new(ok_runtime()),
        ));

        let (status, json) = request(
            app.clone(),
            post_json("/v1/deployments", &deploy_body("rateme")),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let id = json["deployment_id"].as_str().unwrap().to_string();

        let (status, json) = request(
            app.clone(),
            get_req("/v1/projects/rateme/deployments/active"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json[0]["id"], id.as_str(), "{json}");

        let (status, json) =
            request(app.clone(), delete_req(&format!("/v1/deployments/{id}"))).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["status"], "canceling");

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            assert!(
                tokio::time::Instant::now() < deadline,
                "cancel did not land"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let (_, json) = request(app.clone(), get_req(&format!("/v1/deployments/{id}"))).await;
            if json["status"] == "canceled" {
                break;
            }
        }
    }

    fn put_json(uri: &str, body: &serde_json::Value) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::put(uri)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn secrets_send_then_ls_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));

        // "PEM" -> base64 "UEVN"
        let body = serde_json::json!({
            "vars": { "DB_PASSWORD": "hunter2-long" },
            "files": { "certs/server.pem": "UEVN" },
            "apply": false
        });
        let (status, json) =
            request(app.clone(), put_json("/v1/projects/rateme/secrets", &body)).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["saved_keys"], 1);
        assert_eq!(json["saved_files"], 1);
        assert_eq!(json["applied"], false);

        let (status, json) = request(app.clone(), get_req("/v1/projects/rateme/secrets")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["keys"], serde_json::json!(["DB_PASSWORD"]));
        assert_eq!(json["files"], serde_json::json!(["certs/server.pem"]));
    }

    #[tokio::test]
    async fn secrets_send_rejects_bad_paths_base64_and_oversize() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));

        for bad in [
            serde_json::json!({ "vars": {}, "files": { "../escape": "UEVN" } }),
            serde_json::json!({ "vars": {}, "files": { "/abs/path": "UEVN" } }),
            serde_json::json!({ "vars": {}, "files": { "certs\\win.pem": "UEVN" } }),
            serde_json::json!({ "vars": {}, "files": { "ok.pem": "not-base64!!!" } }),
            serde_json::json!({ "vars": { "BAD-DASH": "x" }, "files": {} }),
            serde_json::json!({ "vars": { "OK": "line1\nline2" }, "files": {} }),
        ] {
            let (status, json) =
                request(app.clone(), put_json("/v1/projects/rateme/secrets", &bad)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} -> {json}");
        }

        use base64::Engine as _;
        let big = base64::engine::general_purpose::STANDARD.encode(vec![
            0u8;
            crate::proto::MAX_SECRET_FILE_BYTES
                + 1
        ]);
        let body = serde_json::json!({ "vars": {}, "files": { "big.bin": big } });
        let (status, json) =
            request(app.clone(), put_json("/v1/projects/rateme/secrets", &body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    }

    #[tokio::test]
    async fn secrets_apply_for_unknown_project_is_404() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let body = serde_json::json!({ "vars": { "A_KEY": "value-long-enough" }, "files": {}, "apply": true });
        let (status, _) = request(app, put_json("/v1/projects/ghost/secrets", &body)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn secrets_ls_for_unknown_project_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let (status, json) = request(app, get_req("/v1/projects/ghost/secrets")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["keys"], serde_json::json!([]));
        assert_eq!(json["files"], serde_json::json!([]));
    }

    // The plan's brief text for this test asserts the OLD `/env` routes are
    // gone (404) — but Task 6's own scope note is explicit that `/env` stays
    // registered, unmodified, "exactly as before" until Task 8 removes it;
    // `crates/bin/src/cli/api.rs` (the CLI client) still calls `/env` at
    // runtime and is out of scope for this task. Removing `/env` here would
    // both contradict that stated scope and break the CLI. So this test is
    // adjusted to assert the opposite of its brief name for now: the legacy
    // routes keep working unchanged, side by side with the new `/secrets`
    // route. Task 8 (which does remove `/env`) should flip this back to a
    // 404 check.
    #[tokio::test]
    async fn legacy_env_routes_still_work_pending_task_8_removal() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let body = serde_json::json!({ "vars": { "A": "1" } });
        let (status, json) = request(app.clone(), put_json("/v1/projects/rateme/env", &body)).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        let (status, json) = request(app, get_req("/v1/projects/rateme/env")).await;
        assert_eq!(status, StatusCode::OK, "{json}");
    }

    fn deploy_body_with_commands(name: &str) -> serde_json::Value {
        let mut body = deploy_body(name);
        body["project"]["commands"] = serde_json::json!({
            "create-invite": ["node", "scripts/create-invite.js"]
        });
        body
    }

    /// Deploys and polls until the deployment reaches `success`.
    async fn deploy_and_wait(app: &Router, body: &serde_json::Value) {
        let (status, json) = request(app.clone(), post_json("/v1/deployments", body)).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let id = json["deployment_id"].as_str().unwrap().to_string();
        for _ in 0..100 {
            let (_, d) = request(app.clone(), get_req(&format!("/v1/deployments/{id}"))).await;
            if d["status"] == "success" {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("deployment did not reach success");
    }

    async fn request_text(
        app: Router,
        req: axum::http::Request<axum::body::Body>,
    ) -> (StatusCode, String) {
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn list_commands_returns_deployed_commands() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        deploy_and_wait(&app, &deploy_body_with_commands("rateme")).await;

        let (status, json) = request(app.clone(), get_req("/v1/projects/rateme/commands")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["commands"]["create-invite"],
            serde_json::json!(["node", "scripts/create-invite.js"])
        );

        let (status, _) = request(app, get_req("/v1/projects/ghost/commands")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn run_command_streams_output_and_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let mut runtime = ok_runtime();
        runtime
            .expect_exec()
            .withf(|_, service, argv, _| {
                service == "web"
                    && argv == ["node", "scripts/create-invite.js", "--email", "x@y.com"]
            })
            .returning(|_, _, _, log| {
                log.line("invite created");
                Ok(0)
            });
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(runtime),
        ));
        deploy_and_wait(&app, &deploy_body_with_commands("rateme")).await;

        let (status, body) = request_text(
            app,
            post_json(
                "/v1/projects/rateme/commands/create-invite",
                &serde_json::json!({ "args": ["--email", "x@y.com"] }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("event: log"), "got: {body}");
        assert!(body.contains("invite created"), "got: {body}");
        assert!(body.contains("event: exit"), "got: {body}");
        assert!(body.contains("data: 0"), "got: {body}");
    }

    #[tokio::test]
    async fn run_unknown_command_is_404_with_available_names() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        deploy_and_wait(&app, &deploy_body_with_commands("rateme")).await;

        let (status, json) = request(
            app,
            post_json(
                "/v1/projects/rateme/commands/nope",
                &serde_json::json!({ "args": [] }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let msg = json["error"].as_str().unwrap();
        assert!(msg.contains("create-invite"), "got: {msg}");
    }

    #[tokio::test]
    async fn deploy_rejects_invalid_command_names() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(
            dir.path(),
            Arc::new(ok_source()),
            Arc::new(ok_runtime()),
        ));
        let mut body = deploy_body("rateme");
        body["project"]["commands"] = serde_json::json!({ "Bad Name": ["run"] });
        let (status, _) = request(app.clone(), post_json("/v1/deployments", &body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let mut body = deploy_body("rateme");
        body["project"]["commands"] = serde_json::json!({ "x": [] });
        let (status, _) = request(app, post_json("/v1/deployments", &body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
