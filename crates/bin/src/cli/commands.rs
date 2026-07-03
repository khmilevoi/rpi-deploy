use std::collections::BTreeMap;
use std::path::Path;

use crate::cli::api::ApiClient;
use crate::cli::config::ConnectOpts;
use crate::cli::pitoml::PiToml;
use crate::cli::ssh::SshExec;
use crate::cli::tunnel::SshTunnel;
use crate::duration::parse_duration_secs;
use crate::proto::{DeployRequest, DiagnosticCheckDto};

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
    // One failed cancel (e.g. a deploy that finished in the meantime) must not
    // leave the rest of the active list untouched.
    let mut failures = 0usize;
    for d in active {
        match api.cancel_deployment(&d.id).await {
            Ok(decision) => eprintln!("deployment {} ({}): {decision}", d.id, d.status),
            Err(err) => {
                failures += 1;
                eprintln!("deployment {} ({}): cancel failed: {err}", d.id, d.status);
            }
        }
    }
    if failures > 0 {
        anyhow::bail!("{failures} cancel request(s) failed");
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

/// `rpi ls` EXPOSE cell: `-` for private/unknown, `lan http://<ip>:<port>` for
/// expose=lan with a detected ip, `lan (ip n/a)` when the ip could not be
/// detected (§12.1).
fn expose_cell(expose: &str, lan_ip: Option<&str>, host_port: u16) -> String {
    match expose {
        "lan" => match lan_ip {
            Some(ip) => format!("lan http://{ip}:{host_port}"),
            None => "lan (ip n/a)".to_string(),
        },
        _ => "-".to_string(),
    }
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
        "{:<16} {:<10} {:<28} {:<6} {:<28} SERVICES",
        "NAME", "BRANCH", "HOSTNAME", "PORT", "EXPOSE"
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
        let expose = expose_cell(&p.expose, p.lan_ip.as_deref(), p.host_port);
        println!(
            "{:<16} {:<10} {:<28} {:<6} {:<28} {services}",
            p.name,
            p.branch,
            p.hostname.unwrap_or_else(|| "-".into()),
            p.host_port,
            expose
        );
    }
    Ok(())
}

pub async fn logs(
    project: String,
    follow: bool,
    tail: usize,
    connect: ConnectOpts,
) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());
    api.stream_sse(
        &format!("/v1/projects/{project}/logs?tail={tail}&follow={follow}"),
        |line| println!("{line}"),
    )
    .await
}

pub async fn stats(
    project: Option<String>,
    json: bool,
    connect: ConnectOpts,
) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());
    let resp = api.stats(project.as_deref()).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }
    println!(
        "host: cpu {:.1}%, mem {}/{} bytes, disk {}%, uptime {}",
        resp.host.cpu_percent,
        resp.host.mem_used_bytes,
        resp.host.mem_total_bytes,
        resp.host.disk_used_percent,
        human_duration(resp.host.uptime_secs)
    );
    for p in resp.projects {
        println!("project {}", p.project);
        for s in p.services {
            println!(
                "  {}: cpu {:.1}%, mem {}/{} bytes",
                s.service, s.cpu_percent, s.mem_used_bytes, s.mem_limit_bytes
            );
        }
    }
    Ok(())
}

pub async fn lifecycle(project: String, action: &str, connect: ConnectOpts) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());
    api.lifecycle(&project, action).await?;
    eprintln!("{action} '{project}': done");
    Ok(())
}

pub async fn rm(
    project: String,
    volumes: bool,
    yes: bool,
    connect: ConnectOpts,
) -> anyhow::Result<()> {
    if !yes {
        eprintln!(
            "this removes containers{}, the ingress route, workdir, secrets, deploy key and history of '{project}'",
            if volumes { ", VOLUMES (project data!)" } else { "" }
        );
        eprint!("type the project name to confirm: ");
        use std::io::Write;
        std::io::stderr().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if input.trim() != project {
            anyhow::bail!("confirmation failed: expected '{project}'");
        }
    }

    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());
    let resp = api.remove_project(&project, volumes).await?;
    eprintln!(
        "project '{}' removed{}",
        resp.project,
        if resp.volumes_removed {
            " (volumes included)"
        } else {
            " (volumes kept)"
        }
    );
    if let Some(hostname) = resp.hostname {
        eprintln!("note: the DNS record for {hostname} may still exist;");
        eprintln!("delete it manually: Cloudflare dashboard -> your zone -> DNS -> remove the {hostname} CNAME");
    }
    Ok(())
}

pub async fn status(json: bool, connect: ConnectOpts) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());
    let resp = api.agent_status().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }
    print_agent_status(&resp);
    Ok(())
}

fn print_agent_status(resp: &crate::proto::AgentOverviewDto) {
    println!(
        "agent v{} (cli v{})",
        resp.version,
        env!("CARGO_PKG_VERSION")
    );
    println!("uptime: {}", human_duration(resp.uptime_secs));
    println!("disk: {}% used", resp.disk_used_percent);
    println!(
        "projects: {}, active deployments: {}",
        resp.projects, resp.active_deployments
    );
}

pub(crate) fn render_doctor(checks: &[DiagnosticCheckDto]) -> (String, bool) {
    let mut out = String::new();
    let mut ok = true;
    for c in checks {
        let mark = if c.passed {
            "PASS"
        } else {
            ok = false;
            "FAIL"
        };
        out.push_str(&format!("{mark}  {} - {}\n", c.name, c.detail));
        if let (false, Some(hint)) = (c.passed, &c.hint) {
            out.push_str(&format!("      hint: {hint}\n"));
        }
    }
    (out, ok)
}

fn check(name: &str, passed: bool, detail: String, hint: Option<&str>) -> DiagnosticCheckDto {
    DiagnosticCheckDto {
        name: name.to_string(),
        passed,
        detail,
        hint: if passed {
            None
        } else {
            hint.map(str::to_string)
        },
    }
}

pub async fn doctor(connect: ConnectOpts) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let mut checks: Vec<DiagnosticCheckDto> = Vec::new();
    let ssh = SshExec { profile: &profile };
    checks.push(match ssh.check().await {
        Ok(()) => check(
            "ssh connection",
            true,
            format!("{}@{}", profile.user, profile.host),
            None,
        ),
        Err(e) => check(
            "ssh connection",
            false,
            e,
            Some("check host/user/key in ~/.config/pi/config.toml; try plain `ssh` manually"),
        ),
    });
    match SshTunnel::open(&profile).await {
        Err(e) => checks.push(check(
            "agent tunnel",
            false,
            e.to_string(),
            Some("is pi-agent.service running on the Pi? try `rpi agent status`"),
        )),
        Ok(tunnel) => {
            let api = ApiClient::new(tunnel.base_url.clone());
            match api.version().await {
                Err(e) => checks.push(check(
                    "agent api",
                    false,
                    e.to_string(),
                    Some("agent is unreachable through the tunnel; `rpi agent logs` for details"),
                )),
                Ok(v) => {
                    checks.push(check(
                        "agent api",
                        true,
                        format!("agent v{} (api {})", v.version, v.api),
                        None,
                    ));
                    let cli_version = env!("CARGO_PKG_VERSION");
                    checks.push(check(
                        "version match",
                        v.version == cli_version,
                        format!("cli v{cli_version}, agent v{}", v.version),
                        Some("update the agent binary on the Pi"),
                    ));
                    match api.doctor().await {
                        Ok(resp) => checks.extend(resp.checks),
                        Err(e) => checks.push(check(
                            "agent doctor",
                            false,
                            e.to_string(),
                            Some("agent is older than v0.4? update it on the Pi"),
                        )),
                    }
                }
            }
        }
    }
    let (rendered, ok) = render_doctor(&checks);
    print!("{rendered}");
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}

pub async fn agent_status(connect: ConnectOpts) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let api_attempt = async {
        let tunnel = SshTunnel::open(&profile).await?;
        ApiClient::new(tunnel.base_url.clone()).agent_status().await
    };
    match api_attempt.await {
        Ok(resp) => {
            print_agent_status(&resp);
            Ok(())
        }
        Err(err) => {
            eprintln!("agent API unreachable ({err})");
            eprintln!(
                "falling back to: ssh {}@{} systemctl status pi-agent",
                profile.user, profile.host
            );
            SshExec { profile: &profile }
                .run(&["systemctl", "status", "pi-agent", "--no-pager"])
                .await
        }
    }
}

pub(crate) fn build_agent_logs_query(
    follow: bool,
    since: &Option<String>,
    tail: usize,
    now_unix: i64,
) -> anyhow::Result<String> {
    match since {
        Some(spec) => {
            let secs = parse_duration_secs(spec).map_err(|e| anyhow::anyhow!(e))?;
            let cutoff = now_unix - secs as i64;
            Ok(format!("/v1/agent/logs?since={cutoff}&follow={follow}"))
        }
        None => Ok(format!("/v1/agent/logs?tail={tail}&follow={follow}")),
    }
}

pub(crate) fn journalctl_args(follow: bool, since_unix: Option<i64>, tail: usize) -> Vec<String> {
    let mut args: Vec<String> = ["journalctl", "-u", "pi-agent", "--no-pager", "-n"]
        .map(String::from)
        .to_vec();
    args.push(tail.to_string());
    if let Some(cutoff) = since_unix {
        args.push(format!("--since=@{cutoff}"));
    }
    if follow {
        args.push("-f".to_string());
    }
    args
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub async fn agent_logs(
    follow: bool,
    since: Option<String>,
    tail: usize,
    connect: ConnectOpts,
) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let now = now_unix();
    let query = build_agent_logs_query(follow, &since, tail, now)?;
    let api_attempt = async {
        let tunnel = SshTunnel::open(&profile).await?;
        let api = ApiClient::new(tunnel.base_url.clone());
        api.stream_sse(&query, |line| println!("{line}")).await
    };
    match api_attempt.await {
        Ok(()) => Ok(()),
        Err(err) => {
            eprintln!("agent API unreachable ({err})");
            let since_unix = since
                .as_deref()
                .and_then(|s| parse_duration_secs(s).ok())
                .map(|secs| now - secs as i64);
            let args = journalctl_args(follow, since_unix, tail);
            let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
            eprintln!(
                "falling back to: ssh {}@{} {}",
                profile.user,
                profile.host,
                args.join(" ")
            );
            SshExec { profile: &profile }.run(&args_ref).await
        }
    }
}

fn human_duration(secs: u64) -> String {
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

/// §9.1: differing CLI/agent binary versions are a warning, not an error.
fn version_mismatch_warning(cli_version: &str, agent_version: &str) -> Option<String> {
    (cli_version != agent_version).then(|| {
        format!(
            "warning: CLI v{cli_version} and agent v{agent_version} differ - \
rebuild/update the agent on the Pi (`rpi agent update` ships in v0.5)"
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

    #[test]
    fn render_doctor_marks_failures_and_hints() {
        let checks = vec![
            DiagnosticCheckDto {
                name: "docker daemon".into(),
                passed: true,
                detail: "27.0".into(),
                hint: None,
            },
            DiagnosticCheckDto {
                name: "disk space".into(),
                passed: false,
                detail: "91% used".into(),
                hint: Some("run `rpi gc`".into()),
            },
        ];
        let (out, ok) = render_doctor(&checks);
        assert!(!ok);
        assert!(out.contains("PASS  docker daemon"), "{out}");
        assert!(out.contains("FAIL  disk space"), "{out}");
        assert!(out.contains("hint: run `rpi gc`"), "{out}");
        let (_, ok) = render_doctor(&checks[..1]);
        assert!(ok);
    }

    #[test]
    fn agent_logs_query_prefers_since_over_tail() {
        let q = build_agent_logs_query(false, &None, 50, 1000).unwrap();
        assert_eq!(q, "/v1/agent/logs?tail=50&follow=false");
        let q = build_agent_logs_query(true, &Some("2h".into()), 50, 10_000).unwrap();
        assert_eq!(q, "/v1/agent/logs?since=2800&follow=true");
        assert!(build_agent_logs_query(false, &Some("soon".into()), 50, 0).is_err());
    }

    #[test]
    fn expose_cell_shows_lan_url_only_for_lan() {
        assert_eq!(expose_cell("private", None, 8000), "-".to_string());
        assert_eq!(expose_cell("", None, 8000), "-".to_string());
        assert_eq!(
            expose_cell("lan", Some("192.168.1.50"), 8000),
            "lan http://192.168.1.50:8000".to_string()
        );
        assert_eq!(expose_cell("lan", None, 8000), "lan (ip n/a)".to_string());
    }

    #[test]
    fn journalctl_args_shape() {
        assert_eq!(
            journalctl_args(false, None, 100),
            vec!["journalctl", "-u", "pi-agent", "--no-pager", "-n", "100"]
        );
        assert_eq!(
            journalctl_args(true, Some(1234), 50),
            vec![
                "journalctl",
                "-u",
                "pi-agent",
                "--no-pager",
                "-n",
                "50",
                "--since=@1234",
                "-f"
            ]
        );
    }
}
