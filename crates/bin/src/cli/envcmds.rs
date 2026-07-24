use crate::cli::config::ConnectOpts;
use crate::cli::connect::AgentConn;
use crate::output;

/// Derive the target key from ./rpi.toml + overlay: full resolution, so the
/// same validation applies as on deploy.
fn resolve_key(env: &str, vars: &[String]) -> anyhow::Result<String> {
    let resolved = crate::cli::overlay::resolve(Some(env), vars)?;
    Ok(resolved
        .env
        .expect("resolve with env returns selection")
        .key)
}

fn confirm_key(action: &str, key: &str, yes: bool) -> anyhow::Result<()> {
    if yes {
        return Ok(());
    }
    output::warn(format!("this will {action} environment '{key}'"));
    eprint!("type the environment key to confirm: ");
    use std::io::Write;
    std::io::stderr().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if input.trim() != key {
        anyhow::bail!("confirmation failed: expected '{key}'");
    }
    Ok(())
}

pub async fn env_ls(all: bool, connect: ConnectOpts) -> anyhow::Result<()> {
    let base = if all {
        None
    } else {
        match crate::cli::overlay::resolve(None, &[]) {
            Ok(r) => Some(r.rpitoml.project.name),
            Err(_) => {
                anyhow::bail!("no rpi.toml in the current directory - use `rpi env ls --all`")
            }
        }
    };
    let AgentConn {
        tunnel: _tunnel,
        api,
        compat,
    } = crate::cli::connect::connect_agent(connect).await?;
    compat.gate(crate::compat::Feature::Environments)?;
    let envs = api.list_environments(base.as_deref()).await?;
    if envs.is_empty() {
        output::info("no environments registered");
        return Ok(());
    }
    let mut table = output::table();
    table.set_header(output::header([
        "KEY",
        "BASE",
        "ENV",
        "SLUG",
        "LAST DEPLOY",
        "TTL",
    ]));
    for e in envs {
        table.add_row(vec![
            output::cell(e.key),
            output::cell(e.base),
            output::cell(e.env),
            output::cell(e.slug.unwrap_or_else(|| "-".into())),
            output::cell(
                e.last_success_at
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "-".into()),
            ),
            output::cell(
                e.ttl_secs
                    .map(|t| format!("{t}s"))
                    .unwrap_or_else(|| "-".into()),
            ),
        ]);
    }
    println!("{table}");
    Ok(())
}

pub async fn env_destroy(
    env: String,
    vars: Vec<String>,
    yes: bool,
    connect: ConnectOpts,
) -> anyhow::Result<()> {
    let key = resolve_key(&env, &vars)?;
    confirm_key(
        "DESTROY (stack, volumes, ingress, DNS, secrets, registry) of",
        &key,
        yes,
    )?;
    let AgentConn {
        tunnel: _tunnel,
        api,
        compat,
    } = crate::cli::connect::connect_agent(connect).await?;
    compat.gate(crate::compat::Feature::Environments)?;
    let resp = api.destroy_environment(&key).await?;
    if resp.already_absent {
        output::info(format!(
            "environment '{key}' does not exist - nothing to destroy"
        ));
    } else {
        output::success(format!("environment '{key}' destroyed"));
    }
    Ok(())
}

pub async fn env_reset_data(
    env: String,
    vars: Vec<String>,
    yes: bool,
    connect: ConnectOpts,
) -> anyhow::Result<()> {
    let key = resolve_key(&env, &vars)?;
    confirm_key("REMOVE ALL DATA (volumes) of", &key, yes)?;
    let AgentConn {
        tunnel: _tunnel,
        api,
        compat,
    } = crate::cli::connect::connect_agent(connect).await?;
    compat.gate(crate::compat::Feature::Environments)?;
    api.reset_environment(&key).await?;
    output::success(format!(
        "environment '{key}' data removed - the next `rpi deploy --env {env}` re-runs on_create"
    ));
    Ok(())
}
