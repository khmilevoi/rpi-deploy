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

#[derive(clap::Args)]
struct InitArgs {
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    repo: Option<String>,
    #[arg(long)]
    branch: Option<String>,
    #[arg(long)]
    compose: Option<String>,
    #[arg(long)]
    service: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    hostname: Option<String>,
    #[arg(long)]
    expose: Option<String>,
    #[arg(long = "env")]
    env_file: Option<String>,
    #[arg(long)]
    yes: bool,
}

#[derive(clap::Args)]
struct SetupArgs {
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    user: Option<String>,
    #[arg(long)]
    key: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    default: bool,
    #[arg(long)]
    yes: bool,
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
    /// Stream container logs of a project
    Logs {
        project: String,
        #[arg(short, long)]
        follow: bool,
        #[arg(long, default_value_t = 100)]
        tail: usize,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Live CPU/memory/disk metrics
    Stats {
        project: Option<String>,
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Start project containers (no rebuild)
    Start {
        project: String,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Stop project containers
    Stop {
        project: String,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Restart project containers
    Restart {
        project: String,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Remove a project
    Rm {
        project: String,
        #[arg(long)]
        volumes: bool,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Agent and host overview
    Status {
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Environment self-diagnosis
    Doctor {
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Generate pi.toml in the current project (wizard; flags for CI)
    Init(InitArgs),
    /// Configure a server profile on this machine (wizard; flags for CI)
    Setup(SetupArgs),
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
    /// Agent overview; falls back to systemctl over ssh
    Status {
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Agent logs; falls back to journalctl over ssh
    Logs {
        #[arg(short, long)]
        follow: bool,
        #[arg(long)]
        since: Option<String>,
        #[arg(long, default_value_t = 100)]
        tail: usize,
        #[command(flatten)]
        connect: cli::config::ConnectOpts,
    },
    /// Bootstrap the agent on this Pi (run with sudo; idempotent)
    Setup {
        /// SSH login user to add to the pi-agent group (default: $SUDO_USER)
        #[arg(long)]
        user: Option<String>,
        /// Also bootstrap cloudflared (linger + user unit)
        #[arg(long)]
        with_cloudflared: bool,
        /// Print the plan without changing anything
        #[arg(long)]
        dry_run: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if matches!(
        cli.cmd,
        Cmd::Agent {
            cmd: AgentCmd::Run { .. }
        }
    ) {
        return match cli.cmd {
            Cmd::Agent {
                cmd: AgentCmd::Run { config },
            } => agent::run::run(config).await,
            _ => unreachable!(),
        };
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    match cli.cmd {
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
        Cmd::Logs {
            project,
            follow,
            tail,
            connect,
        } => cli::commands::logs(project, follow, tail, connect).await,
        Cmd::Stats {
            project,
            json,
            connect,
        } => cli::commands::stats(project, json, connect).await,
        Cmd::Start { project, connect } => {
            cli::commands::lifecycle(project, "start", connect).await
        }
        Cmd::Stop { project, connect } => cli::commands::lifecycle(project, "stop", connect).await,
        Cmd::Restart { project, connect } => {
            cli::commands::lifecycle(project, "restart", connect).await
        }
        Cmd::Rm {
            project,
            volumes,
            yes,
            connect,
        } => cli::commands::rm(project, volumes, yes, connect).await,
        Cmd::Status { json, connect } => cli::commands::status(json, connect).await,
        Cmd::Doctor { connect } => cli::commands::doctor(connect).await,
        Cmd::Init(a) => {
            cli::init::run(cli::init::InitFlags {
                name: a.name,
                repo: a.repo,
                branch: a.branch,
                compose: a.compose,
                service: a.service,
                port: a.port,
                hostname: a.hostname,
                expose: a.expose,
                env_file: a.env_file,
                yes: a.yes,
            })
            .await
        }
        Cmd::Setup(a) => {
            cli::setup::run(cli::setup::SetupFlags {
                host: a.host,
                user: a.user,
                key: a.key,
                name: a.name,
                default: a.default,
                yes: a.yes,
            })
            .await
        }
        Cmd::Env {
            cmd: EnvCmd::Send { apply, connect },
        } => cli::commands::env_send(apply, connect).await,
        Cmd::Env {
            cmd: EnvCmd::Ls { connect },
        } => cli::commands::env_ls(connect).await,
        Cmd::Agent {
            cmd: AgentCmd::Run { .. },
        } => unreachable!(),
        Cmd::Agent {
            cmd: AgentCmd::Status { connect },
        } => cli::commands::agent_status(connect).await,
        Cmd::Agent {
            cmd:
                AgentCmd::Logs {
                    follow,
                    since,
                    tail,
                    connect,
                },
        } => cli::commands::agent_logs(follow, since, tail, connect).await,
        Cmd::Agent {
            cmd: AgentCmd::Setup { user, with_cloudflared, dry_run },
        } => agent::setup::run_cmd(user, with_cloudflared, dry_run).await,
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

    #[test]
    fn agent_logs_flags_parse() {
        let cli = Cli::try_parse_from(["pi", "agent", "logs", "-f", "--since", "2h"]).unwrap();
        match cli.cmd {
            Cmd::Agent {
                cmd:
                    AgentCmd::Logs {
                        follow,
                        since,
                        tail,
                        ..
                    },
            } => {
                assert!(follow);
                assert_eq!(since.as_deref(), Some("2h"));
                assert_eq!(tail, 100);
            }
            _ => panic!("expected agent logs"),
        }
    }

    #[test]
    fn init_flags_parse() {
        let cli = Cli::try_parse_from([
            "pi", "init", "--name", "rateme", "--port", "3000", "--expose", "lan", "--yes",
        ])
        .unwrap();
        match cli.cmd {
            Cmd::Init(args) => {
                assert_eq!(args.name.as_deref(), Some("rateme"));
                assert_eq!(args.port, Some(3000));
                assert_eq!(args.expose.as_deref(), Some("lan"));
                assert!(args.yes);
            }
            _ => panic!("expected init"),
        }
    }

    #[test]
    fn setup_flags_parse() {
        let cli = Cli::try_parse_from([
            "pi", "setup", "--host", "pihost.local", "--user", "piuser", "--key", "~/.ssh/pi", "--yes",
        ])
        .unwrap();
        match cli.cmd {
            Cmd::Setup(a) => {
                assert_eq!(a.host.as_deref(), Some("pihost.local"));
                assert_eq!(a.user.as_deref(), Some("piuser"));
                assert!(a.yes);
            }
            _ => panic!("expected setup"),
        }
    }

    #[test]
    fn agent_setup_flags_parse() {
        let cli = Cli::try_parse_from(["pi", "agent", "setup", "--user", "piuser", "--dry-run"]).unwrap();
        match cli.cmd {
            Cmd::Agent { cmd: AgentCmd::Setup { user, with_cloudflared, dry_run } } => {
                assert_eq!(user.as_deref(), Some("piuser"));
                assert!(!with_cloudflared);
                assert!(dry_run);
            }
            _ => panic!("expected agent setup"),
        }
    }
}
