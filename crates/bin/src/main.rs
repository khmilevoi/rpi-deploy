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
        /// Server profile from ~/.config/pi/config.toml
        #[arg(long)]
        server: Option<String>,
    },
    /// List projects on the agent
    #[command(alias = "ps")]
    Ls {
        #[arg(long)]
        server: Option<String>,
    },
    /// Prune docker images and build cache on the agent (§8.1)
    Gc {
        #[arg(long)]
        server: Option<String>,
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
        /// Server profile from ~/.config/pi/config.toml
        #[arg(long)]
        server: Option<String>,
    },
    /// List secret keys stored on the agent (values are never transmitted)
    Ls {
        /// Server profile from ~/.config/pi/config.toml
        #[arg(long)]
        server: Option<String>,
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
            server,
        } => {
            if cancel {
                cli::commands::deploy_cancel(server).await
            } else {
                cli::commands::deploy(git_ref, server).await
            }
        }
        Cmd::Ls { server } => cli::commands::ls(server).await,
        Cmd::Gc { server } => cli::commands::gc(server).await,
        Cmd::Env {
            cmd: EnvCmd::Send { apply, server },
        } => cli::commands::env_send(apply, server).await,
        Cmd::Env {
            cmd: EnvCmd::Ls { server },
        } => cli::commands::env_ls(server).await,
        Cmd::Agent {
            cmd: AgentCmd::Run { config },
        } => agent::run::run(config).await,
    }
}
