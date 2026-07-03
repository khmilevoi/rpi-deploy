---
name: pi-cli
description: Use when operating, installing, testing, or troubleshooting the rpi deploy CLI, including rpi deploy, rpi ls, rpi env send, rpi env ls, rpi gc, rpi agent run, SSH profiles, PI_SERVER, PI_AGENT_URL, local dev agents, and CLI-to-agent connection failures.
---

# Pi CLI

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
| Send env bundle | `rpi env send` |
| Send env bundle and restart running stack | `rpi env send --apply` |
| List stored env keys | `rpi env ls` |
| Prune agent Docker images/build cache | `rpi gc` |
| Run foreground agent | `rpi agent run --config <agent.toml>` |

Remote commands accept either a named profile or direct SSH flags:

```bash
rpi ls --server home
PI_SERVER=home rpi ls
rpi deploy --host pi-host.local --user pi-user --key ~/.ssh/id_ed25519_pi
```

Do not combine `--server` with `--host`; direct `--host` mode requires `--user`.

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
4. If `[env] file = ".env"` is used and secrets are required, run `rpi env send` before the first deploy.
5. Prefer Compose `expose` for the managed service; avoid fixed host `ports` that conflict with rpi's allocator.

## Troubleshooting

For connection failures, isolate layers in this order:

1. SSH from the developer machine: `ssh -i <key> <user>@<host> true`
2. Agent service on the Pi: `systemctl status pi-agent`
3. Agent logs: `journalctl -u pi-agent -n 100 --no-pager`
4. Socket permissions: `ls -l /run/pi/agent.sock` and `groups "$USER"`
5. Direct socket API on the Pi: `curl --unix-socket /run/pi/agent.sock http://localhost/v1/version`

For deploy failures:

- `Permission denied (publickey)`: the Pi cannot fetch `source.repo`; add the printed deploy key to the repository.
- Docker `/home/pi-agent` errors: ensure the systemd unit sets `HOME=/var/lib/pi`, `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, and `WorkingDirectory=/var/lib/pi`.
- Compose does not see secrets: run `rpi env send`, or `rpi env send --apply` for an already running stack.
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
