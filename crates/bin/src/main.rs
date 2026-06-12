mod agent;
mod cli;
mod duration;
mod proto;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "pi",
    version,
    about = "deploy tool for Raspberry Pi (CLI + agent)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Deploy current project (reads ./pi.toml)
    Deploy {
        /// Branch or commit-sha (default — branch from pi.toml)
        #[arg(long = "ref", conflicts_with = "cancel")]
        git_ref: Option<String>,
        /// Cancel the active deploy(s) of the current project instead
        #[arg(long)]
        cancel: bool,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// List projects on the agent
    #[command(alias = "ps")]
    Ls {
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Prune docker images and build cache on the agent (§8.1)
    Gc {
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Manage project secrets
    Env {
        #[command(subcommand)]
        cmd: EnvCmd,
    },
    /// Agent management
    Agent {
        #[command(subcommand)]
        cmd: AgentCmd,
    },
}

#[derive(Subcommand)]
enum EnvCmd {
    /// Send secrets from local .env file to the agent
    Send {
        /// Also apply the new secrets to running containers
        #[arg(long)]
        apply: bool,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// List secret keys stored on the agent (values are never transmitted)
    Ls {
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
}

#[derive(Subcommand)]
enum AgentCmd {
    /// Start the agent (foreground; under systemd)
    Run {
        /// Path to agent.toml (default: /etc/pi/agent.toml)
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    match Cli::parse().cmd {
        Cmd::Deploy {
            git_ref,
            cancel,
            connect,
        } => {
            if cancel {
                cli::commands::deploy_cancel(connect).await
            } else {
                cli::commands::deploy(git_ref, connect).await
            }
        }
        Cmd::Ls { connect } => cli::commands::ls(connect).await,
        Cmd::Gc { connect } => cli::commands::gc(connect).await,
        Cmd::Env {
            cmd: EnvCmd::Send { apply, connect },
        } => cli::commands::env_send(apply, connect).await,
        Cmd::Env {
            cmd: EnvCmd::Ls { connect },
        } => cli::commands::env_ls(connect).await,
        Cmd::Agent {
            cmd: AgentCmd::Run { config },
        } => agent::run::run(config).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn deploy_host_requires_user() {
        assert!(Cli::try_parse_from(["pi", "deploy", "--host", "203.0.113.7"]).is_err());
    }

    #[test]
    fn deploy_ci_flags_parse() {
        let cli = Cli::try_parse_from([
            "pi",
            "deploy",
            "--host",
            "203.0.113.7",
            "--user",
            "pi",
            "--key",
            "./k",
        ])
        .unwrap();
        match cli.cmd {
            Cmd::Deploy { connect, .. } => {
                assert_eq!(connect.host.as_deref(), Some("203.0.113.7"));
                assert_eq!(connect.user.as_deref(), Some("pi"));
                assert_eq!(connect.key.as_deref(), Some("./k"));
            }
            _ => panic!("expected deploy"),
        }
    }

    #[test]
    fn server_flag_conflicts_with_host() {
        assert!(Cli::try_parse_from([
            "pi",
            "deploy",
            "--server",
            "home",
            "--host",
            "203.0.113.7",
            "--user",
            "pi",
        ])
        .is_err());
    }
}
