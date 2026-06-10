use futures::StreamExt;

use crate::cli::sse::SseParser;
use crate::proto::{DeployAccepted, DeployRequest, ProjectViewDto, VersionInfo};

pub struct ApiClient {
    http: reqwest::Client,
    base: String,
}

impl ApiClient {
    pub fn new(base: String) -> ApiClient {
        ApiClient { http: reqwest::Client::new(), base }
    }

    pub async fn version(&self) -> anyhow::Result<VersionInfo> {
        let resp = self.http.get(format!("{}/v1/version", self.base)).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            anyhow::bail!("agent does not expose /v1 — incompatible agent; update it on the Pi");
        }
        Ok(resp.error_for_status()?.json().await?)
    }

    pub async fn deploy(&self, req: &DeployRequest) -> anyhow::Result<DeployAccepted> {
        let resp = self.http.post(format!("{}/v1/deployments", self.base)).json(req).send().await?;
        if resp.status() == reqwest::StatusCode::CONFLICT {
            anyhow::bail!("deploy of this project is already in progress on the agent");
        }
        Ok(resp.error_for_status()?.json().await?)
    }

    pub async fn follow_logs(&self, id: &str, mut on_line: impl FnMut(&str)) -> anyhow::Result<String> {
        let resp = self
            .http
            .get(format!("{}/v1/deployments/{id}/logs", self.base))
            .send()
            .await?
            .error_for_status()?;
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

    pub async fn projects(&self) -> anyhow::Result<Vec<ProjectViewDto>> {
        let resp = self.http.get(format!("{}/v1/projects", self.base)).send().await?;
        Ok(resp.error_for_status()?.json().await?)
    }
}
