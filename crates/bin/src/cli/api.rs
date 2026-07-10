use futures::StreamExt;

use std::collections::BTreeMap;

use crate::cli::sse::SseParser;
use crate::proto::{
    AgentOverviewDto, CommandRunRequest, CommandsResponse, DeployAccepted, DeployRequest,
    DeploymentDto, DiagnosticReportDto, GcResponse, LifecycleResponse, ProjectViewDto,
    RemoveResponse, SecretsListResponse, SecretsSendRequest, SecretsSendResponse, StatsReportDto,
    VersionInfo,
};

#[derive(Debug, serde::Deserialize)]
pub struct StageEventDto {
    pub stage: String,
    pub status: String,
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
}

pub enum DeployStreamEvent<'a> {
    Line(&'a str),
    Stage(StageEventDto),
    Summary { services: usize },
}

fn parse_stage(data: &str) -> Option<StageEventDto> {
    serde_json::from_str(data).ok()
}

fn parse_summary(data: &str) -> Option<usize> {
    #[derive(serde::Deserialize)]
    struct SummaryDto {
        services: usize,
    }
    serde_json::from_str::<SummaryDto>(data)
        .ok()
        .map(|s| s.services)
}

async fn extract_error(resp: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status();
    let msg = resp
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string))
        .unwrap_or_else(|| status.to_string());
    anyhow::bail!("{msg}")
}

pub struct ApiClient {
    http: reqwest::Client,
    base: String,
}

impl ApiClient {
    pub fn new(base: String) -> ApiClient {
        ApiClient {
            http: reqwest::Client::new(),
            base,
        }
    }

    pub async fn version(&self) -> anyhow::Result<VersionInfo> {
        let resp = self
            .http
            .get(format!("{}/v1/version", self.base))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            anyhow::bail!("agent does not expose /v1 — incompatible agent; update it on the Pi");
        }
        Ok(extract_error(resp).await?.json().await?)
    }

    pub async fn deploy(&self, req: &DeployRequest) -> anyhow::Result<DeployAccepted> {
        let resp = self
            .http
            .post(format!("{}/v1/deployments", self.base))
            .json(req)
            .send()
            .await?;
        Ok(extract_error(resp).await?.json().await?)
    }

    pub async fn active_deployments(&self, project: &str) -> anyhow::Result<Vec<DeploymentDto>> {
        let resp = self
            .http
            .get(format!(
                "{}/v1/projects/{project}/deployments/active",
                self.base
            ))
            .send()
            .await?;
        Ok(extract_error(resp).await?.json().await?)
    }

    pub async fn cancel_deployment(&self, id: &str) -> anyhow::Result<String> {
        let resp = self
            .http
            .delete(format!("{}/v1/deployments/{id}", self.base))
            .send()
            .await?;
        let json: serde_json::Value = extract_error(resp).await?.json().await?;
        Ok(json["status"].as_str().unwrap_or("unknown").to_string())
    }

    pub async fn gc(&self) -> anyhow::Result<GcResponse> {
        let resp = self
            .http
            .post(format!("{}/v1/gc", self.base))
            .send()
            .await?;
        Ok(extract_error(resp).await?.json().await?)
    }

    pub async fn stats(&self, project: Option<&str>) -> anyhow::Result<StatsReportDto> {
        let url = match project {
            Some(project) => format!("{}/v1/stats?project={project}", self.base),
            None => format!("{}/v1/stats", self.base),
        };
        let resp = self.http.get(url).send().await?;
        Ok(extract_error(resp).await?.json().await?)
    }

    pub async fn agent_status(&self) -> anyhow::Result<AgentOverviewDto> {
        let resp = self
            .http
            .get(format!("{}/v1/status", self.base))
            .send()
            .await?;
        Ok(extract_error(resp).await?.json().await?)
    }

    pub async fn doctor(&self) -> anyhow::Result<DiagnosticReportDto> {
        let resp = self
            .http
            .get(format!("{}/v1/doctor", self.base))
            .send()
            .await?;
        Ok(extract_error(resp).await?.json().await?)
    }

    pub async fn lifecycle(
        &self,
        project: &str,
        action: &str,
    ) -> anyhow::Result<LifecycleResponse> {
        let resp = self
            .http
            .post(format!(
                "{}/v1/projects/{project}/lifecycle/{action}",
                self.base
            ))
            .send()
            .await?;
        Ok(extract_error(resp).await?.json().await?)
    }

    pub async fn remove_project(
        &self,
        project: &str,
        volumes: bool,
    ) -> anyhow::Result<RemoveResponse> {
        let resp = self
            .http
            .delete(format!(
                "{}/v1/projects/{project}?volumes={volumes}",
                self.base
            ))
            .send()
            .await?;
        Ok(extract_error(resp).await?.json().await?)
    }

    pub async fn follow_logs(
        &self,
        id: &str,
        mut on_event: impl FnMut(DeployStreamEvent<'_>),
    ) -> anyhow::Result<String> {
        let resp = self
            .http
            .get(format!("{}/v1/deployments/{id}/logs", self.base))
            .send()
            .await?;
        let resp = extract_error(resp).await?;
        let mut stream = resp.bytes_stream();
        let mut parser = SseParser::default();
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            buf.extend_from_slice(&chunk?);
            let valid_up_to = match std::str::from_utf8(&buf) {
                Ok(_) => buf.len(),
                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                Err(_) => buf.len(),
            };
            if valid_up_to == 0 {
                continue;
            }
            let text = String::from_utf8_lossy(&buf[..valid_up_to]).into_owned();
            buf.drain(..valid_up_to);
            for ev in parser.push(&text) {
                match ev.event.as_str() {
                    "log" => on_event(DeployStreamEvent::Line(&ev.data)),
                    "stage" => {
                        if let Some(dto) = parse_stage(&ev.data) {
                            on_event(DeployStreamEvent::Stage(dto));
                        }
                    }
                    "summary" => {
                        if let Some(services) = parse_summary(&ev.data) {
                            on_event(DeployStreamEvent::Summary { services });
                        }
                    }
                    "finished" => return Ok(ev.data),
                    _ => {}
                }
            }
        }
        anyhow::bail!("log stream ended without a final status (agent restarted?)")
    }

    pub async fn stream_sse(
        &self,
        query: &str,
        mut on_line: impl FnMut(&str),
    ) -> anyhow::Result<()> {
        let resp = self
            .http
            .get(format!("{}{}", self.base, query))
            .send()
            .await?;
        let resp = extract_error(resp).await?;
        let mut stream = resp.bytes_stream();
        let mut parser = SseParser::default();
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            buf.extend_from_slice(&chunk?);
            let valid_up_to = match std::str::from_utf8(&buf) {
                Ok(_) => buf.len(),
                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                Err(_) => buf.len(),
            };
            if valid_up_to == 0 {
                continue;
            }
            let text = String::from_utf8_lossy(&buf[..valid_up_to]).into_owned();
            buf.drain(..valid_up_to);
            for ev in parser.push(&text) {
                if ev.event == "log" {
                    on_line(&ev.data);
                }
            }
        }
        Ok(())
    }

    pub async fn projects(&self) -> anyhow::Result<Vec<ProjectViewDto>> {
        let resp = self
            .http
            .get(format!("{}/v1/projects", self.base))
            .send()
            .await?;
        Ok(extract_error(resp).await?.json().await?)
    }

    pub async fn send_secrets(
        &self,
        project: &str,
        vars: BTreeMap<String, String>,
        files: BTreeMap<String, String>,
        apply: bool,
    ) -> anyhow::Result<SecretsSendResponse> {
        let req = SecretsSendRequest { vars, files, apply };
        let resp = self
            .http
            .put(format!("{}/v1/projects/{project}/secrets", self.base))
            .json(&req)
            .send()
            .await?;
        Ok(extract_secrets_error(resp).await?.json().await?)
    }

    pub async fn list_secrets(&self, project: &str) -> anyhow::Result<SecretsListResponse> {
        let resp = self
            .http
            .get(format!("{}/v1/projects/{project}/secrets", self.base))
            .send()
            .await?;
        Ok(extract_secrets_error(resp).await?.json().await?)
    }

    /// 404 on this route can mean two very different things: an old agent
    /// without the feature (bare 404, no JSON body) or a domain "not found"
    /// (JSON error). Distinguish them for a usable message.
    async fn commands_not_found(resp: reqwest::Response) -> anyhow::Error {
        match resp
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_string))
        {
            Some(msg) => anyhow::anyhow!("{msg}"),
            None => {
                anyhow::anyhow!("agent does not support [commands]; update rpi-agent on the Pi")
            }
        }
    }

    pub async fn list_commands(&self, project: &str) -> anyhow::Result<CommandsResponse> {
        let resp = self
            .http
            .get(format!("{}/v1/projects/{project}/commands", self.base))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Self::commands_not_found(resp).await);
        }
        Ok(extract_error(resp).await?.json().await?)
    }

    /// Streams command output; returns the in-container exit code.
    pub async fn run_command(
        &self,
        project: &str,
        command: &str,
        args: &[String],
        mut on_line: impl FnMut(&str),
    ) -> anyhow::Result<i32> {
        let resp = self
            .http
            .post(format!(
                "{}/v1/projects/{project}/commands/{command}",
                self.base
            ))
            .json(&CommandRunRequest {
                args: args.to_vec(),
            })
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Self::commands_not_found(resp).await);
        }
        let resp = extract_error(resp).await?;
        let mut stream = resp.bytes_stream();
        let mut parser = SseParser::default();
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            buf.extend_from_slice(&chunk?);
            let valid_up_to = match std::str::from_utf8(&buf) {
                Ok(_) => buf.len(),
                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                Err(_) => buf.len(),
            };
            if valid_up_to == 0 {
                continue;
            }
            let text = String::from_utf8_lossy(&buf[..valid_up_to]).into_owned();
            buf.drain(..valid_up_to);
            for ev in parser.push(&text) {
                match ev.event.as_str() {
                    "log" => on_line(&ev.data),
                    "exit" => {
                        return ev.data.trim().parse::<i32>().map_err(|_| {
                            anyhow::anyhow!("agent sent invalid exit code '{}'", ev.data)
                        })
                    }
                    _ => {}
                }
            }
        }
        anyhow::bail!("command stream ended without an exit status (agent restarted?)")
    }
}

/// Old agents have no /secrets route. axum's bare 404 carries no {"error"}
/// JSON body (every rpi-agent error does), so an error-less 404 means
/// "route not found" -> the agent predates the secrets API.
async fn extract_secrets_error(resp: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        let bytes = resp.bytes().await.unwrap_or_default();
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            if let Some(msg) = v["error"].as_str() {
                anyhow::bail!("{msg}");
            }
        }
        anyhow::bail!("agent does not support the secrets API; update the agent on the Pi");
    }
    extract_error(resp).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};
    use axum::Router;

    /// Binds an ephemeral port, serves `app` in the background, and returns
    /// the base URL to point an `ApiClient` at.
    async fn spawn_app(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    async fn sse_log_and_exit() -> impl IntoResponse {
        (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            "event: log\ndata: hello\n\nevent: exit\ndata: 7\n\n",
        )
    }

    async fn sse_no_exit() -> impl IntoResponse {
        (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            "event: log\ndata: hello\n\n",
        )
    }

    async fn sse_bad_exit() -> impl IntoResponse {
        (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            "event: exit\ndata: not-a-number\n\n",
        )
    }

    /// Splits the two-byte UTF-8 encoding of 'é' (0xC3 0xA9) across two
    /// separately-flushed chunks, exercising the `error_len().is_none()`
    /// (incomplete-sequence-at-end) branch in the client's chunk decoder.
    async fn sse_multibyte_split() -> Response {
        let stream = async_stream::stream! {
            yield Ok::<Vec<u8>, std::convert::Infallible>(b"event: log\ndata: h\xC3".to_vec());
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            yield Ok::<Vec<u8>, std::convert::Infallible>(
                b"\xA9llo\n\nevent: exit\ndata: 0\n\n".to_vec(),
            );
        };
        Response::builder()
            .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
            .body(axum::body::Body::from_stream(stream))
            .unwrap()
    }

    async fn not_found_plain() -> impl IntoResponse {
        StatusCode::NOT_FOUND
    }

    async fn not_found_json() -> impl IntoResponse {
        (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "error": "command 'nope' not found; available: create-invite"
            })),
        )
    }

    #[tokio::test]
    async fn run_command_streams_log_and_returns_exit_code() {
        let app = Router::new().route("/v1/projects/demo/commands/build", post(sse_log_and_exit));
        let client = ApiClient::new(spawn_app(app).await);

        let mut lines = Vec::new();
        let code = client
            .run_command("demo", "build", &[], |l| lines.push(l.to_string()))
            .await
            .unwrap();

        assert_eq!(code, 7);
        assert_eq!(lines, vec!["hello".to_string()]);
    }

    #[tokio::test]
    async fn run_command_decodes_multibyte_utf8_split_across_chunks() {
        let app = Router::new().route(
            "/v1/projects/demo/commands/build",
            post(sse_multibyte_split),
        );
        let client = ApiClient::new(spawn_app(app).await);

        let mut lines = Vec::new();
        let code = client
            .run_command("demo", "build", &[], |l| lines.push(l.to_string()))
            .await
            .unwrap();

        assert_eq!(code, 0);
        assert_eq!(lines, vec!["héllo".to_string()]);
    }

    #[tokio::test]
    async fn run_command_errors_if_stream_ends_without_exit() {
        let app = Router::new().route("/v1/projects/demo/commands/build", post(sse_no_exit));
        let client = ApiClient::new(spawn_app(app).await);

        let err = client
            .run_command("demo", "build", &[], |_| {})
            .await
            .unwrap_err();

        assert!(
            err.to_string().contains("without an exit status"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn run_command_errors_on_unparseable_exit_code() {
        let app = Router::new().route("/v1/projects/demo/commands/build", post(sse_bad_exit));
        let client = ApiClient::new(spawn_app(app).await);

        let err = client
            .run_command("demo", "build", &[], |_| {})
            .await
            .unwrap_err();

        assert!(err.to_string().contains("invalid exit code"), "got: {err}");
    }

    #[tokio::test]
    async fn list_commands_404_without_body_prompts_agent_update() {
        let app = Router::new().route("/v1/projects/demo/commands", get(not_found_plain));
        let client = ApiClient::new(spawn_app(app).await);

        let err = client.list_commands("demo").await.unwrap_err();

        assert!(err.to_string().contains("update rpi-agent"), "got: {err}");
    }

    #[tokio::test]
    async fn list_commands_404_with_json_error_uses_message_verbatim() {
        let app = Router::new().route("/v1/projects/demo/commands", get(not_found_json));
        let client = ApiClient::new(spawn_app(app).await);

        let err = client.list_commands("demo").await.unwrap_err();

        assert_eq!(
            err.to_string(),
            "command 'nope' not found; available: create-invite"
        );
    }

    #[test]
    fn parse_stage_accepts_valid_and_rejects_malformed_payloads() {
        let ev = parse_stage(r#"{"stage":"build","status":"ok","elapsed_ms":48231}"#).unwrap();
        assert_eq!(ev.stage, "build");
        assert_eq!(ev.status, "ok");
        assert_eq!(ev.elapsed_ms, Some(48231));

        let started = parse_stage(r#"{"stage":"fetch","status":"started"}"#).unwrap();
        assert_eq!(started.elapsed_ms, None);

        assert!(parse_stage("not json").is_none());
        assert!(
            parse_stage(r#"{"status":"ok"}"#).is_none(),
            "missing stage field"
        );
    }

    #[test]
    fn parse_summary_accepts_valid_and_rejects_malformed_payloads() {
        assert_eq!(parse_summary(r#"{"services":2}"#), Some(2));
        assert_eq!(parse_summary("nope"), None);
    }
}
