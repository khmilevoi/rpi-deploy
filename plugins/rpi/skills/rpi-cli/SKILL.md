---
name: rpi-cli
description: Use when operating, installing, testing, or troubleshooting the rpi deploy CLI, including rpi deploy, rpi ls, rpi secrets send, rpi secrets ls, rpi command, rpi logs, rpi stats, rpi start/stop/restart/rm, rpi status, rpi doctor, rpi gc, rpi agent run, rpi setup, rpi init, rpi agent setup, SSH profiles, PI_SERVER, PI_AGENT_URL, local dev agents, and CLI-to-agent connection failures.
---

# Rpi CLI

## Overview

Use this skill for commands and workflows around the `rpi` binary. Treat the repository README and CLI source as the source of truth when behavior has drifted.

Primary references in this repo:

- `README.md`
- `crates/bin/src/main.rs`
- `crates/bin/src/cli/config.rs`
- `crates/bin/src/cli/commands.rs`

## Command Map

| Task | Command |
| --- | --- |
| Deploy current project | `rpi deploy` |
| Deploy a ref | `rpi deploy --ref <branch-tag-or-sha>` |
| Cancel active deploys for current `rpi.toml` project | `rpi deploy --cancel` |
| List projects | `rpi ls` or `rpi ps` |
| Send secrets bundle (env + files) | `rpi secrets send` |
| Send secrets bundle and restart running stack | `rpi secrets send --apply` |
| List stored secret keys | `rpi secrets ls` |
| Stream container logs | `rpi logs <project> [-f] [--tail N]` |
| Live CPU/memory/disk metrics | `rpi stats [project]` |
| Start / stop / restart project containers | `rpi start\|stop\|restart <project>` |
| Remove a project | `rpi rm <project> [--volumes]` |
| Agent and host overview | `rpi status` |
| Environment self-diagnosis | `rpi doctor` |
| Prune agent Docker images/build cache | `rpi gc` |
| List commands deployed on the agent | `rpi command` |
| Run a deployed `[commands]` entry | `rpi command <name>` |
| Run a deployed entry with extra args | `rpi command <name> -- <extra-args>` |
| Run foreground agent | `rpi agent run --config <agent.toml>` |
| Agent status on the Pi | `rpi agent status` |
| Agent logs on the Pi | `rpi agent logs [-f] [--since 2h]` |
| One-command developer setup | `rpi setup` |
| Scaffold a new `rpi.toml` | `rpi init` |
| Install/configure the agent on the Pi | `rpi agent setup` |
| Uninstall the agent (keeps data unless `--purge`) | `rpi agent uninstall` |

Remote commands accept either a named profile or direct SSH flags:

```bash
rpi ls --server home
PI_SERVER=home rpi ls
rpi deploy --host pi-host.local --user pi-user --key ~/.ssh/id_ed25519_pi
```

Do not combine `--server` with `--host`; direct `--host` mode requires `--user`.

## Running Admin Commands

`rpi command` runs entries declared in `[commands]` in `rpi.toml`, inside the
`ingress.service` container on the agent:

```bash
rpi command                                   # list mode: commands deployed on the agent
rpi command create-invite                     # run mode: execute a deployed command
rpi command create-invite -- --email x@y.com  # `--` separates extra args, appended to the declared argv
```

- The remote exit code becomes the `rpi` exit code.
- Ctrl+C detaches and best-effort kills the run on the agent; the in-container
  process may still survive, per standard `docker exec` behavior.
- A 404 from an old agent that predates `[commands]` support surfaces as
  "agent does not support [commands]; update rpi-agent on the Pi" — update the
  agent binary and redeploy.

## Client Profile

The CLI reads the user config at:

- Windows: `%APPDATA%\pi\config.toml`
- macOS/Linux: `~/.config/pi/config.toml`

Minimal config:

```toml
default = "home"

[servers.home]
host = "pi-host.local"
user = "pi-user"
key = "~/.ssh/id_ed25519_pi"
```

Selection order is `--server`, then `PI_SERVER`, then `default`, then the only configured server if exactly one exists.

## Local Development

From this repository, run a TCP agent:

```bash
cargo run -p pi -- agent run --config dev/agent.toml
```

Point the CLI to it:

```bash
export PI_AGENT_URL="http://127.0.0.1:7700"
```

PowerShell:

```powershell
$env:PI_AGENT_URL = "http://127.0.0.1:7700"
```

Use local mode for CLI/API testing. Use SSH profile mode when validating real Pi connectivity.

## Deployment Checklist

Before `rpi deploy`:

1. Run from the deployable project's root, not necessarily from this repository root.
2. Confirm `./rpi.toml` exists and has the intended project name, repo, branch, service, and port.
3. Confirm the Pi can read `source.repo`; private repos may require a deploy key on the Pi.
4. If `[secrets]` is configured (env file and/or files) and secrets are required, run `rpi secrets send` before the first deploy.
5. Prefer Compose `expose` for the managed service; avoid fixed host `ports` that conflict with rpi's allocator.

## Troubleshooting

For connection failures, isolate layers in this order:

1. SSH from the developer machine: `ssh -i <key> <user>@<host> true`
2. Agent service on the Pi: `systemctl status rpi-agent`
3. Agent logs: `journalctl -u rpi-agent -n 100 --no-pager`
4. Socket permissions: `ls -l /run/rpi/agent.sock` and `groups "$USER"`
5. Direct socket API on the Pi: `curl --unix-socket /run/rpi/agent.sock http://localhost/v1/version`

For deploy failures:

- `Permission denied (publickey)`: the Pi cannot fetch `source.repo`; add the printed deploy key to the repository.
- Docker `/home/rpi-agent` errors: ensure the systemd unit sets `HOME=/var/lib/rpi`, `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, and `WorkingDirectory=/var/lib/rpi`.
- Compose does not see secrets: run `rpi secrets send`, or `rpi secrets send --apply` for an already running stack.
- Health check fails: verify the app listens on `0.0.0.0`, `[ingress].port` is the container port, and `[healthcheck]` matches the endpoint.
- Host port conflict: remove fixed host `ports:` from Compose and let rpi write the override.

## Editing The CLI

When changing CLI behavior, update both implementation and documentation:

- CLI shape: `crates/bin/src/main.rs`
- profile resolution: `crates/bin/src/cli/config.rs`
- command behavior: `crates/bin/src/cli/commands.rs`
- user-facing docs and examples: `README.md`

Run focused tests first, then the workspace suite when practical:

```bash
cargo test -p pi
cargo test --workspace
```
