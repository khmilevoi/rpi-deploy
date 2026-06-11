use std::path::Path;

use crate::cli::api::ApiClient;
use crate::cli::config::ClientConfig;
use crate::cli::pitoml::PiToml;
use crate::cli::tunnel::SshTunnel;
use crate::proto::DeployRequest;

pub async fn deploy(git_ref: Option<String>, server: Option<String>) -> anyhow::Result<()> {
    let pitoml = PiToml::load(Path::new("pi.toml"))?;
    let project = pitoml.to_project_config();

    let profile = ClientConfig::load()?.select(server.as_deref())?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let version = api.version().await?;
    eprintln!("agent {} (api {})", version.version, version.api);

    let req = DeployRequest { project: (&project).into(), git_ref };
    let accepted = api.deploy(&req).await?;
    eprintln!("deployment {} started; streaming logs:", accepted.deployment_id);

    let status = api.follow_logs(&accepted.deployment_id, |line| println!("{line}")).await?;
    eprintln!("deploy finished: {status}");
    if status != "success" {
        drop(tunnel);
        std::process::exit(1);
    }
    Ok(())
}

pub async fn ls(server: Option<String>) -> anyhow::Result<()> {
    let profile = ClientConfig::load()?.select(server.as_deref())?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let projects = api.projects().await?;
    if projects.is_empty() {
        println!("no projects deployed yet");
        return Ok(());
    }
    println!("{:<16} {:<10} {:<28} {:<6} SERVICES", "NAME", "BRANCH", "HOSTNAME", "PORT");
    for p in projects {
        let services = if p.services.is_empty() {
            "-".to_string()
        } else {
            p.services
                .iter()
                .map(|s| format!("{}:{}", s.service, s.state))
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "{:<16} {:<10} {:<28} {:<6} {services}",
            p.name,
            p.branch,
            p.hostname.unwrap_or_else(|| "-".into()),
            p.host_port
        );
    }
    Ok(())
}
