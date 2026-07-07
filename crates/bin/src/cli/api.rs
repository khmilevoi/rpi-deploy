use futures::StreamExt;

use std::collections::BTreeMap;

use crate::cli::sse::SseParser;
use crate::proto::{
    AgentOverviewDto, CommandRunRequest, CommandsResponse, DeployAccepted, DeployRequest,
    DeploymentDto, DiagnosticReportDto, EnvKeysResponse, EnvSendRequest, EnvSendResponse,
    GcResponse, LifecycleResponse, ProjectViewDto, RemoveResponse, StatsReportDto, VersionInfo,
};

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
        mut on_line: impl FnMut(&str),
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
                    "log" => on_line(&ev.data),
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

    pub async fn send_env(
        &self,
        project: &str,
        vars: BTreeMap<String, String>,
        apply: bool,
    ) -> anyhow::Result<EnvSendResponse> {
        let req = EnvSendRequest { vars, apply };
        let resp = self
            .http
            .put(format!("{}/v1/projects/{project}/env", self.base))
            .json(&req)
            .send()
            .await?;
        Ok(extract_error(resp).await?.json().await?)
    }

    pub async fn env_keys(&self, project: &str) -> anyhow::Result<EnvKeysResponse> {
        let resp = self
            .http
            .get(format!("{}/v1/projects/{project}/env", self.base))
            .send()
            .await?;
        Ok(extract_error(resp).await?.json().await?)
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
            None => anyhow::anyhow!(
                "agent does not support [commands]; update rpi-agent on the Pi"
            ),
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
