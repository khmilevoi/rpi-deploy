use std::collections::BTreeMap;
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

    let req = DeployRequest {
        project: (&project).into(),
        git_ref,
    };
    let accepted = api.deploy(&req).await?;
    eprintln!(
        "deployment {} started; streaming logs:",
        accepted.deployment_id
    );

    let status = api
        .follow_logs(&accepted.deployment_id, |line| println!("{line}"))
        .await?;
    eprintln!("deploy finished: {status}");
    if status != "success" {
        drop(tunnel);
        std::process::exit(1);
    }
    Ok(())
}

pub async fn env_send(apply: bool, server: Option<String>) -> anyhow::Result<()> {
    let pitoml = PiToml::load(Path::new("pi.toml"))?;
    let project_name = pitoml.project.name.clone();
    let env_file = Path::new(&pitoml.env.file).to_path_buf();

    let raw = std::fs::read_to_string(&env_file)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", env_file.display()))?;
    let vars = parse_env_file(&raw)?;
    if vars.is_empty() {
        anyhow::bail!("no variables found in {}", env_file.display());
    }

    let profile = ClientConfig::load()?.select(server.as_deref())?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let n = vars.len();
    let resp = api.send_env(&project_name, vars, apply).await?;
    eprintln!("saved {n} key(s) for project '{project_name}'");
    if resp.applied {
        eprintln!(".env applied to running containers");
    }
    Ok(())
}

pub async fn env_ls(server: Option<String>) -> anyhow::Result<()> {
    let pitoml = PiToml::load(Path::new("pi.toml"))?;
    let project_name = pitoml.project.name.clone();

    let profile = ClientConfig::load()?.select(server.as_deref())?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let resp = api.env_keys(&project_name).await?;
    if resp.keys.is_empty() {
        println!("no secrets stored for project '{project_name}'");
    } else {
        for key in &resp.keys {
            println!("{key}");
        }
    }
    Ok(())
}

fn parse_env_file(text: &str) -> anyhow::Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, val) = line
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("line {}: expected KEY=VALUE, got: {line}", i + 1))?;
        let key = key.trim().to_string();
        let val = strip_quotes(val.trim());
        map.insert(key, val);
    }
    Ok(map)
}

fn strip_quotes(s: &str) -> String {
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
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
    println!(
        "{:<16} {:<10} {:<28} {:<6} SERVICES",
        "NAME", "BRANCH", "HOSTNAME", "PORT"
    );
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
