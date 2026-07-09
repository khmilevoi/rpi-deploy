use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::{DiskProbe, ProjectRepository, SystemProbe};
use pi_domain::entities::{AgentOverview, DiagnosticCheck, DiagnosticReport};
use pi_domain::error::DomainError;
use tokio::process::Command;

#[async_trait]
pub trait ProbeRunner: Send + Sync {
    async fn run(&self, program: &str, args: &[&str]) -> Result<String, String>;
}

pub struct SystemRunner;

#[async_trait]
impl ProbeRunner for SystemRunner {
    async fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
        let output = Command::new(program)
            .args(args)
            .output()
            .await
            .map_err(|e| format!("spawn {program}: {e}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    }
}

pub struct HostSystemProbe {
    runner: Arc<dyn ProbeRunner>,
    disk: Arc<dyn DiskProbe>,
    projects: Arc<dyn ProjectRepository>,
    version: String,
    disk_threshold_percent: u8,
    cloudflared_enabled: bool,
    ingress_active: bool,
    started_at: i64,
}

impl HostSystemProbe {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        runner: Arc<dyn ProbeRunner>,
        disk: Arc<dyn DiskProbe>,
        projects: Arc<dyn ProjectRepository>,
        version: String,
        disk_threshold_percent: u8,
        cloudflared_enabled: bool,
        ingress_active: bool,
        started_at: i64,
    ) -> Arc<HostSystemProbe> {
        Arc::new(HostSystemProbe {
            runner,
            disk,
            projects,
            version,
            disk_threshold_percent,
            cloudflared_enabled,
            ingress_active,
            started_at,
        })
    }

    async fn command_check(
        &self,
        name: &str,
        program: &str,
        args: &[&str],
        hint: &str,
    ) -> DiagnosticCheck {
        match self.runner.run(program, args).await {
            Ok(out) => DiagnosticCheck {
                name: name.into(),
                passed: true,
                detail: out.lines().next().unwrap_or("ok").to_string(),
                hint: None,
            },
            Err(err) => DiagnosticCheck {
                name: name.into(),
                passed: false,
                detail: err,
                hint: Some(hint.into()),
            },
        }
    }
}

#[async_trait]
impl SystemProbe for HostSystemProbe {
    async fn diagnostics(&self) -> DiagnosticReport {
        let mut checks = vec![
            self.command_check(
                "docker daemon",
                "docker",
                &["version", "--format", "{{.Server.Version}}"],
                "start Docker and make sure the rpi-agent user can access it",
            )
            .await,
            self.command_check(
                "docker compose",
                "docker",
                &["compose", "version"],
                "install Docker Compose v2",
            )
            .await,
        ];

        checks.push(match self.runner.run("id", &["-nG"]).await {
            Ok(out) => {
                let in_docker_group = out.split_whitespace().any(|g| g == "docker");
                DiagnosticCheck {
                    name: "rpi-agent group".into(),
                    passed: in_docker_group,
                    detail: format!("groups: {out}"),
                    hint: if !in_docker_group {
                        Some("add rpi-agent to the 'docker' group: sudo usermod -aG docker rpi-agent".into())
                    } else {
                        None
                    },
                }
            }
            Err(err) => DiagnosticCheck {
                name: "rpi-agent group".into(),
                passed: false,
                detail: err,
                hint: Some("add rpi-agent to the 'docker' group".into()),
            },
        });

        checks.push(
            match self
                .runner
                .run("loginctl", &["show-user", "-P", "Linger", "rpi-agent"])
                .await
            {
                Ok(out) => {
                    let linger_enabled = out == "yes";
                    DiagnosticCheck {
                        name: "systemd linger".into(),
                        passed: linger_enabled,
                        detail: format!("Linger={out}"),
                        hint: if !linger_enabled {
                            Some("enable linger: loginctl enable-linger rpi-agent".into())
                        } else {
                            None
                        },
                    }
                }
                Err(err) => DiagnosticCheck {
                    name: "systemd linger".into(),
                    passed: false,
                    detail: err,
                    hint: Some("enable linger: loginctl enable-linger rpi-agent".into()),
                },
            },
        );

        if self.cloudflared_enabled {
            checks.push(
                self.command_check(
                    "cloudflared",
                    "cloudflared",
                    &["--version"],
                    "install cloudflared or disable [cloudflared] routing",
                )
                .await,
            );
            checks.push(
                self.command_check(
                    "cloudflared service",
                    "systemctl",
                    &["--user", "is-active", "cloudflared"],
                    "enable and start cloudflared service: systemctl --user enable --now cloudflared",
                )
                .await,
            );
        }

        if !self.ingress_active {
            if let Ok(projects) = self.projects.list().await {
                let hostnames: Vec<String> = projects
                    .iter()
                    .filter_map(|p| p.config.hostname.clone())
                    .collect();
                if !hostnames.is_empty() {
                    checks.push(DiagnosticCheck {
                        name: "ingress routing".into(),
                        passed: false,
                        detail: format!(
                            "hostname(s) declared but automatic ingress is disabled: {}",
                            hostnames.join(", ")
                        ),
                        hint: Some(
                            "enable it: sudo rpi agent setup --with-cloudflared \
                             --cf-token <token> --domain <zone>"
                                .into(),
                        ),
                    });
                }
            }
        }

        checks.push(match self.disk.used_percent() {
            Ok(percent) => DiagnosticCheck {
                name: "disk space".into(),
                passed: percent < self.disk_threshold_percent,
                detail: format!("{percent}% used"),
                hint: (percent >= self.disk_threshold_percent)
                    .then(|| "run `rpi gc` or free disk space".into()),
            },
            Err(err) => DiagnosticCheck {
                name: "disk space".into(),
                passed: false,
                detail: err.to_string(),
                hint: Some("check agent data directory mount".into()),
            },
        });

        DiagnosticReport { checks }
    }

    async fn overview(&self) -> Result<AgentOverview, DomainError> {
        let projects = self.projects.list().await?.len();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let uptime_secs = (now - self.started_at).max(0) as u64;

        Ok(AgentOverview {
            version: self.version.clone(),
            uptime_secs,
            disk_used_percent: self.disk.used_percent()?,
            projects,
            active_deployments: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_domain::contracts::{MockDiskProbe, MockProjectRepository, SystemProbe};
    use pi_domain::entities::{
        ExposeMode, HealthcheckConfig, Project, ProjectConfig, StageTimeoutOverrides,
    };

    struct FakeRunner;
    #[async_trait]
    impl ProbeRunner for FakeRunner {
        async fn run(&self, _program: &str, _args: &[&str]) -> Result<String, String> {
            Ok("ok".into())
        }
    }

    fn project(hostname: Option<&str>) -> Project {
        Project {
            config: ProjectConfig {
                name: "app".into(),
                repo: "r".into(),
                branch: "main".into(),
                compose_path: "docker-compose.yml".into(),
                service: "web".into(),
                container_port: 80,
                hostname: hostname.map(String::from),
                expose: ExposeMode::default(),
                healthcheck: HealthcheckConfig::default(),
                timeouts: StageTimeoutOverrides::default(),
                commands: Default::default(),
                command_timeout_secs: None,
            },
            host_port: 8002,
            created_at: 1,
        }
    }

    fn probe(ingress_active: bool, projects: Vec<Project>) -> Arc<HostSystemProbe> {
        let mut repo = MockProjectRepository::new();
        repo.expect_list().return_once(move || Ok(projects));
        let mut disk = MockDiskProbe::new();
        disk.expect_used_percent().returning(|| Ok(10));
        HostSystemProbe::new(
            Arc::new(FakeRunner),
            Arc::new(disk),
            Arc::new(repo),
            "0.0.0".into(),
            85,
            false, // cloudflared binary/service checks off — not under test
            ingress_active,
            0,
        )
    }

    #[tokio::test]
    async fn disabled_ingress_with_hostnames_fails_the_ingress_check() {
        let report = probe(false, vec![project(Some("rpi.example.com"))])
            .diagnostics()
            .await;
        let check = report
            .checks
            .iter()
            .find(|c| c.name == "ingress routing")
            .expect("ingress routing check present");
        assert!(!check.passed);
        assert!(check.detail.contains("rpi.example.com"));
        assert!(check
            .hint
            .as_deref()
            .unwrap_or_default()
            .contains("sudo rpi agent setup --with-cloudflared"));
    }

    #[tokio::test]
    async fn active_ingress_or_no_hostnames_add_no_check() {
        for (active, host) in [(true, Some("a.example.com")), (false, None)] {
            let report = probe(active, vec![project(host)]).diagnostics().await;
            assert!(
                report.checks.iter().all(|c| c.name != "ingress routing"),
                "unexpected check for active={active} host={host:?}"
            );
        }
    }
}
