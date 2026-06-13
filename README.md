# pi

`pi` is a deployment tool for Docker Compose projects on Raspberry Pi. The Pi
runs an agent, while the CLI runs on a developer machine or in CI. The CLI
connects to the agent through an SSH tunnel; the agent clones the Git
repository, builds the Compose stack, and starts the containers.

Status: v0.4 (Операционка) — deploy/env/ingress/CI (v0.1-v0.3) + `pi logs`,
`pi stats`, `pi start|stop|restart`, `pi rm`, `pi status`, `pi doctor`,
`pi agent status|logs`, rolling agent logs. One-command install and
`pi agent setup` are planned for v0.5 (§23 spec).

Supported features:

- `pi deploy`;
- `pi deploy --cancel`;
- `pi ls`;
- `pi env send`;
- `pi env ls`;
- `pi gc`;
- `pi logs <project> [-f]`;
- `pi stats [project]`;
- `pi start|stop|restart <project>`;
- `pi rm <project> [--volumes]`;
- `pi status`;
- `pi doctor`;
- `pi agent status`;
- `pi agent logs [-f] [--since 2h]`;
- stable host port allocation;
- Docker Compose overrides;
- health checks;
- a latest-wins deployment queue;
- encrypted env bundles;
- optional Cloudflare Tunnel ingress automation.

## Runtime Model

`pi` has two parts:

- `pi agent run` is a daemon on the Raspberry Pi, usually managed by `systemd`.
  It stores state in SQLite, selects a stable host port, writes a Compose
  override, runs `docker compose build`, and then runs
  `docker compose up -d --remove-orphans`.
- `pi deploy`, `pi ls`, `pi env ...`, and `pi gc` are client commands that run
  on a developer machine or CI runner. They open an SSH tunnel to the agent's
  Unix socket.

Each deployable project must contain a `pi.toml` file. Run `pi deploy` from the
root of the project you want to deploy, not necessarily from the root of this
repository.

## Requirements

On the Raspberry Pi:

- Raspberry Pi OS 64-bit, or another Linux system with `systemd`.
- SSH access from the developer machine or CI.
- `git`.
- Docker Engine.
- Docker Compose plugin, available as `docker compose`.
- Rust toolchain, only if building the binary directly on the Pi.

On the developer machine:

- Git.
- OpenSSH client: `ssh`, `scp`.
- Rust toolchain, if installing the CLI from source.

Check the Pi:

```bash
docker --version
docker compose version
git --version
```

## Prepare The Raspberry Pi

Install base packages:

```bash
sudo apt update
sudo apt install -y git curl build-essential pkg-config libssl-dev
```

Install Docker:

```bash
curl -fsSL https://get.docker.com | sh
sudo usermod -aG docker "$USER"
```

After adding the user to the `docker` group, open a new SSH session and check:

```bash
docker ps
docker compose version
```

If `docker ps` still requires `sudo`, the current session has not picked up the
new group membership.

## Configure SSH

Create a key on the developer machine.

PowerShell:

```powershell
ssh-keygen -t ed25519 -f $env:USERPROFILE\.ssh\id_ed25519_pi
type $env:USERPROFILE\.ssh\id_ed25519_pi.pub
```

Unix shell:

```bash
mkdir -p ~/.ssh
chmod 700 ~/.ssh
ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519_pi
cat ~/.ssh/id_ed25519_pi.pub
```

Add the public key to `~/.ssh/authorized_keys` for the Pi user that the CLI will
use for SSH:

```bash
mkdir -p ~/.ssh
chmod 700 ~/.ssh
nano ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys
```

Test from the developer machine:

```bash
ssh -i ~/.ssh/id_ed25519_pi pi-user@pi-host.local true
```

Replace `pi-user` and `pi-host.local` with the user and host for your Pi. The
`true` command does nothing and exits successfully, which makes it useful for
testing SSH key access.

If the key uses a non-default filename, you can add an SSH profile:

```sshconfig
Host pi-home
    HostName pi-host.local
    User pi-user
    IdentityFile ~/.ssh/id_ed25519_pi
    IdentitiesOnly yes
```

Test it:

```bash
ssh pi-home true
```

## Build And Install The Binary

### Option A: Build On The Pi

This is the simplest option, but it can be slow on smaller boards.

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

git clone <this-repository-url> pi
cd pi
cargo build --release
sudo install -m 755 target/release/pi /usr/local/bin/pi
```

Check it:

```bash
pi --help
```

### Option B: Cross-Build On The Developer Machine

Run these commands from the root of this repository.

```bash
cargo install cross
cross build --release --target aarch64-unknown-linux-gnu
scp target/aarch64-unknown-linux-gnu/release/pi pi-user@pi-host.local:/tmp/pi
```

On the Pi:

```bash
sudo install -m 755 /tmp/pi /usr/local/bin/pi
pi --help
```

## Install `pi-agent`

Create the service user:

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin pi-agent || true
sudo usermod -aG docker pi-agent
sudo usermod -aG pi-agent "$USER"
```

`pi-agent` runs Docker. The login user is added to the `pi-agent` group so the
SSH tunnel can connect to the agent's Unix socket.

Create directories:

```bash
sudo mkdir -p /var/lib/pi /etc/pi
sudo chown -R pi-agent:pi-agent /var/lib/pi
```

Create `/etc/pi/agent.toml`:

```bash
sudo tee /etc/pi/agent.toml >/dev/null <<'EOF'
data_dir = "/var/lib/pi"
socket = "/run/pi/agent.sock"
port_min = 8000
port_max = 8999
build_concurrency = 1
history_keep = 50

[timeouts]
fetch = "2m"
build = "30m"
up = "5m"

[gc]
disk_threshold_percent = 85
EOF
```

Create the `systemd` unit:

```bash
sudo tee /etc/systemd/system/pi-agent.service >/dev/null <<'EOF'
[Unit]
Description=pi deploy agent
After=network-online.target docker.service
Wants=network-online.target

[Service]
User=pi-agent
Group=pi-agent
Environment=HOME=/var/lib/pi
Environment=XDG_CONFIG_HOME=/var/lib/pi/.config
Environment=XDG_CACHE_HOME=/var/lib/pi/.cache
WorkingDirectory=/var/lib/pi
ExecStart=/usr/local/bin/pi agent run --config /etc/pi/agent.toml
RuntimeDirectory=pi
RuntimeDirectoryMode=0750
Restart=on-failure

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable --now pi-agent
systemctl status pi-agent
```

Agent logs:

```bash
journalctl -u pi-agent -f
```

After adding the login user to the `pi-agent` group, open a new SSH session.

`pi-agent` is intentionally created without a home directory. Agent state lives
in `/var/lib/pi`, and the `HOME` and `XDG_*` variables prevent Docker/BuildKit
from trying to write to a missing `/home/pi-agent`.

## Install The CLI On A Developer Machine

Run these commands from the root of this repository.

PowerShell:

```powershell
cargo install --path .\crates\bin --locked
pi --help
```

If `pi` is not found in the current PowerShell session:

```powershell
$env:PATH += ";$env:USERPROFILE\.cargo\bin"
```

Unix shell:

```bash
cargo install --path crates/bin --locked
pi --help
```

If `pi` is not found:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

For a permanent setup, add that line to your shell startup file.

## Configure A Client Profile

The CLI reads config from the user config directory:

- Windows: `%APPDATA%\pi\config.toml`;
- macOS/Linux: `~/.config/pi/config.toml`.

PowerShell:

```powershell
New-Item -ItemType Directory -Force "$env:APPDATA\pi"
notepad "$env:APPDATA\pi\config.toml"
```

Unix shell:

```bash
mkdir -p ~/.config/pi
nano ~/.config/pi/config.toml
```

Example:

```toml
default = "home"

[servers.home]
host = "pi-host.local"
user = "pi-user"
key = "~/.ssh/id_ed25519_pi"
```

Check it:

```bash
pi ls
```

If no projects have been deployed yet, the expected output is:

```text
no projects deployed yet
```

For CI or a one-off command, you can skip the config file:

```bash
pi ls --host pi-host.local --user pi-user --key ~/.ssh/id_ed25519_pi
pi deploy --host pi-host.local --user pi-user --key ~/.ssh/id_ed25519_pi
```

Select a specific profile:

```bash
pi ls --server home
PI_SERVER=home pi ls
```

## Prepare A Project For Deployment

Add `pi.toml` to the root of the project you want to deploy.

### Web Service

```toml
schema = 1

[project]
name = "example-web"

[source]
repo = "<git-repository-url>"
branch = "main"

[build]
compose = "docker-compose.yml"

[ingress]
hostname = "app.example.com"
service = "web"
port = 3000

[healthcheck]
path = "/health"
expect = "200"
timeout = "60s"

[env]
file = ".env"
```

### Worker, Bot, Or Internal Service

If the service does not need public HTTP ingress, omit `hostname`.

```toml
schema = 1

[project]
name = "example-worker"

[source]
repo = "<git-repository-url>"
branch = "main"

[build]
compose = "docker-compose.yml"

[ingress]
service = "app"
port = 3000

[healthcheck]
path = "/health"
expect = "200"
timeout = "60s"

[env]
file = ".env"
```

Fields:

- `project.name` is the Compose project name and the state key used by the
  agent.
- `source.repo` is the Git URL cloned by the Pi.
- `source.branch` is the default ref used by `pi deploy`.
- `build.compose` is the Compose file inside the project repository.
- `ingress.service` is the service name in Compose.
- `ingress.port` is the port inside the container.
- `ingress.hostname` is optional; use it for public ingress.
- `healthcheck.path` is the HTTP endpoint checked through the allocated host
  port. If the path is not set, the agent uses a TCP probe.
- `env.file` is the local file read by `pi env send`.

Optional per-project timeouts:

```toml
[timeouts]
fetch = "3m"
build = "45m"
up = "2m"
```

## Docker Compose Requirements

`pi-agent` writes an override file roughly like this:

```yaml
services:
  web:
    ports:
      - "127.0.0.1:8000:3000"
```

For that reason, the production Compose file usually should not set a fixed
host port manually.

Recommended:

```yaml
services:
  web:
    build:
      context: .
    expose:
      - "3000"
```

Avoid this for a service managed by `pi`:

```yaml
services:
  web:
    ports:
      - "127.0.0.1:3000:3000"
```

That `ports` mapping can conflict with the agent's port allocator or with
another project on the same Pi.

For runtime files, mount directories instead of individual files that may not
exist in a fresh clone:

```yaml
services:
  app:
    environment:
      APP_LOG_FILE: /app/logs/app.log
    volumes:
      - ./logs:/app/logs
```

Ignore runtime directories in the application repository:

```gitignore
logs/
data/
```

```dockerignore
/logs
/data
```

For SQLite or other local state, mount a persistent directory:

```yaml
services:
  app:
    environment:
      DATABASE_URL: file:///data/app.db
    volumes:
      - ./data:/data
```

## Secrets And `.env`

If `pi.toml` contains:

```toml
[env]
file = ".env"
```

send secrets from the root of the deployable project:

```bash
pi env send
```

The CLI reads the local `.env`, sends the values to the agent, and the agent
stores an encrypted bundle in `/var/lib/pi/secrets`. During `pi deploy`, the
agent writes `.env` into the project workdir before running Docker Compose.

Before the first deploy of a project that needs secrets:

```bash
pi env send
pi deploy
```

After changing secrets for an already running project:

```bash
pi env send --apply
```

`--apply` restarts the Compose stack with the new `.env`. Without `--apply`, the
values are only saved and will be applied by the next `pi deploy`.

List stored keys without values:

```bash
pi env ls
```

## Deploy

From the root of the deployable project:

```bash
pi deploy
```

Deploy a specific branch, tag, or commit SHA:

```bash
pi deploy --ref <git-ref>
```

List projects:

```bash
pi ls
```

Cancel active deployments for the current project:

```bash
pi deploy --cancel
```

Prune Docker images and build cache on the Pi:

```bash
pi gc
```

`pi deploy` reads `./pi.toml`, asks the agent to clone or fetch the configured
repository, builds the Compose stack, starts containers, runs the health check,
and prints the final status.

## Private Git Repositories

The Raspberry Pi must have access to `source.repo`.

For a private repository, the first fetch may fail and print a public deploy
key. Add that key to the repository as a read-only deploy key and retry:

```bash
pi deploy
```

For GitHub, this is located at:

```text
Repository -> Settings -> Deploy keys -> Add deploy key
```

Write access is usually not needed.

## Cloudflare Tunnel

Cloudflare Tunnel is only needed when the service must be reachable from the
internet. Services that only use outbound connections usually do not need public
ingress.

A typical locally managed `cloudflared` config:

```yaml
tunnel: <tunnel-id-or-name>
credentials-file: /var/lib/pi/cloudflared/<tunnel-id>.json

ingress:
  - hostname: app.example.com
    service: http://127.0.0.1:8000
  - service: http_status:404
```

The final `http_status:404` rule must remain the catch-all.

### Manual Ingress

If `/etc/pi/agent.toml` has no `[cloudflared]` section, deployment does not
fail. The agent logs the address that must be routed manually:

```text
hostname -> http://127.0.0.1:<host-port>
```

You can see `host-port` with:

```bash
pi ls
```

### Automatic Ingress

To let the agent edit a local `cloudflared` config, add this to
`/etc/pi/agent.toml`:

```toml
[cloudflared]
config = "/var/lib/pi/cloudflared/config.yml"
tunnel = "home"
restart = ["systemctl", "--user", "restart", "cloudflared"]
```

`pi-agent` must be allowed to:

- read and write the configured `config.yml`;
- run `cloudflared tunnel route dns`;
- run the `restart` command without an interactive password prompt.

If `cloudflared` runs as a system-wide service, it is usually simpler to keep
ingress manual or configure the smallest necessary restart permission
separately.

## Development

Run tests:

```bash
cargo test --workspace
```

Run a local TCP agent:

```bash
cargo run -p pi -- agent run --config dev/agent.toml
```

Point the CLI to the local dev agent.

PowerShell:

```powershell
$env:PI_AGENT_URL = "http://127.0.0.1:7700"
```

Unix shell:

```bash
export PI_AGENT_URL="http://127.0.0.1:7700"
```

## Troubleshooting

### `pi ls` Does Not Connect

Check SSH from the developer machine:

```bash
ssh -i ~/.ssh/id_ed25519_pi pi-user@pi-host.local true
```

Check the agent on the Pi:

```bash
systemctl status pi-agent
journalctl -u pi-agent -n 100 --no-pager
ls -l /run/pi/agent.sock
groups "$USER"
```

After adding the login user to the `pi-agent` group, open a new SSH session.

On the Pi itself, you can check the agent directly:

```bash
curl --unix-socket /run/pi/agent.sock http://localhost/v1/version
curl --unix-socket /run/pi/agent.sock http://localhost/v1/projects
```

### Clone Fails With `Permission denied (publickey)`

The Pi cannot authenticate to `source.repo`. Add the deploy key printed by the
agent, or configure another SSH key with read access to the repository.

After fixing access:

```bash
pi deploy
```

### Docker Build Fails With `/home/pi-agent` Errors

Check that the `systemd` unit contains:

```ini
Environment=HOME=/var/lib/pi
Environment=XDG_CONFIG_HOME=/var/lib/pi/.config
Environment=XDG_CACHE_HOME=/var/lib/pi/.cache
WorkingDirectory=/var/lib/pi
```

Then run:

```bash
sudo systemctl daemon-reload
sudo systemctl restart pi-agent
```

### Docker Permission Denied

Check groups:

```bash
groups pi-agent
getent group docker
```

Add the service user to the Docker group and restart the agent:

```bash
sudo usermod -aG docker pi-agent
sudo systemctl restart pi-agent
```

### Compose Does Not See `.env`

Send the env bundle before deploying:

```bash
pi env send
pi deploy
```

For an already running project:

```bash
pi env send --apply
```

### Health Check Fails

Check that:

- the application listens on `0.0.0.0` inside the container, not only on
  loopback;
- `[ingress].port` matches the container port;
- `[healthcheck].path` exists;
- `[healthcheck].expect` matches the HTTP status code.

On the Pi:

```bash
docker compose -p <project-name> ps
curl http://127.0.0.1:<host-port>/health
```

### Host Port Is In Use

The agent selects a port from `port_min..port_max` and stores it in SQLite. If
the Compose file also defines a fixed host `ports:` mapping, it can conflict
with the agent allocation. Remove the fixed host port and use `expose`.

### CLI And Agent Versions Differ

The CLI prints a warning if the local CLI version and the remote agent version
differ. Rebuild and reinstall both binaries from the same repository revision.

On the Pi:

```bash
cargo build --release
sudo install -m 755 target/release/pi /usr/local/bin/pi
sudo systemctl restart pi-agent
```

On the developer machine:

```bash
cargo install --path crates/bin --locked
```

## Pre-Deploy Checklist

1. On the Pi, `systemctl status pi-agent` shows `active (running)`.
2. From the developer machine, `ssh pi-user@pi-host.local true` works without a
   password.
3. From the developer machine, `pi ls` responds.
4. The deployable project contains `pi.toml`.
5. `[source].repo` is reachable from the Pi.
6. `[source].branch` is the intended default branch.
7. `[build].compose` points to an existing Compose file.
8. `[ingress].service` matches the service name in Compose.
9. `[ingress].port` matches the container port.
10. Compose does not define a conflicting fixed host port.
11. Mutable runtime files are stored in mounted directories.
12. If the project needs secrets, `pi env send` has been run.
13. `pi deploy` finishes with `deploy finished: success`.
14. `pi ls` shows the project, branch, host port, hostname if configured, and
    service status.
