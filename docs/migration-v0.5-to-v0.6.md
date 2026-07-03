# Migrating From v0.5 To v0.6

v0.6 renames the tool and changes how it is installed. This guide covers both
runtime roles:

- the **agent** — the daemon on the Raspberry Pi (`rpi agent run`, the
  `rpi-agent` systemd service);
- the **client** — the CLI on a developer machine or CI runner (`rpi deploy`,
  `rpi ls`, `rpi env ...`, `rpi gc`).

Do both sides. A v0.6 client cannot read a `pi.toml`, and a v0.5 client warns
when it talks to a v0.6 agent (and vice versa), so upgrade the agent and every
client together.

## At A Glance

| Changed (breaking) | Old | New |
| --- | --- | --- |
| CLI command / binary | `pi` | `rpi` |
| Installed binary path (agent) | `/usr/local/bin/pi` | `/usr/local/bin/rpi` |
| Installed binary path (client, from source) | `~/.cargo/bin/pi` | `~/.cargo/bin/rpi` |
| Project config file | `pi.toml` | `rpi.toml` (hard cutover, **no fallback** — rename each project's config yourself) |
| Systemd service user/group/unit | `pi-agent`, `pi-agent.service` | `rpi-agent`, `rpi-agent.service` (**`rpi agent setup` migrates this automatically** — see below) |
| Agent config dir | `/etc/pi` | `/etc/rpi` (auto-migrated) |
| Agent state dir | `/var/lib/pi` | `/var/lib/rpi` (auto-migrated, `secret.key`/`state.db` move with it) |
| Agent socket/log dirs | `/run/pi`, `/var/log/pi` | `/run/rpi`, `/var/log/rpi` (auto-migrated) |
| Install method | build from source | `npm install -g rpi-deploy` (builds from source on install) |

Two different migration styles, by design:

- **The agent-side rename (user/group/unit/paths) is automatic.** Run
  `sudo rpi agent setup` and it detects a `pi-agent` install and converts it
  to `rpi-agent` in place — see "Migrate The Agent" below for exactly what it
  does.
- **The project config rename (`pi.toml` → `rpi.toml`) is manual, on
  purpose.** It lives in your project repositories, not on the Pi, so there is
  nothing for `rpi agent setup` to find and convert automatically. Rename it
  yourself — see "Migrate The Client" below.

Unchanged either way:

| Unchanged |
| --- | --- |
| Client config dir: `%APPDATA%\pi\config.toml` (Windows), `~/.config/pi/config.toml` (macOS/Linux) |
| Environment variables: `PI_SERVER`, `PI_AGENT_URL` |

## Prerequisites

- Node.js >= 18 and npm on both the Pi and the developer machine
  (Raspberry Pi OS: `sudo apt-get install -y nodejs npm`).
- Docker already installed on the Pi (unchanged from v0.5).
- On the Pi, building from source during `npm install` takes roughly 10 minutes.

## Migrate The Agent (Raspberry Pi)

`rpi agent setup` does the entire agent-side migration in one command — there
is no separate "migrate" step.

1. Install the v0.6 package (builds `rpi` from source):

   ```bash
   sudo npm install -g rpi-deploy@latest
   ```

   If you were on a from-source v0.5 install, this is the same command — npm
   replaces the manual `cargo build` / `install` step.

2. Run setup:

   ```bash
   sudo rpi agent setup
   ```

   On a Pi still running the old `pi-agent` identity, this first converts it
   to `rpi-agent` in place, then runs its normal idempotent bootstrap:

   - stops the old `pi-agent.service` (best-effort — it may already be
     stopped);
   - `groupmod -n rpi-agent pi-agent`, then `usermod -l rpi-agent pi-agent`
     — uid/gid are unchanged, so every file already owned by that id is
     "renamed" for free, no `chown` needed;
   - moves `/var/lib/pi` → `/var/lib/rpi`, `/etc/pi` → `/etc/rpi`,
     `/var/log/pi` → `/var/log/rpi` (each only if the old path exists and the
     new one doesn't — `secret.key`, `state.db`, and any cloudflared config
     move with their directory, nothing is deleted);
   - backs up the old unit file to
     `/etc/systemd/system/pi-agent.service.bak` (never deleted) and writes
     the new `rpi-agent.service`;
   - re-enables `loginctl` linger under the new login name if cloudflared was
     previously configured.

   Then it installs the running binary to `/usr/local/bin/rpi` and restarts
   the agent, same as a fresh install. Use `--dry-run` first if you want to
   preview the changes — in dry-run mode the migration only reports what it
   *would* do and makes no changes.

3. Remove the stale v0.5 binary — `agent setup` installs `rpi` but leaves the
   old `pi` in place:

   ```bash
   sudo rm -f /usr/local/bin/pi
   ```

4. Verify:

   ```bash
   rpi doctor
   rpi agent status
   systemctl status rpi-agent
   ```

   `systemctl status rpi-agent` should show `active (running)`, and its
   `ExecStart` should now read `/usr/local/bin/rpi agent run --config
   /etc/rpi/agent.toml`.

Updating an already-migrated agent later is the same two commands, and the
migration step is a no-op once `rpi-agent` already exists:

```bash
sudo npm install -g rpi-deploy@latest
sudo rpi agent setup   # swaps the binary and restarts the agent
```

## Migrate The Client (Developer Machine / CI)

The client migration has one manual step v0.5 users must not skip: renaming each
project's `pi.toml` to `rpi.toml`. Unlike the agent-side rename, this cannot be
automated — the config lives in your project repositories, not on a machine
`rpi agent setup` can reach.

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
| `pi-agent` user/group, `pi-agent.service` | `rpi-agent` user/group, `rpi-agent.service` (auto-migrated by `rpi agent setup`) |
| `/etc/pi/agent.toml` | `/etc/rpi/agent.toml` (auto-migrated) |
| `/var/lib/pi` (`secret.key`, `state.db`, secrets, cloudflared config) | `/var/lib/rpi` (auto-migrated, contents unchanged) |
| `/run/pi/agent.sock` | `/run/rpi/agent.sock` (auto-migrated) |
| `/var/log/pi` | `/var/log/rpi` (auto-migrated) |

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

### `systemctl status pi-agent` says "could not be found"

Expected after migration — the unit is now named `rpi-agent.service`. Use
`systemctl status rpi-agent`. If `rpi doctor` / `rpi agent status` also fail,
check that `sudo rpi agent setup` completed without errors (re-run it; it is
idempotent).

### Migration ran but `groupmod`/`usermod` failed

`rpi agent setup` reports this as an error and stops before touching any
directories (it never leaves the install in a half-renamed state). The
`pi-agent` user/group and its directories are untouched at that point — fix
the underlying issue (commonly: the `pi-agent` user has a process still
running under it, or `rpi-agent` already exists as a *different* uid from a
manual partial migration) and re-run `sudo rpi agent setup`.

## Rollback

If you need to return to v0.5:

- **Client:** reinstall the v0.5 build and restore the old config filename
  (`git mv rpi.toml pi.toml`, or check out the pre-rename commit).
- **Agent:** this is more involved than in earlier `pi.toml`-only migrations,
  because the agent-side rename also touches the Linux user/group and three
  directories:
  1. Stop the agent: `sudo systemctl disable --now rpi-agent`.
  2. Reverse the identity rename: `sudo groupmod -n pi-agent rpi-agent` then
     `sudo usermod -l pi-agent rpi-agent` (uid/gid unchanged, so file
     ownership is preserved the same way the forward migration preserved it).
  3. Move the directories back: `sudo mv /var/lib/rpi /var/lib/pi`,
     `sudo mv /etc/rpi /etc/pi`, `sudo mv /var/log/rpi /var/log/pi`.
  4. Restore the old unit: the forward migration backed it up to
     `/etc/systemd/system/pi-agent.service.bak` — restore it with
     `sudo mv /etc/systemd/system/pi-agent.service.bak
     /etc/systemd/system/pi-agent.service` (remove the v0.6
     `rpi-agent.service` first: `sudo rm -f
     /etc/systemd/system/rpi-agent.service`).
  5. Reinstall the v0.5 binary at `/usr/local/bin/pi`, then
     `sudo systemctl daemon-reload && sudo systemctl enable --now pi-agent`.

  `secret.key`, `state.db`, and any cloudflared config are never deleted by
  either direction of this migration, so agent state survives a rollback
  intact as long as you move the directories back before reinstalling v0.5.
