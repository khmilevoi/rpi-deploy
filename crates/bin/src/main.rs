mod agent;
mod cli;
mod proto;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "pi", version, about = "deploy tool for Raspberry Pi (CLI + agent)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Deploy current project (reads ./pi.toml)
    Deploy {
        /// Branch or commit-sha (default — branch from pi.toml)
        #[arg(long = "ref")]
        git_ref: Option<String>,
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
    /// Agent management
    Agent {
        #[command(subcommand)]
        cmd: AgentCmd,
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
        Cmd::Deploy { git_ref, server } => cli::commands::deploy(git_ref, server).await,
        Cmd::Ls { server } => cli::commands::ls(server).await,
        Cmd::Agent { cmd: AgentCmd::Run { config } } => agent::run::run(config).await,
    }
}
