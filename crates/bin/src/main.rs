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
    /// Деплой текущего проекта (читает ./pi.toml)
    Deploy {
        /// Ветка или commit-sha (дефолт — branch из pi.toml)
        #[arg(long = "ref")]
        git_ref: Option<String>,
        /// Профиль сервера из ~/.config/pi/config.toml
        #[arg(long)]
        server: Option<String>,
    },
    /// Список проектов на агенте
    #[command(alias = "ps")]
    Ls {
        #[arg(long)]
        server: Option<String>,
    },
    /// Управление агентом
    Agent {
        #[command(subcommand)]
        cmd: AgentCmd,
    },
}

#[derive(Subcommand)]
enum AgentCmd {
    /// Запустить агент (foreground; под systemd)
    Run {
        /// Путь к agent.toml (дефолт: /etc/pi/agent.toml)
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
