use pi_application::list::ProjectView;
use pi_domain::entities::{Deployment, HealthcheckConfig, ProjectConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub api: String,
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
            healthcheck: HealthcheckConfig::default(),
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
        }
    }
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
                .map(|s| ServiceStateDto { service: s.service, state: s.state })
                .collect(),
        }
    }
}
