use std::collections::BTreeMap;
use std::path::Path;

use crate::cli::api::ApiClient;
use crate::cli::config::ConnectOpts;
use crate::cli::pitoml::PiToml;
use crate::cli::tunnel::SshTunnel;
use crate::proto::DeployRequest;

pub async fn deploy(git_ref: Option<String>, connect: ConnectOpts) -> anyhow::Result<()> {
    let pitoml = PiToml::load(Path::new("pi.toml"))?;
    let project = pitoml.to_project_config();

    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let version = api.version().await?;
    eprintln!("agent {} (api {})", version.version, version.api);
    if let Some(warning) = version_mismatch_warning(env!("CARGO_PKG_VERSION"), &version.version) {
        eprintln!("{warning}");
    }

    let req = DeployRequest {
        project: (&project).into(),
        git_ref,
    };
    let accepted = api.deploy(&req).await?;
    if accepted.queued {
        eprintln!(
            "deployment {} queued behind the active deploy (latest wins); waiting...",
            accepted.deployment_id
        );
    } else {
        eprintln!(
            "deployment {} started; streaming logs:",
            accepted.deployment_id
        );
    }

    let status = api
        .follow_logs(&accepted.deployment_id, |line| println!("{line}"))
        .await?;
    eprintln!("deploy finished: {status}");
    if status == "superseded" {
        eprintln!("note: a newer deploy request replaced this one - not an error");
    }
    if status != "success" && status != "superseded" {
        drop(tunnel);
        std::process::exit(1);
    }
    Ok(())
}

pub async fn deploy_cancel(connect: ConnectOpts) -> anyhow::Result<()> {
    let pitoml = PiToml::load(Path::new("pi.toml"))?;
    let project_name = pitoml.project.name.clone();

    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let active = api.active_deployments(&project_name).await?;
    if active.is_empty() {
        eprintln!("no active deployment for '{project_name}' - nothing to cancel");
        return Ok(());
    }
    for d in active {
        let decision = api.cancel_deployment(&d.id).await?;
        eprintln!("deployment {} ({}): {decision}", d.id, d.status);
    }
    Ok(())
}

pub async fn env_send(apply: bool, connect: ConnectOpts) -> anyhow::Result<()> {
    let pitoml = PiToml::load(Path::new("pi.toml"))?;
    let project_name = pitoml.project.name.clone();
    let env_file = Path::new(&pitoml.env.file).to_path_buf();

    let raw = std::fs::read_to_string(&env_file)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", env_file.display()))?;
    let vars = parse_env_file(&raw)?;
    if vars.is_empty() {
        anyhow::bail!("no variables found in {}", env_file.display());
    }

    let profile = connect.resolve()?;
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

pub async fn gc(connect: ConnectOpts) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let resp = api.gc().await?;
    eprintln!(
        "gc done: disk {}% used; build cache pruned: {}",
        resp.disk_used_percent,
        if resp.builder_pruned { "yes" } else { "no" }
    );
    Ok(())
}

pub async fn env_ls(connect: ConnectOpts) -> anyhow::Result<()> {
    let pitoml = PiToml::load(Path::new("pi.toml"))?;
    let project_name = pitoml.project.name.clone();

    let profile = connect.resolve()?;
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

/// Same dotenv dialect as the agent's PUT validation (§10, plan Task 3):
/// anything accepted here is accepted server-side, and vice versa.
fn parse_env_file(text: &str) -> anyhow::Result<BTreeMap<String, String>> {
    let bundle = pi_infrastructure::dotenv::parse(text).map_err(|e| anyhow::anyhow!(e))?;
    Ok(bundle.vars)
}

pub async fn ls(connect: ConnectOpts) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
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

/// §9.1: differing CLI/agent binary versions are a warning, not an error.
fn version_mismatch_warning(cli_version: &str, agent_version: &str) -> Option<String> {
    (cli_version != agent_version).then(|| {
        format!(
            "warning: CLI v{cli_version} and agent v{agent_version} differ - \
rebuild/update the agent on the Pi (`pi agent update` ships in v0.5)"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_file_parsing_matches_agent_rules() {
        let text = "# c\nexport TOKEN=\"abc=def\"\nNAME='single'\nDB=postgres://u:p@db/x\n";
        let vars = parse_env_file(text).unwrap();
        assert_eq!(vars["TOKEN"], "abc=def");
        assert_eq!(vars["NAME"], "single");
        assert_eq!(vars["DB"], "postgres://u:p@db/x");
        assert_eq!(vars.len(), 3);
        assert!(parse_env_file("1BAD=x").is_err());
    }

    #[test]
    fn version_mismatch_produces_warning_only_on_difference() {
        assert!(version_mismatch_warning("0.3.0", "0.3.0").is_none());
        let warning = version_mismatch_warning("0.3.0", "0.2.0").unwrap();
        assert!(warning.contains("0.3.0") && warning.contains("0.2.0"));
    }
}
