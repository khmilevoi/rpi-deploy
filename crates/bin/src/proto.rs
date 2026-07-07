use std::collections::BTreeMap;

use pi_application::list::ProjectView;
use pi_application::remove::RemoveReport;
use pi_domain::entities::{
    AgentOverview, Deployment, DiagnosticCheck, DiagnosticReport, ExposeMode, HealthcheckConfig,
    HostStats, ProjectConfig, ProjectStats, ServiceStats, StageTimeoutOverrides, StatsReport,
};
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
    pub expose: Option<String>,
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
            expose: dto
                .expose
                .as_deref()
                .and_then(ExposeMode::parse)
                .unwrap_or_default(),
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
            expose: Some(config.expose.as_str().to_string()),
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

/// Legacy `/env` route DTOs (agent/http.rs). The CLI no longer sends these
/// (Task 8 replaced `rpi env send/ls` with `rpi secrets send/ls`), but the
/// agent's `/env` route handlers still deserialize/serialize them to keep
/// talking to not-yet-upgraded old CLIs; kept only for that.
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

/// Secrets bundle limits, enforced by the CLI before upload and re-checked
/// by the agent (secrets spec §2.7). Decoded byte sizes.
pub const MAX_SECRET_FILE_BYTES: usize = 1024 * 1024;
pub const MAX_SECRETS_BUNDLE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretsSendRequest {
    pub vars: BTreeMap<String, String>,
    /// Relative path (forward slashes) -> base64-encoded contents.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub apply: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretsSendResponse {
    pub saved_keys: usize,
    pub saved_files: usize,
    pub applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretsListResponse {
    pub keys: Vec<String>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcResponse {
    pub disk_used_percent: u8,
    pub builder_pruned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatsDto {
    pub service: String,
    pub cpu_percent: f64,
    pub mem_used_bytes: u64,
    pub mem_limit_bytes: u64,
}

impl From<ServiceStats> for ServiceStatsDto {
    fn from(s: ServiceStats) -> ServiceStatsDto {
        ServiceStatsDto {
            service: s.service,
            cpu_percent: s.cpu_percent,
            mem_used_bytes: s.mem_used_bytes,
            mem_limit_bytes: s.mem_limit_bytes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostStatsDto {
    pub cpu_percent: f64,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
    pub disk_used_percent: u8,
    pub uptime_secs: u64,
}

impl From<HostStats> for HostStatsDto {
    fn from(h: HostStats) -> HostStatsDto {
        HostStatsDto {
            cpu_percent: h.cpu_percent,
            mem_used_bytes: h.mem_used_bytes,
            mem_total_bytes: h.mem_total_bytes,
            disk_used_percent: h.disk_used_percent,
            uptime_secs: h.uptime_secs,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectStatsDto {
    pub project: String,
    pub services: Vec<ServiceStatsDto>,
    pub last_deploy: Option<DeploymentDto>,
}

impl From<ProjectStats> for ProjectStatsDto {
    fn from(p: ProjectStats) -> ProjectStatsDto {
        ProjectStatsDto {
            project: p.project,
            services: p.services.into_iter().map(Into::into).collect(),
            last_deploy: p.last_deploy.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsReportDto {
    pub host: HostStatsDto,
    pub projects: Vec<ProjectStatsDto>,
}

impl From<StatsReport> for StatsReportDto {
    fn from(r: StatsReport) -> StatsReportDto {
        StatsReportDto {
            host: r.host.into(),
            projects: r.projects.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentOverviewDto {
    pub version: String,
    pub uptime_secs: u64,
    pub disk_used_percent: u8,
    pub projects: usize,
    pub active_deployments: usize,
}

impl From<AgentOverview> for AgentOverviewDto {
    fn from(a: AgentOverview) -> AgentOverviewDto {
        AgentOverviewDto {
            version: a.version,
            uptime_secs: a.uptime_secs,
            disk_used_percent: a.disk_used_percent,
            projects: a.projects,
            active_deployments: a.active_deployments,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticCheckDto {
    pub name: String,
    pub passed: bool,
    pub detail: String,
    pub hint: Option<String>,
}

impl From<DiagnosticCheck> for DiagnosticCheckDto {
    fn from(c: DiagnosticCheck) -> DiagnosticCheckDto {
        DiagnosticCheckDto {
            name: c.name,
            passed: c.passed,
            detail: c.detail,
            hint: c.hint,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticReportDto {
    pub checks: Vec<DiagnosticCheckDto>,
}

impl From<DiagnosticReport> for DiagnosticReportDto {
    fn from(r: DiagnosticReport) -> DiagnosticReportDto {
        DiagnosticReportDto {
            checks: r.checks.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleResponse {
    pub project: String,
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveResponse {
    pub project: String,
    pub hostname: Option<String>,
    pub volumes_removed: bool,
}

impl From<RemoveReport> for RemoveResponse {
    fn from(r: RemoveReport) -> RemoveResponse {
        RemoveResponse {
            project: r.project,
            hostname: r.hostname,
            volumes_removed: r.volumes_removed,
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
    pub expose: String,
    /// Detected LAN ip for expose=lan projects; absent for private or when the
    /// agent could not detect one. `#[serde(default)]` keeps legacy agents
    /// (which never send this field) deserializable.
    #[serde(default)]
    pub lan_ip: Option<String>,
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
            expose: v.expose.as_str().to_string(),
            lan_ip: v.lan_ip.map(|ip| ip.to_string()),
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
            expose: None,
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

    #[test]
    fn expose_roundtrips_and_defaults_private_when_absent() {
        // legacy payload without `expose` -> Private
        let json = r#"{"project":{"name":"a","repo":"r","branch":"main","compose":"docker-compose.yml","service":"web","port":3000,"hostname":null},"ref":null}"#;
        let req: DeployRequest = serde_json::from_str(json).unwrap();
        let config: ProjectConfig = req.project.into();
        assert_eq!(config.expose, pi_domain::entities::ExposeMode::Private);

        // lan roundtrip
        let mut config = config;
        config.expose = pi_domain::entities::ExposeMode::Lan;
        let dto = ProjectDto::from(&config);
        assert_eq!(dto.expose.as_deref(), Some("lan"));
        let back: ProjectConfig = dto.into();
        assert_eq!(back.expose, pi_domain::entities::ExposeMode::Lan);
    }

    #[test]
    fn project_view_dto_exposes_expose_mode_string() {
        let view = ProjectView {
            name: "a".into(),
            repo: "r".into(),
            branch: "main".into(),
            hostname: None,
            host_port: 8000,
            expose: pi_domain::entities::ExposeMode::Lan,
            lan_ip: None,
            services: vec![],
        };
        let dto = ProjectViewDto::from(view);
        assert_eq!(dto.expose, "lan");
        assert_eq!(dto.lan_ip, None);
    }

    #[test]
    fn project_view_dto_carries_expose_and_lan_ip() {
        let view = ProjectView {
            name: "a".into(),
            repo: "r".into(),
            branch: "main".into(),
            hostname: None,
            host_port: 8000,
            expose: pi_domain::entities::ExposeMode::Lan,
            lan_ip: Some("192.168.1.50".parse().unwrap()),
            services: vec![],
        };
        let dto = ProjectViewDto::from(view);
        assert_eq!(dto.expose, "lan");
        assert_eq!(dto.lan_ip.as_deref(), Some("192.168.1.50"));
    }
}
