use std::collections::BTreeMap;

use pi_application::list::ProjectView;
use pi_domain::entities::{Deployment, HealthcheckConfig, ProjectConfig, StageTimeoutOverrides};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub api: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthcheckDto {
    pub path: Option<String>,
    pub expect: Option<String>,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutsDto {
    pub fetch_secs: Option<u64>,
    pub build_secs: Option<u64>,
    pub up_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectDto {
    pub name: String,
    pub repo: String,
    pub branch: String,
    pub compose: String,
    pub service: String,
    pub port: u16,
    pub hostname: Option<String>,
    #[serde(default)]
    pub healthcheck: Option<HealthcheckDto>,
    #[serde(default)]
    pub timeouts: Option<TimeoutsDto>,
}

impl From<ProjectDto> for ProjectConfig {
    fn from(dto: ProjectDto) -> ProjectConfig {
        ProjectConfig {
            name: dto.name,
            repo: dto.repo,
            branch: dto.branch,
            compose_path: dto.compose,
            service: dto.service,
            container_port: dto.port,
            hostname: dto.hostname,
            healthcheck: dto
                .healthcheck
                .map(|h| HealthcheckConfig {
                    path: h.path,
                    expect: h.expect,
                    timeout_secs: h.timeout_secs.unwrap_or(60),
                })
                .unwrap_or_default(),
            timeouts: dto
                .timeouts
                .map(|t| StageTimeoutOverrides {
                    fetch_secs: t.fetch_secs,
                    build_secs: t.build_secs,
                    up_secs: t.up_secs,
                })
                .unwrap_or_default(),
        }
    }
}

impl From<&ProjectConfig> for ProjectDto {
    fn from(config: &ProjectConfig) -> ProjectDto {
        ProjectDto {
            name: config.name.clone(),
            repo: config.repo.clone(),
            branch: config.branch.clone(),
            compose: config.compose_path.clone(),
            service: config.service.clone(),
            port: config.container_port,
            hostname: config.hostname.clone(),
            healthcheck: Some(HealthcheckDto {
                path: config.healthcheck.path.clone(),
                expect: config.healthcheck.expect.clone(),
                timeout_secs: Some(config.healthcheck.timeout_secs),
            }),
            timeouts: Some(TimeoutsDto {
                fetch_secs: config.timeouts.fetch_secs,
                build_secs: config.timeouts.build_secs,
                up_secs: config.timeouts.up_secs,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSendRequest {
    pub vars: BTreeMap<String, String>,
    #[serde(default)]
    pub apply: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSendResponse {
    pub saved_keys: usize,
    pub applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvKeysResponse {
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcResponse {
    pub disk_used_percent: u8,
    pub builder_pruned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployRequest {
    pub project: ProjectDto,
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployAccepted {
    pub deployment_id: String,
    /// true when the deploy waits behind an active one (latest wins, §8.1).
    #[serde(default)]
    pub queued: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentDto {
    pub id: String,
    pub project: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub commit_sha: Option<String>,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub log_tail: String,
}

impl From<Deployment> for DeploymentDto {
    fn from(d: Deployment) -> DeploymentDto {
        DeploymentDto {
            id: d.id,
            project: d.project,
            git_ref: d.git_ref,
            commit_sha: d.commit_sha,
            status: d.status.as_str().to_string(),
            started_at: d.started_at,
            finished_at: d.finished_at,
            log_tail: d.log_tail,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStateDto {
    pub service: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectViewDto {
    pub name: String,
    pub repo: String,
    pub branch: String,
    pub hostname: Option<String>,
    pub host_port: u16,
    pub services: Vec<ServiceStateDto>,
}

impl From<ProjectView> for ProjectViewDto {
    fn from(v: ProjectView) -> ProjectViewDto {
        ProjectViewDto {
            name: v.name,
            repo: v.repo,
            branch: v.branch,
            hostname: v.hostname,
            host_port: v.host_port,
            services: v
                .services
                .into_iter()
                .map(|s| ServiceStateDto {
                    service: s.service,
                    state: s.state,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v01_deploy_request_without_healthcheck_still_deserializes() {
        let json = r#"{"project":{"name":"a","repo":"r","branch":"main","compose":"docker-compose.yml","service":"web","port":3000,"hostname":null},"ref":null}"#;
        let req: DeployRequest = serde_json::from_str(json).unwrap();
        let config: ProjectConfig = req.project.into();
        assert_eq!(config.healthcheck.timeout_secs, 60);
    }

    #[test]
    fn healthcheck_roundtrips_through_dto() {
        let mut config: ProjectConfig = ProjectDto {
            name: "a".into(),
            repo: "r".into(),
            branch: "main".into(),
            compose: "docker-compose.yml".into(),
            service: "web".into(),
            port: 3000,
            hostname: None,
            healthcheck: None,
            timeouts: None,
        }
        .into();
        config.healthcheck.path = Some("/health".into());
        config.healthcheck.timeout_secs = 120;
        let dto = ProjectDto::from(&config);
        let back: ProjectConfig = dto.into();
        assert_eq!(back.healthcheck, config.healthcheck);
    }

    #[test]
    fn timeouts_roundtrip_through_dto_and_default_when_absent() {
        let json = r#"{"project":{"name":"a","repo":"r","branch":"main","compose":"docker-compose.yml","service":"web","port":3000,"hostname":null},"ref":null}"#;
        let req: DeployRequest = serde_json::from_str(json).unwrap();
        let config: ProjectConfig = req.project.into();
        assert_eq!(
            config.timeouts,
            Default::default(),
            "v0.2 payloads still work"
        );

        let mut config = config;
        config.timeouts.build_secs = Some(3600);
        let dto = ProjectDto::from(&config);
        let back: ProjectConfig = dto.into();
        assert_eq!(back.timeouts.build_secs, Some(3600));
    }
}
