use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use pi_domain::entities::{DeployRef, ProjectConfig};
use pi_domain::error::DomainError;
use pi_infrastructure::events::DeployEvent;

use crate::agent::state::AppState;
use crate::proto::{DeployAccepted, DeployRequest, DeploymentDto, ProjectViewDto, VersionInfo};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/version", get(version))
        .route("/v1/deployments", post(create_deployment))
        .route("/v1/deployments/{id}", get(get_deployment))
        .route("/v1/deployments/{id}/logs", get(deployment_logs))
        .route("/v1/projects", get(list_projects))
        .with_state(state)
}

pub struct ApiError(pub DomainError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            DomainError::DeployInProgress(_) => StatusCode::CONFLICT,
            DomainError::NotFound(_) => StatusCode::NOT_FOUND,
            DomainError::Invalid(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(serde_json::json!({ "error": self.0.to_string() }))).into_response()
    }
}

async fn version() -> Json<VersionInfo> {
    Json(VersionInfo { version: env!("CARGO_PKG_VERSION").to_string(), api: "v1".to_string() })
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
        return Err(ApiError(DomainError::Invalid("project.port must be > 0".into())));
    }
    let git_ref = DeployRef::parse(req.git_ref.as_deref().unwrap_or(&config.branch));

    let permit = state.deploy.try_begin(&config.name).map_err(ApiError)?;
    let deployment_id = state.ids.new_id();
    let sink = state.hub.register(&deployment_id);

    let deploy = Arc::clone(&state.deploy);
    let id = deployment_id.clone();
    tokio::spawn(async move {
        if let Err(err) = deploy.execute(permit, id, config, git_ref, sink).await {
            tracing::warn!("deploy failed: {err}");
        }
    });

    Ok((StatusCode::ACCEPTED, Json(DeployAccepted { deployment_id })).into_response())
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

async fn list_projects(State(state): State<AppState>) -> Result<Json<Vec<ProjectViewDto>>, ApiError> {
    let views = state.list.execute().await.map_err(ApiError)?;
    Ok(Json(views.into_iter().map(Into::into).collect()))
}

fn sse_log(line: String) -> Result<Event, Infallible> {
    Ok(Event::default().event("log").data(line))
}

fn sse_finished(status: &str) -> Result<Event, Infallible> {
    Ok(Event::default().event("finished").data(status.to_string()))
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
            if let Some(status) = sub.finished {
                yield sse_finished(status.as_str());
                return;
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
        return Ok(Sse::new(stream).keep_alive(KeepAlive::default()).into_response());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::state::AppState;
    use http_body_util::BodyExt;
    use pi_application::deploy::DeployProject;
    use pi_application::list::ListProjects;
    use pi_domain::contracts::{ContainerRuntime, LogSink, MockContainerRuntime, MockSource, Source};
    use pi_domain::entities::FetchedSource;
    use pi_infrastructure::events::DeployEventsHub;
    use pi_infrastructure::history::SqliteHistory;
    use pi_infrastructure::overrides::FsOverrideStore;
    use pi_infrastructure::repo::SqliteProjectRepo;
    use pi_infrastructure::sqlite::Db;
    use pi_infrastructure::sys::{SystemClock, UuidGen};
    use tower::ServiceExt;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn ok_source() -> MockSource {
        let mut source = MockSource::new();
        source.expect_fetch().returning(|p, _, _| {
            Ok(FetchedSource { workdir: std::env::temp_dir().join(&p.name), commit_sha: SHA.into() })
        });
        source
    }

    fn ok_runtime() -> MockContainerRuntime {
        let mut runtime = MockContainerRuntime::new();
        runtime.expect_build().returning(|_, _| Ok(()));
        runtime.expect_up().returning(|_, _| Ok(()));
        runtime.expect_ps().returning(|_| Ok(vec![]));
        runtime
    }

    fn state_with(
        dir: &std::path::Path,
        source: Arc<dyn Source>,
        runtime: Arc<dyn ContainerRuntime>,
    ) -> AppState {
        let db = Db::open(&dir.join("state.db")).unwrap();
        let projects = SqliteProjectRepo::new(db.clone(), 8000, 8999);
        let history: Arc<dyn pi_domain::contracts::DeploymentHistory> = SqliteHistory::new(db);
        let overrides = FsOverrideStore::new(dir.join("overrides"));
        let deploy = DeployProject::new(
            source,
            Arc::clone(&runtime),
            projects.clone(),
            Arc::clone(&history),
            overrides,
            SystemClock::new(),
        );
        let list = ListProjects::new(projects, runtime);
        AppState { deploy, list, history, hub: DeployEventsHub::new(), ids: UuidGen::new() }
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

    async fn request(app: Router, req: axum::http::Request<axum::body::Body>) -> (StatusCode, serde_json::Value) {
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
        axum::http::Request::get(uri).body(axum::body::Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn version_handshake() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(dir.path(), Arc::new(ok_source()), Arc::new(ok_runtime())));
        let (status, json) = request(app, get_req("/v1/version")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["api"], "v1");
    }

    #[tokio::test]
    async fn deploy_end_to_end_with_mocked_docker() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(dir.path(), Arc::new(ok_source()), Arc::new(ok_runtime())));

        let (status, json) = request(app.clone(), post_json("/v1/deployments", &deploy_body("rateme"))).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{json}");
        let id = json["deployment_id"].as_str().unwrap().to_string();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            assert!(tokio::time::Instant::now() < deadline, "deploy did not finish in time");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let (status, json) = request(app.clone(), get_req(&format!("/v1/deployments/{id}"))).await;
            // 404 is OK briefly while record_started hasn't committed yet
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
    async fn concurrent_deploy_of_same_project_is_409() {
        struct GatedSource(Arc<tokio::sync::Notify>);

        #[async_trait::async_trait]
        impl Source for GatedSource {
            async fn fetch(
                &self,
                p: &ProjectConfig,
                _r: &DeployRef,
                _l: Arc<dyn LogSink>,
            ) -> Result<FetchedSource, DomainError> {
                self.0.notified().await;
                Ok(FetchedSource { workdir: std::env::temp_dir().join(&p.name), commit_sha: SHA.into() })
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let gate = Arc::new(tokio::sync::Notify::new());
        let app = router(state_with(dir.path(), Arc::new(GatedSource(Arc::clone(&gate))), Arc::new(ok_runtime())));

        let (status, _) = request(app.clone(), post_json("/v1/deployments", &deploy_body("rateme"))).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let (status, json) = request(app.clone(), post_json("/v1/deployments", &deploy_body("rateme"))).await;
        assert_eq!(status, StatusCode::CONFLICT, "{json}");
        gate.notify_one();
    }

    #[tokio::test]
    async fn unknown_deployment_is_404() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(state_with(dir.path(), Arc::new(ok_source()), Arc::new(ok_runtime())));
        let (status, _) = request(app, get_req("/v1/deployments/nope")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
