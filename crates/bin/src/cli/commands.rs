use std::collections::BTreeMap;
use std::path::Path;

use base64::Engine as _;

use crate::cli::api::ApiClient;
use crate::cli::config::ConnectOpts;
use crate::cli::rpitoml::{RpiToml, SecretsSection};
use crate::cli::ssh::SshExec;
use crate::cli::tunnel::SshTunnel;
use crate::duration::parse_duration_secs;
use crate::output;
use crate::proto::{DeployRequest, DiagnosticCheckDto};

pub async fn deploy(git_ref: Option<String>, connect: ConnectOpts) -> anyhow::Result<()> {
    let rpitoml = RpiToml::load(Path::new("rpi.toml"))?;
    let project = rpitoml.to_project_config();
    output::show_deploy_banner(&rpitoml.project.name);

    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let version = api.version().await?;
    output::status(format!("agent {} (api {})", version.version, version.api));
    if let Some(warning) = version_mismatch_warning(env!("CARGO_PKG_VERSION"), &version.version) {
        output::warn(warning);
    }

    let req = DeployRequest {
        project: (&project).into(),
        git_ref,
    };
    let started = std::time::Instant::now();
    let accepted = api.deploy(&req).await?;
    if accepted.queued {
        output::status(format!(
            "deployment {} queued behind the active deploy (latest wins); waiting...",
            accepted.deployment_id
        ));
    } else {
        output::status(format!(
            "deployment {} started; streaming logs:",
            accepted.deployment_id
        ));
    }

    let mut pane = output::LogPane::new(format!("deploy '{}'", rpitoml.project.name), 10);
    let mut warnings: Vec<String> = Vec::new();
    let status = api
        .follow_logs(&accepted.deployment_id, |line| {
            if let Some(w) = deploy_warning(line) {
                warnings.push(w.to_string());
            }
            pane.push_line(line)
        })
        .await?;
    let elapsed = started.elapsed();
    let name = &rpitoml.project.name;
    let url = rpitoml.ingress.hostname.as_deref();
    match status.as_str() {
        "success" => pane.finish_ok(&output::deploy_stamp(
            output::StampOutcome::Success,
            name,
            url,
            elapsed,
        )),
        "superseded" => pane.finish_neutral(&output::deploy_stamp(
            output::StampOutcome::Superseded,
            name,
            url,
            elapsed,
        )),
        _ => {
            pane.finish_err(&output::deploy_stamp(
                output::StampOutcome::Failed,
                name,
                url,
                elapsed,
            ));
            for w in &warnings {
                output::warn(w);
            }
            drop(tunnel);
            std::process::exit(1);
        }
    }
    for w in &warnings {
        output::warn(w);
    }
    Ok(())
}

pub async fn deploy_cancel(connect: ConnectOpts) -> anyhow::Result<()> {
    let rpitoml = RpiToml::load(Path::new("rpi.toml"))?;
    let project_name = rpitoml.project.name.clone();

    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let active = api.active_deployments(&project_name).await?;
    if active.is_empty() {
        output::status(format!(
            "no active deployment for '{project_name}' - nothing to cancel"
        ));
        return Ok(());
    }
    // One failed cancel (e.g. a deploy that finished in the meantime) must not
    // leave the rest of the active list untouched.
    let mut failures = 0usize;
    for d in active {
        match api.cancel_deployment(&d.id).await {
            Ok(decision) => {
                output::status(format!("deployment {} ({}): {decision}", d.id, d.status))
            }
            Err(err) => {
                failures += 1;
                output::error(format!(
                    "deployment {} ({}): cancel failed: {err}",
                    d.id, d.status
                ));
            }
        }
    }
    if failures > 0 {
        anyhow::bail!("{failures} cancel request(s) failed");
    }
    Ok(())
}

pub async fn secrets_send(apply: bool, connect: ConnectOpts) -> anyhow::Result<()> {
    let rpitoml = RpiToml::load(Path::new("rpi.toml"))?;
    let project_name = rpitoml.project.name.clone();
    let (vars, files) = collect_secrets(Path::new("."), &rpitoml.secrets)?;
    if vars.is_empty() && files.is_empty() {
        anyhow::bail!("no secrets to send: env file has no variables and [secrets].files is empty");
    }

    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let (n, m) = (vars.len(), files.len());
    let resp = api.send_secrets(&project_name, vars, files, apply).await?;
    output::success(format!(
        "saved {n} key(s) and {m} file(s) for project '{project_name}'"
    ));
    if resp.applied {
        output::success("secrets applied to running containers");
    }
    Ok(())
}

/// Assemble the outgoing bundle per secrets spec §3: an explicitly configured
/// env file must exist, the default ".env" may be absent; all missing
/// [secrets].files are reported in one error; limits match the agent's.
///
/// Every path is resolved with `secretpath::resolve_within_root` before it is
/// opened: `rpi.toml` parsing already rejects `..`/absolute strings via
/// `validate_rel_path`, but that is a string-only check and cannot see that a
/// path component is, on disk, a symlink pointing outside the project root
/// (e.g. a git-tracked symlink committed by a malicious contributor). Without
/// this check `rpi secrets send` would follow such a symlink and upload
/// whatever it points to — anywhere on the filesystem the invoking user can
/// read — to the remote agent.
fn collect_secrets(
    root: &Path,
    section: &SecretsSection,
) -> anyhow::Result<(BTreeMap<String, String>, BTreeMap<String, String>)> {
    let vars = match &section.env {
        Some(name) => {
            let display = root.join(name);
            let real = pi_infrastructure::secretpath::resolve_within_root(root, name)
                .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", display.display()))?;
            let raw = std::fs::read_to_string(&real)
                .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", display.display()))?;
            parse_env_file(&raw)?
        }
        None => match pi_infrastructure::secretpath::resolve_within_root(root, ".env") {
            Ok(real) => {
                let raw = std::fs::read_to_string(&real)
                    .map_err(|e| anyhow::anyhow!("cannot read .env: {e}"))?;
                parse_env_file(&raw)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(anyhow::anyhow!("cannot read .env: {e}")),
        },
    };

    let mut files = BTreeMap::new();
    let mut missing: Vec<&str> = Vec::new();
    let mut total: usize = 0;
    for rel in &section.files {
        let display = root.join(rel);
        let real = match pi_infrastructure::secretpath::resolve_within_root(root, rel) {
            Ok(p) => p,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                missing.push(rel);
                continue;
            }
            Err(e) => return Err(anyhow::anyhow!("cannot read {}: {e}", display.display())),
        };
        let bytes = match std::fs::read(&real) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                missing.push(rel);
                continue;
            }
            Err(e) => return Err(anyhow::anyhow!("cannot read {}: {e}", display.display())),
        };
        if bytes.len() > crate::proto::MAX_SECRET_FILE_BYTES {
            anyhow::bail!("secret file '{rel}' is {} bytes; max is 1 MiB", bytes.len());
        }
        total += bytes.len();
        if total > crate::proto::MAX_SECRETS_BUNDLE_BYTES {
            anyhow::bail!("secret files exceed 8 MiB total");
        }
        files.insert(
            rel.clone(),
            base64::engine::general_purpose::STANDARD.encode(&bytes),
        );
    }
    if !missing.is_empty() {
        anyhow::bail!(
            "secret file(s) not found: {} (paths are relative to the project root)",
            missing.join(", ")
        );
    }
    Ok((vars, files))
}

pub async fn gc(connect: ConnectOpts) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let resp = api.gc().await?;
    output::success(format!(
        "gc done: disk {}% used; build cache pruned: {}",
        resp.disk_used_percent,
        if resp.builder_pruned { "yes" } else { "no" }
    ));
    Ok(())
}

pub async fn secrets_ls(connect: ConnectOpts) -> anyhow::Result<()> {
    let rpitoml = RpiToml::load(Path::new("rpi.toml"))?;
    let project_name = rpitoml.project.name.clone();

    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let resp = api.list_secrets(&project_name).await?;
    if resp.keys.is_empty() && resp.files.is_empty() {
        output::info(format!("no secrets stored for project '{project_name}'"));
        return Ok(());
    }
    if !resp.keys.is_empty() {
        output::heading("env keys:");
        for key in &resp.keys {
            println!("  {key}");
        }
    }
    if !resp.files.is_empty() {
        output::heading("files:");
        for file in &resp.files {
            println!("  {file}");
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
        output::info("no projects deployed yet");
        return Ok(());
    }
    let mut table = output::table();
    table.set_header(output::header([
        "NAME", "BRANCH", "HOSTNAME", "PORT", "EXPOSE", "SERVICES",
    ]));
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
        let services_sem = output::services_sem(
            &p.services
                .iter()
                .map(|s| s.state.as_str())
                .collect::<Vec<_>>(),
        );
        let expose = expose_cell(&p.expose, p.lan_ip.as_deref(), p.host_port);
        table.add_row(vec![
            output::cell(p.name),
            output::cell(p.branch),
            output::cell(p.hostname.unwrap_or_else(|| "-".into())),
            output::cell(p.host_port.to_string()),
            output::cell(expose),
            output::cell_sem(services, services_sem),
        ]);
    }
    println!("{table}");
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
    let mut host_table = output::table();
    host_table.set_header(output::header(["CPU", "MEM", "DISK", "UPTIME"]));
    let host_mem_pct = if resp.host.mem_total_bytes > 0 {
        resp.host.mem_used_bytes as f64 / resp.host.mem_total_bytes as f64 * 100.0
    } else {
        0.0
    };
    host_table.add_row(vec![
        output::cell_sem(
            format!("{:.1}%", resp.host.cpu_percent),
            output::usage_sem(resp.host.cpu_percent),
        ),
        output::cell_sem(
            format!(
                "{}/{} bytes",
                resp.host.mem_used_bytes, resp.host.mem_total_bytes
            ),
            output::usage_sem(host_mem_pct),
        ),
        output::cell_sem(
            format!("{}%", resp.host.disk_used_percent),
            output::usage_sem(resp.host.disk_used_percent as f64),
        ),
        output::cell(human_duration(resp.host.uptime_secs)),
    ]);
    println!("{host_table}");

    if !resp.projects.is_empty() {
        let mut services_table = output::table();
        services_table.set_header(output::header(["PROJECT", "SERVICE", "CPU", "MEM"]));
        for p in resp.projects {
            let project_name = p.project.clone();
            for s in p.services {
                let mem_pct = if s.mem_limit_bytes > 0 {
                    s.mem_used_bytes as f64 / s.mem_limit_bytes as f64 * 100.0
                } else {
                    0.0
                };
                services_table.add_row(vec![
                    output::cell(project_name.clone()),
                    output::cell(s.service),
                    output::cell_sem(
                        format!("{:.1}%", s.cpu_percent),
                        output::usage_sem(s.cpu_percent),
                    ),
                    output::cell_sem(
                        format!("{}/{} bytes", s.mem_used_bytes, s.mem_limit_bytes),
                        output::usage_sem(mem_pct),
                    ),
                ]);
            }
        }
        println!("{services_table}");
    }
    Ok(())
}

pub async fn lifecycle(project: String, action: &str, connect: ConnectOpts) -> anyhow::Result<()> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());
    api.lifecycle(&project, action).await?;
    output::success(format!("{action} '{project}': done"));
    Ok(())
}

fn format_command_line(name: &str, spec: &pi_domain::entities::CommandSpec) -> String {
    let base = format!("{name}  ->  {}", spec.argv.join(" "));
    match &spec.service {
        Some(service) => format!("{base}  [service: {service}]"),
        None => base,
    }
}

pub async fn command(
    name: Option<String>,
    args: Vec<String>,
    connect: ConnectOpts,
) -> anyhow::Result<()> {
    let rpitoml = RpiToml::load(Path::new("rpi.toml"))?;
    let project_name = rpitoml.project.name.clone();

    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());

    let Some(name) = name else {
        // List mode: the agent's answer is the deployed reality; the local
        // file only powers the "undeployed changes" hint.
        let resp = api.list_commands(&project_name).await?;
        if resp.commands.is_empty() {
            output::status(format!(
                "no commands deployed for '{project_name}' - declare [commands] in rpi.toml and run `rpi deploy`"
            ));
        } else {
            for (cmd, spec) in &resp.commands {
                println!("{}", format_command_line(cmd, spec));
            }
        }
        let local = rpitoml.to_project_config().commands;
        let undeployed: Vec<&str> = local
            .keys()
            .filter(|k| !resp.commands.contains_key(*k))
            .map(String::as_str)
            .collect();
        if !undeployed.is_empty() {
            output::note(format!(
                "local rpi.toml declares undeployed command(s): {} - run `rpi deploy`",
                undeployed.join(", ")
            ));
        }
        return Ok(());
    };

    let mut pane = output::LogPane::new(format!("command '{name}'"), 10);
    let code = api
        .run_command(&project_name, &name, &args, |line| pane.push_line(line))
        .await?;
    if code != 0 {
        pane.finish_err(&format!("command '{name}' exited with code {code}"));
        drop(tunnel);
        std::process::exit(code);
    }
    pane.finish_ok_keep(&format!("command '{name}' finished (exit 0)"));
    Ok(())
}

pub async fn rm(
    project: String,
    volumes: bool,
    yes: bool,
    connect: ConnectOpts,
) -> anyhow::Result<()> {
    if !yes {
        output::warn(format!(
            "this removes containers{}, the ingress route, workdir, secrets, deploy key and history of '{project}'",
            if volumes { ", VOLUMES (project data!)" } else { "" }
        ));
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
    output::success(format!(
        "project '{}' removed{}",
        resp.project,
        if resp.volumes_removed {
            " (volumes included)"
        } else {
            " (volumes kept)"
        }
    ));
    if let Some(hostname) = resp.hostname {
        output::note(format!(
            "the DNS record for {hostname} may still exist; delete it manually: Cloudflare dashboard -> your zone -> DNS -> remove the {hostname} CNAME"
        ));
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
    let mut table = output::table();
    table.set_header(output::header(["FIELD", "VALUE"]));
    table.add_row(vec![
        "agent".to_string(),
        format!("v{} (cli v{})", resp.version, env!("CARGO_PKG_VERSION")),
    ]);
    table.add_row(vec!["uptime".to_string(), human_duration(resp.uptime_secs)]);
    table.add_row(vec![
        "disk".to_string(),
        format!("{}% used", resp.disk_used_percent),
    ]);
    table.add_row(vec!["projects".to_string(), resp.projects.to_string()]);
    table.add_row(vec![
        "active deployments".to_string(),
        resp.active_deployments.to_string(),
    ]);
    println!("{table}");
}

pub(crate) fn render_doctor(checks: &[DiagnosticCheckDto]) -> (String, bool) {
    let mut out = String::new();
    let mut ok = true;
    for c in checks {
        let mark = if c.passed {
            output::styled_ok("PASS")
        } else {
            ok = false;
            output::styled_err("FAIL")
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
            Some("is rpi-agent.service running on the Pi? try `rpi agent status`"),
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
            output::warn(format!("agent API unreachable ({err})"));
            output::note(format!(
                "falling back to: ssh {}@{} systemctl status rpi-agent",
                profile.user, profile.host
            ));
            SshExec { profile: &profile }
                .run(&["systemctl", "status", "rpi-agent", "--no-pager"])
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
    let mut args: Vec<String> = ["journalctl", "-u", "rpi-agent", "--no-pager", "-n"]
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
            output::warn(format!("agent API unreachable ({err})"));
            let since_unix = since
                .as_deref()
                .and_then(|s| parse_duration_secs(s).ok())
                .map(|secs| now - secs as i64);
            let args = journalctl_args(follow, since_unix, tail);
            let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
            output::note(format!(
                "falling back to: ssh {}@{} {}",
                profile.user,
                profile.host,
                args.join(" ")
            ));
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
            "CLI v{cli_version} and agent v{agent_version} differ - \
rebuild/update the agent on the Pi (`rpi agent update` ships in v0.5)"
        )
    })
}

/// A deploy log line the agent marked as a warning — re-surfaced next to the
/// final summary so it cannot scroll away with the stream.
fn deploy_warning(line: &str) -> Option<&str> {
    line.strip_prefix("warning: ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::rpitoml::SecretsSection;

    fn section(env: Option<&str>, files: &[&str]) -> SecretsSection {
        SecretsSection {
            env: env.map(str::to_string),
            files: files.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn deploy_warning_extracts_only_prefixed_lines() {
        assert_eq!(deploy_warning("warning: x y"), Some("x y"));
        assert_eq!(deploy_warning("ingress: routing a -> b"), None);
        assert_eq!(deploy_warning(" warning: not at start"), None);
    }

    #[test]
    fn collect_reads_env_and_files_as_base64() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "A=1\n").unwrap();
        std::fs::create_dir_all(dir.path().join("certs")).unwrap();
        std::fs::write(dir.path().join("certs/server.pem"), b"PEM").unwrap();

        let (vars, files) =
            collect_secrets(dir.path(), &section(None, &["certs/server.pem"])).unwrap();
        assert_eq!(vars["A"], "1");
        assert_eq!(files["certs/server.pem"], "UEVN"); // base64("PEM")
    }

    #[test]
    fn explicit_env_file_must_exist_but_default_is_optional() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), b"x").unwrap();

        let err = collect_secrets(dir.path(), &section(Some(".env.prod"), &[])).unwrap_err();
        assert!(err.to_string().contains(".env.prod"), "got: {err}");

        let (vars, files) = collect_secrets(dir.path(), &section(None, &["f.txt"])).unwrap();
        assert!(vars.is_empty(), "missing default .env is fine");
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn all_missing_files_are_reported_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let err = collect_secrets(dir.path(), &section(None, &["a.pem", "b.pem"])).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("a.pem") && msg.contains("b.pem"), "got: {msg}");
    }

    #[test]
    fn oversized_file_is_rejected_locally() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("big.bin"),
            vec![0u8; crate::proto::MAX_SECRET_FILE_BYTES + 1],
        )
        .unwrap();
        let err = collect_secrets(dir.path(), &section(None, &["big.bin"])).unwrap_err();
        assert!(err.to_string().contains("1 MiB"), "got: {err}");
    }

    /// Defense-in-depth: `rpi.toml`'s own `validate_rel_path` should already
    /// reject a literal `..` in `[secrets].files`, but `collect_secrets` must
    /// not blindly trust that upstream check either (a `SecretsSection` can
    /// be built directly, and this is also the last line of defense against
    /// a symlink resolving outside the root, exercised by the `cfg(unix)`
    /// tests below).
    #[test]
    fn collect_secrets_rejects_file_path_escaping_root() {
        let dir = tempfile::tempdir().unwrap();
        let outer = tempfile::tempdir().unwrap();
        std::fs::write(outer.path().join("escaped.txt"), b"outside-secret").unwrap();
        let outer_name = outer.path().file_name().unwrap().to_str().unwrap();
        let rel = format!("../{outer_name}/escaped.txt");

        let result = collect_secrets(dir.path(), &section(None, &[&rel]));
        assert!(
            result.is_err(),
            "must not read a file outside the project root"
        );
    }

    #[test]
    fn collect_secrets_rejects_env_path_escaping_root() {
        let dir = tempfile::tempdir().unwrap();
        let outer = tempfile::tempdir().unwrap();
        std::fs::write(outer.path().join("prod.env"), b"SECRET=leak\n").unwrap();
        let outer_name = outer.path().file_name().unwrap().to_str().unwrap();
        let rel = format!("../{outer_name}/prod.env");

        let result = collect_secrets(dir.path(), &section(Some(&rel), &[]));
        assert!(
            result.is_err(),
            "must not read an env file outside the project root"
        );
    }

    #[cfg(unix)]
    #[test]
    fn collect_secrets_rejects_symlinked_file_entry() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("id_rsa"), b"PRIVATE-KEY").unwrap();
        std::os::unix::fs::symlink(outside.path().join("id_rsa"), dir.path().join("certs.pem"))
            .unwrap();

        let result = collect_secrets(dir.path(), &section(None, &["certs.pem"]));
        assert!(
            result.is_err(),
            "must not follow a symlink out of the project root"
        );
    }

    #[cfg(unix)]
    #[test]
    fn collect_secrets_rejects_symlinked_default_env_file() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("real.env"), b"SECRET=leak\n").unwrap();
        std::os::unix::fs::symlink(outside.path().join("real.env"), dir.path().join(".env"))
            .unwrap();

        let result = collect_secrets(dir.path(), &section(None, &[]));
        assert!(
            result.is_err(),
            "must not follow a symlink out of the project root"
        );
    }

    #[cfg(unix)]
    #[test]
    fn collect_secrets_rejects_symlinked_explicit_env_file() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("real.env"), b"SECRET=leak\n").unwrap();
        std::os::unix::fs::symlink(outside.path().join("real.env"), dir.path().join("prod.env"))
            .unwrap();

        let result = collect_secrets(dir.path(), &section(Some("prod.env"), &[]));
        assert!(
            result.is_err(),
            "must not follow a symlink out of the project root"
        );
    }

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
            vec!["journalctl", "-u", "rpi-agent", "--no-pager", "-n", "100"]
        );
        assert_eq!(
            journalctl_args(true, Some(1234), 50),
            vec![
                "journalctl",
                "-u",
                "rpi-agent",
                "--no-pager",
                "-n",
                "50",
                "--since=@1234",
                "-f"
            ]
        );
    }

    #[cfg(test)]
    mod command_list_tests {
        use super::format_command_line;
        use pi_domain::entities::CommandSpec;

        #[test]
        fn service_less_command_shows_argv_only() {
            let spec = CommandSpec::new(vec!["node".into(), "seed.js".into()]);
            assert_eq!(format_command_line("seed", &spec), "seed  ->  node seed.js");
        }

        #[test]
        fn service_pinned_command_shows_service_suffix() {
            let spec = CommandSpec {
                argv: vec!["node".into(), "x.cjs".into()],
                service: Some("server".into()),
            };
            assert_eq!(
                format_command_line("create-invite", &spec),
                "create-invite  ->  node x.cjs  [service: server]"
            );
        }
    }
}
