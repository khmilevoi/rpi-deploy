use std::sync::Arc;

use async_trait::async_trait;
use pi_domain::contracts::{DiskProbe, ProjectRepository, SystemProbe};
use pi_domain::entities::{AgentOverview, DiagnosticCheck, DiagnosticReport};
use pi_domain::error::DomainError;
use sysinfo::System;
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
}

impl HostSystemProbe {
    pub fn new(
        runner: Arc<dyn ProbeRunner>,
        disk: Arc<dyn DiskProbe>,
        projects: Arc<dyn ProjectRepository>,
        version: String,
    ) -> Arc<HostSystemProbe> {
        Arc::new(HostSystemProbe {
            runner,
            disk,
            projects,
            version,
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
                "start Docker and make sure the pi-agent user can access it",
            )
            .await,
            self.command_check(
                "docker compose",
                "docker",
                &["compose", "version"],
                "install Docker Compose v2",
            )
            .await,
            self.command_check(
                "cloudflared",
                "cloudflared",
                &["--version"],
                "install cloudflared or disable [cloudflared] routing",
            )
            .await,
        ];

        checks.push(match self.disk.used_percent() {
            Ok(percent) => DiagnosticCheck {
                name: "disk space".into(),
                passed: percent < 90,
                detail: format!("{percent}% used"),
                hint: (percent >= 90).then(|| "run `pi gc` or free disk space".into()),
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
        Ok(AgentOverview {
            version: self.version.clone(),
            uptime_secs: System::uptime(),
            disk_used_percent: self.disk.used_percent()?,
            projects,
            active_deployments: 0,
        })
    }
}
