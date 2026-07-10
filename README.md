# rpi-deploy

`rpi` (package `rpi-deploy`) is a deployment tool for Docker Compose projects
on Raspberry Pi. The Pi runs an agent, while the CLI runs on a developer
machine or in CI. The CLI connects to the agent through an SSH tunnel; the
agent clones the Git repository, builds the Compose stack, and starts the
containers.

Website: https://rpi.iiskelo.com

Status: v0.16 (raspberry triangle brand banner + deploy result stamp) —
everything from v0.1–v0.6 (deploy/secrets/
ingress/CI, `rpi logs`, `rpi stats`, `rpi start|stop|restart`, `rpi rm`,
`rpi status`, `rpi doctor`, `rpi agent status|logs`, one-command setup,
`npm install -g rpi-deploy` for both roles), v0.7 prebuilt binaries
(GitHub Actions builds `rpi` for Windows x64, Linux x64, and Linux aarch64 on
every release tag, and `npm install` downloads the matching binary in seconds
instead of compiling for ~10 minutes; building from source remains the
fallback everywhere else, see "Build And Install The Binary" below), `[commands]`
/ `rpi command` for one-off admin commands in the service container,
`[secrets].files` for delivering arbitrary secret files (certs, keys)
alongside the env file, v0.10 per-command service override (`[commands]`
entries can target a different Compose service than the project default),
and v0.11 colorful console output (colored semantic messages, tables for
`rpi ls`/`status`/`stats`, spinners, and a bordered scrolling log pane for
`rpi deploy`/`rpi command` streaming output, with v0.11.1 fixing a log pane
flicker and preserving streamed color), and v0.12 semantic console colours
(a shared palette drives message markers, cyan table headers with
status/usage-colored cells, the spinner glyph, and the log-pane frame — grey
while streaming, red with a full-log dump on failure — and `rpi command` now
keeps its streamed output on screen after a successful run instead of
clearing it), and v0.13 Cloudflare Tunnel auto-bootstrap (`rpi agent setup`
with a Cloudflare API token and a domain installs `cloudflared`, creates or
adopts the tunnel and DNS record entirely through the Cloudflare API — no
`cloudflared tunnel login`, no `cert.pem` — and writes a validated
`config.yml`; the agent degrades to disabled ingress instead of failing to
start when the token is missing or misconfigured) plus a unified
`rpi agent migrate` framework (detects pending internal migrations, tracks
applied ones in a ledger in `state.db`; the existing `pi`-to-`rpi` rename now
runs through it), and v0.14 a themed console layer (a single theme object now
drives every colour and marker glyph in `rpi`'s output — messages, tables,
the spinner, and the log pane; the default `raspberry` theme brands every
message line with a raspberry `▸` marker and the site's green/amber palette,
switchable to the pre-brand `classic` look via `PI_THEME=classic`), and v0.15
adoption of hand-built cloudflared tunnels plus loud manual-ingress signals
(`rpi agent setup` can now adopt a tunnel and `config.yml` that were created
outside `rpi` without rewriting them or causing downtime; deploys and
`rpi doctor` now warn loudly when a project declares a public hostname but
ingress is disabled, so a manually-managed route is never silently
mismatched; and the agent sets `XDG_RUNTIME_DIR` before restarting
`cloudflared` so the restart no longer fails on systems that need it), and
v0.16 a raspberry triangle brand banner (a pink-to-raspberry gradient logo on
`rpi deploy`, bare `rpi`, and `rpi --version`, gated to interactive terminals
and degrading cleanly under `NO_COLOR`/non-unicode/piped output; `rpi deploy`
now ends with a result stamp showing status, project, ingress URL, and
elapsed time; and table cells render the exact brand colour on
`COLORTERM=truecolor`/`24bit` terminals instead of the nearest 256-colour
approximation), and v0.17 a deploy pipeline view (`rpi deploy` now renders
each stage (`fetch → build → start → health → route → gc`) as a collapsing
timed pane with a `✓ build (48.3s)` summary per stage and a service count in
the final stamp; older CLI/agent combinations keep the previous single-pane
view).

First deploy of a private SSH repo no longer fails on a missing deploy key:
`rpi deploy` now preflights repo access before starting the pipeline. If the
agent can't read the repo it registers a read-only deploy key through your
local `gh` automatically (the token never leaves your machine; the private
key never leaves the Pi), or — without `gh` — prints the public key with
instructions and continues by itself once you add it (polls every 5 s for up
to 10 min). `--no-gh-key` skips the GitHub API path. Old agents skip the
preflight; the fetch stage still prints the key hint there.

Supported features:

- `rpi deploy`;
- `rpi deploy --cancel`;
- `rpi ls`;
- `rpi secrets send`;
- `rpi secrets ls`;
- `rpi gc`;
- `rpi command` (run `[commands]` entries in the service container);
- `rpi logs <project> [-f]`;
- `rpi stats [project]`;
- `rpi start|stop|restart <project>`;
- `rpi rm <project> [--volumes]`;
- `rpi status`;
- `rpi doctor`;
- `rpi agent status`;
- `rpi agent logs [-f] [--since 2h]`;
- `rpi setup`;
- `rpi init`;
- `rpi agent setup`;
- `rpi agent uninstall`;
- `rpi agent migrate [--list] [--dry-run] [--run <id>] [--all --yes]`;
- stable host port allocation;
- Docker Compose overrides;
- health checks;
- a latest-wins deployment queue;
- encrypted secrets bundles (environment and files);
- optional Cloudflare Tunnel ingress automation.

## Runtime Model

`rpi` has two parts:

- `rpi agent run` is a daemon on the Raspberry Pi, usually managed by `systemd`.
  It stores state in SQLite, selects a stable host port, writes a Compose
  override, runs `docker compose build`, and then runs
  `docker compose up -d --remove-orphans`.
- `rpi deploy`, `rpi ls`, `rpi secrets ...`, and `rpi gc` are client commands that run
  on a developer machine or CI runner. They open an SSH tunnel to the agent's
  Unix socket.

Each deployable project must contain a `rpi.toml` file. Run `rpi deploy` from the
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
sudo apt install -y git curl build-essential pkg-config
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

## Install Via npm

Client and agent are the same package; the role comes from what you run after
installing. Node.js >= 18 and npm are required (on Raspberry Pi OS:
`sudo apt-get install -y nodejs npm`).

Developer machine (Linux/macOS/Windows):

```bash
npm install -g rpi-deploy
rpi setup
rpi init
```

Raspberry Pi (agent). Docker must already be installed — the install itself
builds without it, but `rpi agent setup` requires it
(`curl -fsSL https://get.docker.com | sh`):

```bash
sudo npm install -g rpi-deploy    # downloads a prebuilt arm64 binary, seconds
sudo rpi agent setup              # installs /usr/local/bin/rpi, unit, start
rpi doctor
```

If Node.js comes from `nvm` (or another per-user version manager) rather than
apt, `sudo` will not find it — `sudo: npm: command not found` / `sudo: rpi:
command not found`, since `sudo` resets `PATH` before running. Install
without `sudo` instead (the nvm prefix is already user-writable), and forward
`PATH` only for the one command that needs root:

```bash
npm install -g rpi-deploy                  # no sudo: nvm's prefix is user-owned
sudo env "PATH=$PATH" rpi agent setup      # sudo alone can't find rpi/node
rpi doctor
```

`agent setup` self-installs a native binary to `/usr/local/bin/rpi`, already
on root's default `PATH`; every `sudo rpi ...` call after this one works
without the `env` wrapper.

Recent npm versions (v11+ observed) also block unreviewed `postinstall`
scripts by default, printing `npm warn allow-scripts ... not yet covered by
allowScripts` instead of failing — if `rpi` then reports the binary was never
built, reinstall with the script allowed:

```bash
npm install -g --allow-scripts=rpi-deploy rpi-deploy   # add sudo for a system-wide (apt) npm
```

`npm approve-scripts <pkg>` does not work for a global install (`EGLOBAL` — no
project `package.json` to write to); `--allow-scripts` on the install command,
or `npm config set allow-scripts=rpi-deploy --location=user` to persist it, is
the supported path for `-g`.

Update (both roles):

```bash
npm install -g rpi-deploy@latest  # with sudo on the Pi, unless npm is nvm-managed
sudo rpi agent setup              # Pi only: swaps the binary and restarts the agent
```

Upgrading from v0.5? The command was renamed `pi` → `rpi` and the project
config `pi.toml` → `rpi.toml` (hard cutover, no fallback). Follow the
step-by-step guide for both roles in
[docs/migration-v0.5-to-v0.6.md](docs/migration-v0.5-to-v0.6.md).

On install the package downloads a prebuilt binary from the matching GitHub
Release (Windows x64, Linux x64, Linux aarch64) and verifies its SHA-256
checksum. On other platforms (macOS, 32-bit ARM), or when the download fails
(offline, proxy, checksum mismatch), it falls back to building the bundled
Rust sources (`cargo build --release --locked`); rustup is installed
automatically when cargo is missing, and the build directory is removed
afterwards to save disk space. Building on Windows needs the Visual Studio
Build Tools C++ workload. Set `RPI_DEPLOY_BUILD_FROM_SOURCE=1` to skip the
download and force the source build. Installing with `--ignore-scripts`
leaves the CLI unusable (`rpi` will report that the binary was not built) —
as does npm's `allow-scripts` gate on recent versions, see above.

## Build And Install The Binary

### Option A: Build On The Pi

This is the simplest option, but it can be slow on smaller boards.

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

git clone <this-repository-url> pi
cd pi
cargo build --release
sudo install -m 755 target/release/rpi /usr/local/bin/rpi
```

Check it:

```bash
rpi --help
```

### Option B: Cross-Build On The Developer Machine

Run these commands from the root of this repository.

```bash
cargo install cross
cross build --release --target aarch64-unknown-linux-gnu
scp target/aarch64-unknown-linux-gnu/release/rpi pi-user@pi-host.local:/tmp/rpi
```

On the Pi:

```bash
sudo install -m 755 /tmp/rpi /usr/local/bin/rpi
rpi --help
```

## Quick Setup

On the Pi, after the binary is installed (see above):

```bash
sudo rpi agent setup
```

This is idempotent: it creates the `rpi-agent` user, directories, the systemd
unit, and `/etc/rpi/agent.toml` if missing, repairs `/var/log/rpi`, and never
touches `secret.key` or `state.db`. On a Pi still running the old `pi-agent`
identity (v0.5 and earlier v0.6 pre-releases), it converts it to `rpi-agent`
in place first — renaming the Linux user/group and moving `/var/lib/pi`,
`/etc/pi`, `/var/log/pi` to their `rpi`-named equivalents (all owned files
move with their directories, nothing is deleted) — before running the steps
above. Re-running it is safe. Use `--dry-run` to
preview, `--with-cloudflared` to scaffold cloudflared.

On the developer machine:

```bash
rpi setup            # wizard: server profile + SSH key + config.toml
rpi init             # wizard: generate rpi.toml in the current project
```

## Install `rpi-agent`

Create the service user:

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin rpi-agent || true
sudo usermod -aG docker rpi-agent
sudo usermod -aG rpi-agent "$USER"
```

`rpi-agent` runs Docker. The login user is added to the `rpi-agent` group so the
SSH tunnel can connect to the agent's Unix socket.

Create directories:

```bash
sudo mkdir -p /var/lib/rpi /etc/rpi
sudo chown -R rpi-agent:rpi-agent /var/lib/rpi
```

Create `/etc/rpi/agent.toml`:

```bash
sudo tee /etc/rpi/agent.toml >/dev/null <<'EOF'
data_dir = "/var/lib/rpi"
socket = "/run/rpi/agent.sock"
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
sudo tee /etc/systemd/system/rpi-agent.service >/dev/null <<'EOF'
[Unit]
Description=pi deploy agent
After=network-online.target docker.service
Wants=network-online.target

[Service]
User=rpi-agent
Group=rpi-agent
ExecStart=/usr/local/bin/rpi agent run --config /etc/rpi/agent.toml
RuntimeDirectory=pi
RuntimeDirectoryMode=0750
Restart=on-failure
Environment=HOME=/var/lib/rpi
Environment=XDG_CONFIG_HOME=/var/lib/rpi/.config
Environment=XDG_CACHE_HOME=/var/lib/rpi/.cache
WorkingDirectory=/var/lib/rpi

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable --now rpi-agent
systemctl status rpi-agent
```

Agent logs:

```bash
journalctl -u rpi-agent -f
```

After adding the login user to the `rpi-agent` group, open a new SSH session.

`rpi-agent` is intentionally created without a home directory. Agent state lives
in `/var/lib/rpi`, and the `HOME` and `XDG_*` variables prevent Docker/BuildKit
from trying to write to a missing `/home/rpi-agent`.

## Install The CLI On A Developer Machine

Run these commands from the root of this repository.

PowerShell:

```powershell
cargo install --path .\crates\bin --locked
rpi --help
```

If `rpi` is not found in the current PowerShell session:

```powershell
$env:PATH += ";$env:USERPROFILE\.cargo\bin"
```

Unix shell:

```bash
cargo install --path crates/bin --locked
rpi --help
```

If `rpi` is not found:

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
rpi ls
```

If no projects have been deployed yet, the expected output is:

```text
▸ no projects deployed yet
```

For CI or a one-off command, you can skip the config file:

```bash
rpi ls --host pi-host.local --user pi-user --key ~/.ssh/id_ed25519_pi
rpi deploy --host pi-host.local --user pi-user --key ~/.ssh/id_ed25519_pi
```

Select a specific profile:

```bash
rpi ls --server home
PI_SERVER=home rpi ls
```

### Console theme

`rpi` output uses the brand theme: a raspberry `▸` marker on every message
line, site green/amber for success/warn. Set `PI_THEME=classic` for the
pre-brand look (cyan accent, `●` marker). `NO_COLOR`, piping, and non-TTY
output disable styling entirely, as before.

`rpi deploy`, bare `rpi`, and `rpi --version` show a raspberry triangle logo
with a vertical gradient. The banner appears only on an interactive terminal —
piped or CI output stays plain, and `rpi --version | cat` prints just
`rpi <version>`. On truecolor terminals (`COLORTERM=truecolor`) the logo and
table colours render as the exact brand `#C51A4A`; elsewhere they use the
nearest 256-colour. The logo is always raspberry, independent of `PI_THEME`.

## Prepare A Project For Deployment

Add `rpi.toml` to the root of the project you want to deploy.

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

[secrets]
env = ".env"                     # optional, default ".env"
files = [                        # optional; recreated at the same paths on the Pi
  "certs/server.pem",
]
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

[secrets]
env = ".env"                     # optional, default ".env"
files = [                        # optional; recreated at the same paths on the Pi
  "certs/server.pem",
]
```

Fields:

- `project.name` is the Compose project name and the state key used by the
  agent.
- `source.repo` is the Git URL cloned by the Pi.
- `source.branch` is the default ref used by `rpi deploy`.
- `build.compose` is the Compose file inside the project repository.
- `ingress.service` is the service name in Compose.
- `ingress.port` is the port inside the container.
- `ingress.hostname` is optional; use it for public ingress.
- `healthcheck.path` is the HTTP endpoint checked through the allocated host
  port. If the path is not set, the agent uses a TCP probe.
- `secrets.env` is the optional local file read by `rpi secrets send` (default `.env`).
- `secrets.files` are optional secret files (certs, keys) recreated at the same paths
  on the Pi on every deploy. Secret files are sent encrypted, stored age-encrypted
  on the agent, and written 0600 into the checkout; paths are relative, use forward
  slashes, and `..` is rejected.

Optional per-project timeouts:

```toml
[timeouts]
fetch = "3m"
build = "45m"
up = "2m"
```

### `[commands]` — admin commands (optional)

One-off admin commands runnable inside the service container with `rpi command`:

```toml
[commands]
create-invite = "node scripts/create-invite.js"
migrate = ["npx", "prisma", "migrate", "deploy"]
backup = "sh -c 'pg_dump mydb | gzip > /data/backup.gz'"
```

- Value: string (split with shell-word rules — quotes work, no variables/pipes/redirects) or an explicit argv array. Need a shell? Spell it out: `sh -c '...'`.
- Names must match `[a-z0-9][a-z0-9_-]*`.
- Commands are registered on the agent **at deploy time** and run in the `ingress.service` container by default via `docker compose exec -T`. To run a command in a different compose service, use the table form:

```toml
[commands.create-invite]
run     = "node dist/scripts/create-invite.cjs"   # string or array, same rules as the shorthand
service = "server"                                 # optional; omitted => ingress service
```

`rpi command` (list mode, no name given) shows the target service for commands that pin one.

- The agent only executes deployed commands — there is no generic remote exec.
- Timeout: 10 minutes by default; override with `command = "30m"` in `[timeouts]`.

## Docker Compose Requirements

`rpi-agent` writes an override file roughly like this:

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

Avoid this for a service managed by `rpi`:

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

## Secrets (Environment And Files)

If `rpi.toml` contains:

```toml
[secrets]
env = ".env"                     # optional, default ".env"
files = [                        # optional; recreated at the same paths on the Pi
  "certs/server.pem",
]
```

send secrets (env bundle + files) from the root of the deployable project:

```bash
rpi secrets send
```

The CLI reads the local `.env` and files, sends them to the agent, and the agent
stores an encrypted bundle in `/var/lib/rpi/secrets`. During `rpi deploy`, the
agent writes them into the project workdir before running Docker Compose.

Before the first deploy of a project that needs secrets:

```bash
rpi secrets send
rpi deploy
```

After changing secrets for an already running project:

```bash
rpi secrets send --apply
```

`--apply` restarts the Compose stack with the new secrets. Without `--apply`, the
values are only saved and will be applied by the next `rpi deploy`.

List stored env keys and secret file paths (values are never transmitted):

```bash
rpi secrets ls
```

## Deploy

From the root of the deployable project:

```bash
rpi deploy
```

Deploy a specific branch, tag, or commit SHA:

```bash
rpi deploy --ref <git-ref>
```

List projects:

```bash
rpi ls
```

Cancel active deployments for the current project:

```bash
rpi deploy --cancel
```

Prune Docker images and build cache on the Pi:

```bash
rpi gc
```

Run an admin command declared in `[commands]`:

```bash
rpi command                                   # list commands deployed on the agent
rpi command create-invite                     # run a command in the service container
rpi command create-invite -- --email x@y.com  # extra args are appended to the declared argv
```

The remote exit code becomes the `rpi` exit code. Ctrl+C detaches and best-effort
kills the run on the agent (the in-container process may survive — standard
`docker exec` behavior). A concurrent deploy is not blocked by a running command;
if it restarts the container mid-run, the command fails.

`rpi deploy` reads `./rpi.toml`, asks the agent to clone or fetch the configured
repository, builds the Compose stack, starts containers, runs the health check,
and prints the final status.

## Private Git Repositories

The Raspberry Pi must have access to `source.repo`.

For a private repository, the first fetch may fail and print a public deploy
key. Add that key to the repository as a read-only deploy key and retry:

```bash
rpi deploy
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

### Automatic Setup (recommended)

With a Cloudflare API token and a domain, the whole tunnel is bootstrapped by
one command:

```bash
sudo rpi agent setup --with-cloudflared --cf-token <token> --domain <zone>
```

This is fully automatic: it installs the `cloudflared` binary for the host's
CPU architecture, creates (or adopts) the tunnel through the Cloudflare API,
writes the credentials JSON and a validated `config.yml`, writes the
`[cloudflare]`/`[cloudflared]` sections into `/etc/rpi/agent.toml`, and enables
the `cloudflared` systemd `--user` service.

No interactive `cloudflared tunnel login` and no `cert.pem` are needed — the
running tunnel only uses the credentials JSON this command writes. Tunnel
management and DNS records both go through the Cloudflare REST API using the
token, not the `cloudflared` CLI.

- `--cf-token` can be omitted if the `CLOUDFLARE_API_TOKEN` environment
  variable is set.
- `--tunnel <name>` overrides the tunnel name. By default it is derived from
  the machine's hostname, falling back to `rpi` if that can't be read.
- The Cloudflare account id is derived automatically from the token — there
  is no account flag.
- The token needs these scopes: `Zone:DNS:Edit`, `Zone:Zone:Read`,
  `Account:Cloudflare Tunnel:Edit`.
- The token is stored at `/var/lib/rpi/cloudflare/token`, owned
  `root:rpi-secrets` with mode `0640`. `rpi-agent` reads it through the
  `rpi-secrets` group.

Omitting `--cf-token`/`--domain` is backward compatible: `--with-cloudflared`
falls back to today's behavior — it scaffolds linger and the user unit and
prints the manual completion steps below.

### Adopting an existing tunnel

If `/var/lib/rpi/cloudflared/config.yml` already exists (a hand-built tunnel),
`sudo rpi agent setup --with-cloudflared --cf-token <token> --domain <zone>` adopts it
instead of recreating anything:

- the existing `config.yml` is **never rewritten** and cloudflared is **not
  restarted** — hand-written routes and uptime are preserved;
- the tunnel id is taken from the `tunnel:` key (a name is resolved via the
  Cloudflare API); credentials are checked at the `credentials-file:` path;
- `[cloudflare]`/`[cloudflared]` are appended to `/etc/rpi/agent.toml`, after
  which every deploy manages routes and DNS by itself.

Token scopes: `Zone:Zone:Read` + `Zone:DNS:Edit` are enough when `tunnel:` holds
the tunnel id (UUID). `Account:Cloudflare Tunnel:Edit` is additionally needed for
fresh installs and for adoption when `tunnel:` holds a name.

If a project declares `[ingress] hostname` while the agent has no ingress
configured, the deploy summary and `rpi doctor` now say so explicitly.

### Manual Setup

Without a token, configure `cloudflared` locally instead. A typical locally
managed `cloudflared` config:

```yaml
tunnel: <tunnel-id-or-name>
credentials-file: /var/lib/rpi/cloudflared/<tunnel-id>.json

ingress:
  - hostname: app.example.com
    service: http://127.0.0.1:8000
  - service: http_status:404
```

The final `http_status:404` rule must remain the catch-all.

### Manual Ingress

If `/etc/rpi/agent.toml` has no `[cloudflared]` section, deployment does not
fail. The agent logs the address that must be routed manually:

```text
hostname -> http://127.0.0.1:<host-port>
```

You can see `host-port` with:

```bash
rpi ls
```

### Automatic Ingress

To let the agent edit a local `cloudflared` config and manage DNS records for
you, add both a `[cloudflared]` and a `[cloudflare]` section to
`/etc/rpi/agent.toml`:

```toml
[cloudflared]
config = "/var/lib/rpi/cloudflared/config.yml"
tunnel = "home"
restart = ["systemctl", "--user", "restart", "cloudflared"]

[cloudflare]
zone = "example.com"
token_file = "/var/lib/rpi/cloudflare/token"
```

`rpi-agent` must be allowed to:

- read and write the configured `config.yml`;
- run the `restart` command without an interactive password prompt;
- read `token_file` (a Cloudflare API token scoped to DNS edit + tunnel read
  on the zone).

DNS records (proxied CNAMEs to `<tunnel-id>.cfargotunnel.com`) are created
through the Cloudflare API using the stored token — `rpi-agent` does not need
permission to run `cloudflared tunnel route dns`.

Both sections are required for automatic DNS: `[cloudflared]` alone is not
enough. If `[cloudflare]` is missing, or its `token_file` cannot be read,
ingress falls back to manual — deploys still succeed, but the agent logs a
warning and you must route hostnames yourself as described above.

If `cloudflared` runs as a system-wide service, it is usually simpler to keep
ingress manual or configure the smallest necessary restart permission
separately.

## Migrations

Host-level upgrades (for example, renaming a legacy install) are handled by
`rpi agent migrate`:

```bash
rpi agent migrate [--list] [--dry-run] [--run <id>] [--all --yes]
```

- Migrations run uniformly and idempotently; each applied migration is
  recorded so it is never re-applied.
- `--list` shows every registered migration and whether it has been applied.
- Non-disruptive migrations run automatically during `rpi agent setup`.
  Disruptive ones are only reported there and must be applied explicitly with
  `--run <id>` (or `--all --yes` to apply every pending migration).
- `--dry-run` shows the plan without changing anything.

The only migration currently registered is `pi-to-rpi`, which renames a
legacy `pi-agent` install to `rpi-agent`.

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

### `npm warn allow-scripts ...` / `rpi: binary not built`

npm blocked the `postinstall` script that builds the Rust binary and skipped
it instead of failing outright. Reinstall with the script explicitly allowed:

```bash
npm install -g --allow-scripts=rpi-deploy rpi-deploy   # add sudo for a system-wide (apt) npm
```

To avoid repeating the flag on every future update:

```bash
npm config set allow-scripts=rpi-deploy --location=user
```

`npm approve-scripts <pkg>` does not work here — it writes to a project
`package.json`, and a global install has none (`EGLOBAL`).

### `sudo: npm: command not found` / `sudo: rpi: command not found`

Node.js is managed by `nvm` (or a similar per-user version manager); `sudo`
resets `PATH` and does not see it. Install without `sudo` — the nvm prefix is
already user-writable — and forward `PATH` only for the one command that
needs root:

```bash
npm install -g rpi-deploy
sudo env "PATH=$PATH" rpi agent setup
```

`agent setup` self-installs a native binary to `/usr/local/bin/rpi`, already
on root's default `PATH`; later `sudo rpi ...` calls work without the `env`
wrapper.

### `rpi ls` Does Not Connect

Check SSH from the developer machine:

```bash
ssh -i ~/.ssh/id_ed25519_pi pi-user@pi-host.local true
```

Check the agent on the Pi:

```bash
systemctl status rpi-agent
journalctl -u rpi-agent -n 100 --no-pager
ls -l /run/rpi/agent.sock
groups "$USER"
```

After adding the login user to the `rpi-agent` group, open a new SSH session.

On the Pi itself, you can check the agent directly:

```bash
curl --unix-socket /run/rpi/agent.sock http://localhost/v1/version
curl --unix-socket /run/rpi/agent.sock http://localhost/v1/projects
```

### Clone Fails With `Permission denied (publickey)`

The Pi cannot authenticate to `source.repo`. Add the deploy key printed by the
agent, or configure another SSH key with read access to the repository.

After fixing access:

```bash
rpi deploy
```

### Docker Build Fails With `/home/rpi-agent` Errors

Check that the `systemd` unit contains:

```ini
Environment=HOME=/var/lib/rpi
Environment=XDG_CONFIG_HOME=/var/lib/rpi/.config
Environment=XDG_CACHE_HOME=/var/lib/rpi/.cache
WorkingDirectory=/var/lib/rpi
```

Then run:

```bash
sudo systemctl daemon-reload
sudo systemctl restart rpi-agent
```

### Docker Permission Denied

Check groups:

```bash
groups rpi-agent
getent group docker
```

Add the service user to the Docker group and restart the agent:

```bash
sudo usermod -aG docker rpi-agent
sudo systemctl restart rpi-agent
```

### Compose Does Not See Secrets

Send the secrets bundle before deploying:

```bash
rpi secrets send
rpi deploy
```

For an already running project:

```bash
rpi secrets send --apply
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
sudo install -m 755 target/release/rpi /usr/local/bin/rpi
sudo systemctl restart rpi-agent
```

On the developer machine:

```bash
cargo install --path crates/bin --locked
```

## Pre-Deploy Checklist

1. On the Pi, `systemctl status rpi-agent` shows `active (running)`.
2. From the developer machine, `ssh pi-user@pi-host.local true` works without a
   password.
3. From the developer machine, `rpi ls` responds.
4. The deployable project contains `rpi.toml`.
5. `[source].repo` is reachable from the Pi.
6. `[source].branch` is the intended default branch.
7. `[build].compose` points to an existing Compose file.
8. `[ingress].service` matches the service name in Compose.
9. `[ingress].port` matches the container port.
10. Compose does not define a conflicting fixed host port.
11. Mutable runtime files are stored in mounted directories.
12. If the project needs secrets, `rpi secrets send` has been run.
13. `rpi deploy` finishes with `deploy finished: success`.
14. `rpi ls` shows the project, branch, host port, hostname if configured,
    expose mode (`-` for private, `lan http://<lan-ip>:<port>` for
    `expose = "lan"`), and service status.
