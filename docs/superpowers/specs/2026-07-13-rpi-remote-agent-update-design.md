# Client-triggered agent update (`rpi upgrade` / `rpi agent update`)

Date: 2026-07-13

## Problem

There is no way to update the rpi binary on a board from the client. Today an
operator must SSH into the Pi by hand, obtain a newer binary (`npm i -g
rpi-deploy@…` or a manual copy), and run `sudo rpi agent setup`, which
self-installs the binary to `/usr/local/bin/rpi` and restarts the `rpi-agent`
systemd unit. The CLI even tells users this in prose ("update it on the Pi",
`crates/bin/src/cli/commands.rs:680`). Everything about the update is manual and
undocumented as a single step.

The core constraint is deliberate: the agent runs as the unprivileged
`rpi-agent` user (docker group only, no sudo). It **cannot** write the
root-owned `/usr/local/bin/rpi` nor restart the system unit `rpi-agent.service`.
So a client-driven update cannot be performed by the agent process itself, and
we do not want to grant the agent that privilege.

## Goal

Add a single client command that updates the agent on a board to a chosen
version, securely, reusing the trust boundary that already exists (SSH + sudo +
GitHub Releases over HTTPS with SHA256 verification). As a companion, add a
no-npm one-line installer that shares the same download-and-verify recipe.

Non-goals:

- **No new privileged surface on the agent.** The agent never touches its own
  binary; the privileged swap happens only via `sudo` on the board, over an
  authenticated SSH session — identical to today's manual `sudo rpi agent
  setup`.
- **No client self-update.** `rpi upgrade` updates the board only. Keeping the
  client and board in sync is achieved by defaulting the target to the client's
  own version (see below); the client is updated by its normal channel (npm /
  the new installer). A dedicated `rpi self-update` is explicitly deferred.
- **No signing infrastructure beyond what releases already publish.** Integrity
  rests on the release `SHA256SUMS` file and GitHub's TLS, exactly as the npm
  `postinstall` already does.

## Trust model

The root of trust is unchanged from a first install:

1. **Transport & authentication.** The client already reaches the agent only
   through an SSH tunnel keyed by the operator's SSH key
   (`crates/bin/src/cli/tunnel.rs`). The update reuses that same SSH access to
   run a privileged command on the board. Anyone who can already `rpi deploy`
   can already SSH to the box; the update adds no new access path.
2. **Privilege.** Writing `/usr/local/bin/rpi` and restarting the unit require
   root. The client obtains it via `sudo` on the board over `ssh -t`. This is
   the same privilege the operator already exercises for `sudo rpi agent setup`.
3. **Binary integrity.** The board fetches a release artifact and verifies its
   SHA256 against the release `SHA256SUMS` (GitHub-direct branch), or delegates
   to npm's own integrity (npm branch). Both anchor on GitHub over HTTPS — the
   same anchor `scripts/postinstall.js` uses today.

No new secret, key, or trust anchor is introduced.

## Distribution facts this design relies on

From `scripts/postinstall.js` and `.github/workflows/release.yml`:

- Each release publishes per-target archives named
  `rpi-v<version>-<triple>.<ext>` plus a `SHA256SUMS` file. For the Pi the
  triple is `aarch64-unknown-linux-musl` and the ext is `tar.gz`.
- The Pi binary is a **static musl** build — no glibc/node runtime dependency.
- `postinstall.js` is not magic: it downloads that same archive from GitHub
  Releases, verifies SHA256 against `SHA256SUMS`, and extracts `rpi` into
  `dist/rpi` (source build is only a fallback). The download/verify logic
  (`downloadPrebuilt`, `parseSha256Sums`, `assetName`, target-triple map) is the
  reference to port to Rust.

## Commands

The CLI already follows a namespace convention: `rpi agent <x>` runs **on the
agent host** (`agent run`, `agent setup`), while bare verbs (`deploy`, `ls`,
`stats`, `setup`, …) run on the client and talk to a remote agent. The two new
commands honour that split.

### `rpi agent update [--version <X>]` — board-side, root

Runs on the Pi, next to `agent setup`/`agent run`. Performs the actual work:

1. **Resolve the target version.** If `--version <X>` is given, use it. If
   omitted (someone runs it directly on the board), resolve the latest published
   release (GitHub API `releases/latest`). When invoked by `rpi upgrade` the
   client always passes an explicit `--version`, so the board receives a concrete
   version and the resolution path is deterministic.
2. **Detect the channel.** Reuse `resolve_npm_dist_binary`
   (`crates/bin/src/cli/setup.rs`), which runs `sudo -u <login_user> -i -- npm
   root -g` and checks for `<root>/rpi-deploy/dist/rpi`.
   - **npm branch** (npm install detected): run `npm i -g rpi-deploy@<X>` as the
     login user; the source binary is the refreshed `dist/rpi`.
   - **GitHub-direct branch** (no npm install): download
     `rpi-v<X>-aarch64-unknown-linux-musl.tar.gz` + `SHA256SUMS` from the release
     (triple derived from `uname -m` via a `cloudflared_asset`-style map), verify
     the SHA256, extract to a temp path; the source binary is the extracted
     `rpi`.
3. **Apply via the existing setup path.** Feed the source binary into the same
   flow as `agent setup`: `self_install::ensure_installed(source,
   /usr/local/bin/rpi)` (atomic write + rename), run the idempotent `setup()`,
   and `restart_agent_if_active()` when the binary actually changed. If the board
   is already on the target version, `ensure_installed` reports `UpToDate` and no
   restart happens.

Concretely this is a thin front over the existing `run_cmd`
(`crates/bin/src/cli/setup.rs`): today `run_cmd` self-installs from
`current_exe()`; `agent update` self-installs from a freshly obtained binary
instead, then runs the same setup + restart.

Flags: `--version <X>` (optional), `--dry-run` (mirror `agent setup`; resolve +
report without downloading/swapping).

### `rpi upgrade [--version <X>]` — client-side pult

Runs on the laptop. Triggers the board to update itself:

1. **Resolve the SSH profile** via the existing `ConnectOpts` / `ServerProfile`
   (`crates/bin/src/cli/config.rs`).
2. **Resolve the target version.** Default: the client's own
   `CARGO_PKG_VERSION` — "bring the board up to the version of the rpi I'm
   running", which keeps the client↔agent pair aligned with the compat
   handshake (`crates/bin/src/compat.rs`). `--version <X>` overrides; `--version
   latest` resolves the newest published release via the GitHub API. The client
   resolves to a concrete version and passes it to the board explicitly.
3. **Show and confirm.** Do a quick `/v1/version` read (the same handshake
   `connect_agent` already performs) to show `current → target`, then prompt for
   confirmation. `--yes` skips the prompt.
4. **Trigger over SSH.** Open an SSH **exec** session (not the socket-forward
   tunnel) with a TTY so sudo can prompt if needed:
   `ssh -t <user>@<host> sudo rpi agent update --version <X>`, streaming the
   board's output to the client's terminal.
5. **Verify.** After the session exits, reconnect and read `/v1/version` to
   confirm the new version is live; report success/skew.

Flags: `--version <X|latest>`, `--yes`, plus the standard connection flags the
other client commands accept (server/profile selection).

The SSH-exec path is a new, small sibling to `SshTunnel::open`
(`crates/bin/src/cli/tunnel.rs`): same profile/key handling, but it runs a remote
command with an allocated TTY and inherited stdio instead of a `-N -L` forward.
It honours `PI_AGENT_URL` only insofar as it still needs SSH — for the pure local
dev override (`PI_AGENT_URL` set, no SSH) `rpi upgrade` is not applicable and
errors with a clear message.

## `install.sh` — one-line, no-npm install

A standalone POSIX shell installer, hosted as a stable release/raw URL, run as
`curl -fsSL <url> | sh`. It shares the exact recipe as the GitHub-direct branch
and `postinstall.js`, but in shell because there is no rpi to run yet:

1. Detect arch (`uname -m` → target triple; `aarch64`/`arm64` →
   `aarch64-unknown-linux-musl`, `x86_64`/`amd64` → `x86_64-unknown-linux-musl`).
2. Resolve version: default latest (GitHub API), `RPI_VERSION` env override.
3. Download `rpi-v<X>-<triple>.tar.gz` + `SHA256SUMS`; verify the SHA256
   (`sha256sum`/`shasum`); extract.
4. Install the `rpi` binary to `/usr/local/bin` (override `RPI_INSTALL_DIR`),
   using `sudo` only if the target dir is not writable.
5. Print next steps (`sudo rpi agent setup` on a Pi; `rpi setup` on a dev
   machine). It does **not** run setup itself.

`install.sh` closes the bootstrap gap for binary-only (no-node) installs and is
the natural first step before the very first `rpi agent update` exists on a
board.

## sudo over non-interactive SSH

`ssh -t` allocates a TTY and inherits stdio, so:

- If the board's sudo requires a password, the operator is prompted in their own
  terminal.
- If the login user has `NOPASSWD`, no prompt appears — fully non-interactive
  for free.

No special code is needed for the non-interactive/CI case. For operators who
want it, document a narrow sudoers rule rather than blanket NOPASSWD:

```
<login-user> ALL=(root) NOPASSWD: /usr/local/bin/rpi agent update *
```

The interactive `ssh -t` default is what ships; the sudoers rule is
documentation only.

## Reuse vs new code

Reuse:

- `self_install::ensure_installed`, `restart_agent_if_active`,
  `resolve_npm_dist_binary`, and the `run_cmd` setup flow
  (`crates/bin/src/cli/setup.rs`, `self_install.rs`).
- A `cloudflared_asset`-style arch→triple map (`setup.rs` already has the
  pattern for cloudflared assets).
- `ConnectOpts` / `ServerProfile` and the SSH profile/key handling
  (`crates/bin/src/cli/config.rs`, `tunnel.rs`).
- The `/v1/version` handshake (`connect_agent`, `crates/bin/src/cli/connect.rs`).

New:

- Version resolution + GitHub archive download + SHA256 verification in Rust
  (port of `postinstall.js`'s `downloadPrebuilt`/`parseSha256Sums`/`assetName`).
- `rpi agent update` subcommand wiring (board-side).
- `rpi upgrade` client command + an SSH-exec runner (sibling of `SshTunnel`).
- `install.sh`.

## Bootstrap caveat

`rpi agent update` exists only from the release that introduces it. Boards on
older versions must be updated once by the manual path (or the new
`install.sh` + `sudo rpi agent setup`); every subsequent update is a single
`rpi upgrade`. This one-time gap is normal for self-update features.

## Testing

Unit:

- Version resolution (explicit, default-to-client-version, `latest`).
- SHA256 verification: matching, mismatching, and missing-from-`SHA256SUMS`
  cases (mirror `postinstall.test.js`).
- Channel detection (npm present vs absent) via the existing `FakeSys`.
- arch → triple mapping (including unsupported arch).
- Reuse the existing `self_install` tests for the swap semantics.

End-to-end (default for this repo — features ship with an e2e scenario unless
there is an explicit reason not to): an `agent update` scenario that points the
download at a **local fixture release server** via an env-overridable base URL
(so tests never hit real GitHub), asserting the binary is swapped and the agent
restarts onto the new version. Where feasible, fold into the existing docker
e2e harness (`docs/superpowers/specs/2026-07-10-docker-e2e-deployment-design.md`).

To keep the download testable, the release base URL must be injectable (env var,
e.g. `RPI_RELEASE_BASE_URL`) rather than hardcoded — both for the board-side
`agent update` and for `install.sh`.
