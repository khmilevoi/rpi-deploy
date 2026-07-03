# Migrating From v0.5 To v0.6

v0.6 renames the tool and changes how it is installed. This guide covers both
runtime roles:

- the **agent** — the daemon on the Raspberry Pi (`rpi agent run`, the
  `pi-agent` systemd service);
- the **client** — the CLI on a developer machine or CI runner (`rpi deploy`,
  `rpi ls`, `rpi env ...`, `rpi gc`).

Do both sides. A v0.6 client cannot read a `pi.toml`, and a v0.5 client warns
when it talks to a v0.6 agent (and vice versa), so upgrade the agent and every
client together.

## At A Glance

Three things changed, and they are all breaking:

| Changed (breaking) | Old | New |
| --- | --- | --- |
| CLI command / binary | `pi` | `rpi` |
| Installed binary path (agent) | `/usr/local/bin/pi` | `/usr/local/bin/rpi` |
| Installed binary path (client, from source) | `~/.cargo/bin/pi` | `~/.cargo/bin/rpi` |
| Project config file | `pi.toml` | `rpi.toml` (hard cutover, **no fallback**) |
| Install method | build from source | `npm install -g rpi-deploy` (builds from source on install) |

Everything else keeps its old name, so most of your setup carries over
untouched:

| Unchanged |
| --- |
| Service user and unit: `pi-agent`, `pi-agent.service` |
| Agent config: `/etc/pi/agent.toml` |
| Agent state: `/var/lib/pi` (including `secret.key`, `state.db`) |
| Agent socket and logs: `/run/pi/agent.sock`, `/var/log/pi` |
| Client config dir: `%APPDATA%\pi\config.toml` (Windows), `~/.config/pi/config.toml` (macOS/Linux) |
| Environment variables: `PI_SERVER`, `PI_AGENT_URL` |

In short: the `pi` command becomes `rpi`, and each project's `pi.toml` becomes
`rpi.toml`. Nothing under `/etc/pi` or `/var/lib/pi` is renamed, and your client
profile in `config.toml` still works as-is.

## Prerequisites

- Node.js >= 18 and npm on both the Pi and the developer machine
  (Raspberry Pi OS: `sudo apt-get install -y nodejs npm`).
- Docker already installed on the Pi (unchanged from v0.5).
- On the Pi, building from source during `npm install` takes roughly 10 minutes.

## Migrate The Agent (Raspberry Pi)

There is no config to rename on the agent side. The migration is: install the
new package, let `rpi agent setup` rewrite the systemd unit and swap the binary,
then delete the old binary.

1. Install the v0.6 package (builds `rpi` from source):

   ```bash
   sudo npm install -g rpi-deploy@latest
   ```

   If you were on a from-source v0.5 install, this is the same command — npm
   replaces the manual `cargo build` / `install` step.

2. Rewrite the unit and install the binary:

   ```bash
   sudo rpi agent setup
   ```

   This is idempotent. It rewrites `/etc/systemd/system/pi-agent.service`
   (backing up the previous unit to `pi-agent.service.bak` if it differs),
   installs the running binary to `/usr/local/bin/rpi`, and restarts the agent.
   It never touches `/etc/pi/agent.toml`, `secret.key`, or `state.db`. Use
   `--dry-run` first if you want to preview the changes.

3. Remove the stale v0.5 binary — `agent setup` installs `rpi` but leaves the
   old `pi` in place:

   ```bash
   sudo rm -f /usr/local/bin/pi
   ```

4. Verify:

   ```bash
   rpi doctor
   rpi agent status
   systemctl status pi-agent
   ```

   `systemctl status pi-agent` should show `active (running)`, and its
   `ExecStart` should now read `/usr/local/bin/rpi agent run ...`.

Updating an already-migrated agent later is the same two commands:

```bash
sudo npm install -g rpi-deploy@latest
sudo rpi agent setup   # swaps the binary and restarts the agent
```

## Migrate The Client (Developer Machine / CI)

The client migration has one manual step v0.5 users must not skip: renaming each
project's `pi.toml` to `rpi.toml`.

1. Install the v0.6 package:

   ```bash
   npm install -g rpi-deploy
   ```

   This replaces `cargo install --path crates/bin`. (Installing from source
   still works and now also produces `rpi` — see the README.)

2. Remove the stale v0.5 binary if you installed from source before:

   ```bash
   # Unix
   rm -f ~/.cargo/bin/pi
   ```

   ```powershell
   # Windows
   Remove-Item "$env:USERPROFILE\.cargo\bin\pi.exe" -ErrorAction SilentlyContinue
   ```

3. Your client profile needs no change. `config.toml` still lives in the `pi/`
   config directory, and `PI_SERVER` / `PI_AGENT_URL` are unchanged.

4. Rename `pi.toml` → `rpi.toml` in **every** deployable project. This is a hard
   cutover — v0.6 only reads `rpi.toml` and will not fall back to `pi.toml`.
   Commit the rename.

   ```bash
   # Unix, from a project root
   git mv pi.toml rpi.toml        # or: mv pi.toml rpi.toml
   ```

   ```powershell
   # Windows, from a project root
   git mv pi.toml rpi.toml        # or: Rename-Item pi.toml rpi.toml
   ```

   To rename across many repositories under one directory:

   ```bash
   # Unix
   find . -maxdepth 2 -name pi.toml -execdir mv pi.toml rpi.toml \;
   ```

   Do not run `rpi init` to "convert" an old config — `init` generates a fresh
   `rpi.toml` from a wizard and backs up any existing `rpi.toml` to
   `rpi.toml.bak`; it does not read your old `pi.toml`.

5. Update CI. Change the install step to `npm install -g rpi-deploy` and replace
   every `pi ...` invocation with `rpi ...`. If your workflow checked in or
   referenced `pi.toml`, update those paths to `rpi.toml`.

6. Verify:

   ```bash
   rpi --help
   rpi ls
   ```

   From a renamed project root, `rpi deploy` should read `./rpi.toml` and run as
   before.

## Command And Name Mapping

| v0.5 | v0.6 |
| --- | --- |
| `pi deploy` / `pi ls` / `pi env send` / `pi gc` | `rpi deploy` / `rpi ls` / `rpi env send` / `rpi gc` |
| `pi agent setup` / `pi agent run` | `rpi agent setup` / `rpi agent run` |
| `pi.toml` (project root) | `rpi.toml` (project root) |
| `cargo build --release` + `install ... /usr/local/bin/pi` | `sudo npm install -g rpi-deploy` + `sudo rpi agent setup` |
| `cargo install --path crates/bin` | `npm install -g rpi-deploy` |
| `/usr/local/bin/pi`, `~/.cargo/bin/pi` | `/usr/local/bin/rpi`, `~/.cargo/bin/rpi` |

## Troubleshooting

### `cannot read rpi.toml ... (run from the project root, see §12)`

You are either not in the project root, or the project still has a `pi.toml`.
Rename it: `git mv pi.toml rpi.toml`. There is no automatic fallback to the old
filename.

### Both `pi` and `rpi` respond

The old binary was left behind. Remove it so scripts and muscle memory fail
loudly instead of silently running the stale version:

```bash
sudo rm -f /usr/local/bin/pi      # Pi
rm -f ~/.cargo/bin/pi             # dev machine (from-source installs)
```

### `rpi: binary not built; reinstall without --ignore-scripts`

The npm package builds the Rust binary in its `postinstall` script. Installing
with `--ignore-scripts` skips that build and leaves the `rpi` shim with nothing
to run. Reinstall normally:

```bash
npm install -g rpi-deploy         # add sudo on the Pi
```

### CLI warns that versions differ

The client prints a warning when its version and the agent's version differ.
After migrating, make sure both were installed from the same v0.6 release:

```bash
# Pi
sudo npm install -g rpi-deploy@latest
sudo rpi agent setup

# developer machine
npm install -g rpi-deploy@latest
```

## Rollback

If you need to return to v0.5:

- **Client:** reinstall the v0.5 build and restore the old config filename
  (`git mv rpi.toml pi.toml`, or check out the pre-rename commit).
- **Agent:** reinstall the v0.5 binary at `/usr/local/bin/pi`. If a v0.6
  `rpi agent setup` had rewritten the unit, its previous version is at
  `/etc/systemd/system/pi-agent.service.bak`; restore it and run
  `sudo systemctl daemon-reload && sudo systemctl restart pi-agent`.

Because `/etc/pi/agent.toml` and everything under `/var/lib/pi` (state and
secrets) are never renamed or removed by the migration, agent state survives a
rollback intact.
