# pi v0.6 — npm Install (`rpi`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `npm install -g rpi-deploy` installs the CLI (command `rpi`) for both roles by building the bundled Rust sources on the user's machine; `sudo rpi agent setup` self-installs the binary to `/usr/local/bin/rpi` and restarts the daemon on updates.

**Architecture:** The CLI command is renamed `pi` → `rpi` (binary and messages only — the `pi-agent` user/unit, `/etc/pi`, `/var/lib/pi`, `pi.toml`, and crate names stay). The npm package ships the Rust workspace sources; `scripts/postinstall.js` provisions rustup if needed, runs `cargo build --release --locked`, copies the binary to `dist/`, and deletes `target/`. The global `rpi` command is a Node shim (`bin/rpi.js`) spawning `dist/rpi`. `rpi agent setup` copies its own binary (`current_exe`) to `/usr/local/bin/rpi` and restarts an active `pi-agent` when the binary changed.

**Tech Stack:** plain Node ≥ 18 (CommonJS, zero npm dependencies), cargo/rustup, existing Rust workspace (tokio, async-trait; `tempfile` already a dev-dependency of `crates/bin`).

**Spec:** `docs/superpowers/specs/2026-07-03-pi-npm-install-v0.6-design.md` (authoritative for behavior).

## Global Constraints

- CLI command is `rpi`. Rename boundary (spec §3.2): `[[bin]]` name, clap name, `ExecStart`, canonical path `/usr/local/bin/rpi`, all user-facing strings and docs say `rpi …`. NOT renamed: user/group/unit `pi-agent`, `/etc/pi`, `/var/lib/pi`, `/var/log/pi`, `/run/pi`, `pi.toml`, Rust package names (`pi`, `pi-domain`, …).
- npm package name `rpi-deploy`, version `0.6.0`, command `bin: {"rpi": "bin/rpi.js"}`; `engines.node >= 18`; `os: ["linux", "darwin", "win32"]`. (An unrelated npm package `rpi` exists — we only need the bin name, which is a documented, accepted collision risk.)
- Tarball whitelist (`files`): `bin/`, `scripts/postinstall.js`, `scripts/check-version.js`, `crates/`, `Cargo.toml`, `Cargo.lock` (npm adds `package.json`, `README.md`, `LICENSE` automatically). No `target/`, no `docs/`, no `plugins/`.
- JS files are plain CommonJS for Node ≥ 18: no npm dependencies, no build step, no ESM.
- `postinstall` never runs apt/brew/winget and never checks Docker. It MAY auto-install rustup (`-y`). Missing C toolchain → print the exact fix command and exit non-zero.
- After a successful build, postinstall copies `target/release/rpi[.exe]` → `dist/rpi[.exe]` (mode `0o755`) and deletes `target/` entirely (SD-card space on the Pi).
- Canonical agent binary path is exactly `/usr/local/bin/rpi`; the systemd unit keeps the name `pi-agent.service`, only its `ExecStart` changes (existing v0.5 units get backed up to `.bak` by the existing `write_unit_with_backup`).
- Self-install comparison is byte-wise; replacement is atomic (`.rpi.tmp` in the target dir → rename); never delete user files. The old `/usr/local/bin/pi` from v0.5 is left in place (manual cleanup, documented in README).
- Docker error/warning messages in setup must include: `curl -fsSL https://get.docker.com | sh`.
- Versions must match: `Cargo.toml` `[workspace.package] version` == `package.json` `version` == `0.6.0`; `scripts/check-version.js` enforces this on `prepublishOnly`.
- Repo URLs use `khmilevoi/rpi-deploy` (repo renamed from `pi-deploy` on 2026-07-02).
- Per repo CLAUDE.md, prefix commands with `rtk` (e.g. `rtk cargo test`, `rtk git add`); `rtk` passes unknown commands through unchanged.
- Local verification: Windows 11 host (PowerShell tool = pwsh 7); npm global prefix is `%APPDATA%\npm`. Rust tests must pass on Windows (gate unix-only asserts with `#[cfg(unix)]`).
- `npm publish` is manual and NOT part of this plan (post-acceptance step).

## File Structure

```
crates/bin/Cargo.toml                 # [[bin]] name = "rpi" (package name "pi" stays)
crates/bin/src/main.rs                # clap name = "rpi"
crates/bin/src/agent/setup.rs         # ExecStart -> /usr/local/bin/rpi; restart helper; docker hints; run_cmd wiring
crates/bin/src/agent/self_install.rs  # NEW: byte-compare + atomic copy of the running binary
crates/bin/src/agent/mod.rs           # + pub mod self_install;
crates/*/src/**                       # user-facing strings & rustdoc: `pi …` -> `rpi …`
package.json                          # NEW: npm manifest
bin/rpi.js                            # NEW: Node shim for the built binary
scripts/postinstall.js                # NEW: rustup provisioning + cargo build + dist + cleanup
scripts/check-version.js              # NEW: prepublishOnly version guard
LICENSE                               # NEW: MIT
.gitignore                            # + /dist, /node_modules
Cargo.toml                            # version 0.5.0 -> 0.6.0 (line 6)
README.md                             # title/status, "Install Via npm", full pi->rpi command sweep
plugins/pi/skills/pi-cli/SKILL.md     # command sweep pi -> rpi
plugins/pi/skills/pi-toml/SKILL.md    # command sweep pi -> rpi (pi.toml itself stays)
```

---

### Task 1: Rename the CLI command `pi` → `rpi`

**Files:**
- Modify: `crates/bin/Cargo.toml:6-8` (`[[bin]]` section)
- Modify: `crates/bin/src/main.rs:12` (clap name)
- Modify: `crates/bin/src/agent/setup.rs` (UNIT `ExecStart` line 63, warning line 257, error message line 335, unit tests lines 415-416)
- Modify: user-facing strings and rustdoc across `crates/` (exact list in Step 3)

**Interfaces:**
- Consumes: nothing.
- Produces: binary named `rpi` (`target/release/rpi[.exe]`); canonical unit `ExecStart=/usr/local/bin/rpi agent run --config /etc/pi/agent.toml`. Tasks 2–7 all rely on the `rpi` name. Rust package name stays `pi` (so `cargo test -p pi` keeps working).

- [ ] **Step 1: Update the unit-canon test to expect the new ExecStart**

In `crates/bin/src/agent/setup.rs`, in test `unit_template_matches_spec_byte_for_byte` (line ~416), change the ExecStart assert to:

```rust
        assert!(UNIT.contains("ExecStart=/usr/local/bin/rpi agent run --config /etc/pi/agent.toml\n"));
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `rtk cargo test -p pi unit_template_matches_spec_byte_for_byte`
Expected: FAIL — UNIT still contains the old `/usr/local/bin/pi` ExecStart.

- [ ] **Step 3: Perform the rename**

a) `crates/bin/Cargo.toml` — only the `[[bin]]` name changes (the `[package] name = "pi"` on line 2 stays):

```toml
[[bin]]
name = "rpi"
path = "src/main.rs"
```

b) `crates/bin/src/main.rs:12` — clap command name:

```rust
    name = "rpi",
```

c) `crates/bin/src/agent/setup.rs`:

- Line 63 (inside the `UNIT` constant): `ExecStart=/usr/local/bin/rpi agent run --config /etc/pi/agent.toml`
- Line 257: `rep.warnings.push("systemctl enable --now pi-agent failed (is /usr/local/bin/rpi installed?)".into());`
- Line 335: `"cannot determine the SSH login user; run via `sudo rpi agent setup` or pass --user <name>"`

(The unit `Description=pi deploy agent`, the unit filename `pi-agent.service`, and all `pi-agent`/`/etc/pi`/`/var/lib/pi` identifiers do NOT change.)

d) User-facing strings — change `pi <subcommand>` to `rpi <subcommand>` in exactly these places:

- `crates/infrastructure/src/probe.rs:178` — hint `"run \`rpi gc\` or free disk space"`
- `crates/application/src/env.rs:69` — `"project '{project}' is not deployed yet; run \`rpi deploy\` first"`
- `crates/application/src/remove.rs:63` — `"…cancel it first using \`rpi deploy --cancel\`"`
- `crates/bin/src/cli/init.rs:164` — `"next: \`rpi env send\` (if you use secrets), then \`rpi deploy\`"`
- `crates/bin/src/cli/commands.rs:383` — `"is pi-agent.service running on the Pi? try \`rpi agent status\`"` (unit name stays `pi-agent.service`)
- `crates/bin/src/cli/commands.rs:392` — `"agent is unreachable through the tunnel; \`rpi agent logs\` for details"`
- `crates/bin/src/cli/commands.rs:543` — `"rebuild/update the agent on the Pi (\`rpi agent update\` ships in v0.5)"`
- `crates/bin/src/cli/commands.rs:583` — `hint: Some("run \`rpi gc\`".into())`
- `crates/bin/src/cli/commands.rs:590` — test assert: `assert!(out.contains("hint: run \`rpi gc\`"), "{out}");`
- `crates/bin/src/cli/keys.rs:53` — `"Then re-run \`rpi setup\` (or \`rpi doctor --server <alias>\`)."`
- `crates/bin/src/cli/setup.rs:96` — `"fix SSH access, then run \`rpi doctor --server {name}\`"`

e) Rustdoc sweep — update remaining doc-comment command mentions (`` `pi env send` ``, `` `pi ls` ``, `` `pi deploy --cancel` ``, …) to `` `rpi …` `` in: `crates/infrastructure/src/dotenv.rs`, `crates/domain/src/contracts.rs`, `crates/domain/src/entities.rs`, `crates/application/src/{env.rs,gc.rs,list.rs,scheduler.rs}`, `crates/bin/src/cli/{init.rs,commands.rs,pitoml.rs,setup.rs}`, `crates/bin/src/agent/http.rs`. Find them all with the Step 4 greps — mechanical replace of the command word only (never `pi.toml`, `pi-agent`, paths).

- [ ] **Step 4: Verify no stale command references remain**

Run both searches over `crates/`:

```
rtk grep -n '`pi ' crates
rtk grep -n '/usr/local/bin/pi[^a-z]' crates
rtk grep -n 'sudo pi ' crates
```

Expected: no matches for any of the three. (`pi-agent`, `/etc/pi`, `pi.toml` do not match these patterns and must remain.)

- [ ] **Step 5: Full workspace test run**

Run: `rtk cargo test --workspace`
Expected: PASS, 0 failed — including the updated unit-canon and `rpi gc` hint tests. Also confirm the binary name (PowerShell):

```powershell
rtk cargo build -p pi; Test-Path target\debug\rpi.exe
```

Expected: build ok, then `True`.

- [ ] **Step 6: Commit**

```bash
rtk git add crates && rtk git commit -m "feat!: rename CLI command pi -> rpi (binary, ExecStart, messages)"
```

---

### Task 2: Rust module `self_install`

**Files:**
- Create: `crates/bin/src/agent/self_install.rs`
- Modify: `crates/bin/src/agent/mod.rs` (add module)

**Interfaces:**
- Consumes: the `rpi` rename from Task 1 (canonical path value).
- Produces (used by Task 3):
  - `pub const AGENT_BIN_PATH: &str = "/usr/local/bin/rpi";`
  - `pub enum SelfInstallAction { AlreadyCanonical, UpToDate, Installed }` (derives `Debug, PartialEq, Eq`)
  - `pub fn ensure_installed(current: &Path, target: &Path, dry_run: bool) -> Result<SelfInstallAction, String>`

- [ ] **Step 1: Register the module and write the failing tests**

In `crates/bin/src/agent/mod.rs` add (alphabetical order):

```rust
pub mod self_install;
```

Create `crates/bin/src/agent/self_install.rs` with the tests and a stub (implementation in Step 3):

```rust
use std::fs;
use std::io;
use std::path::Path;

/// Canonical agent binary path — must match ExecStart in setup::UNIT.
pub const AGENT_BIN_PATH: &str = "/usr/local/bin/rpi";

#[derive(Debug, PartialEq, Eq)]
pub enum SelfInstallAction {
    /// The running binary already is the canonical file — nothing to do.
    AlreadyCanonical,
    /// The canonical binary is byte-identical — nothing to do.
    UpToDate,
    /// The canonical binary was (or, in dry-run, would be) written.
    Installed,
}

/// Copy `current` (the running binary) over `target` when they differ.
/// Atomic: write `<target dir>/.rpi.tmp`, chmod 0755, rename over target.
pub fn ensure_installed(
    current: &Path,
    target: &Path,
    dry_run: bool,
) -> Result<SelfInstallAction, String> {
    let _ = (current, target, dry_run);
    unimplemented!("Task 2 Step 3")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_dirs() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("node_modules-rpi");
        let target = dir.path().join("usr-local-bin").join("rpi");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&current, b"binary-v2").unwrap();
        (dir, current, target)
    }

    #[test]
    fn installs_when_target_missing() {
        let (_d, current, target) = setup_dirs();
        let action = ensure_installed(&current, &target, false).unwrap();
        assert_eq!(action, SelfInstallAction::Installed);
        assert_eq!(fs::read(&target).unwrap(), b"binary-v2");
    }

    #[test]
    fn replaces_when_target_differs() {
        let (_d, current, target) = setup_dirs();
        fs::write(&target, b"binary-v1").unwrap();
        let action = ensure_installed(&current, &target, false).unwrap();
        assert_eq!(action, SelfInstallAction::Installed);
        assert_eq!(fs::read(&target).unwrap(), b"binary-v2");
    }

    #[test]
    fn up_to_date_when_identical() {
        let (_d, current, target) = setup_dirs();
        fs::write(&target, b"binary-v2").unwrap();
        let action = ensure_installed(&current, &target, false).unwrap();
        assert_eq!(action, SelfInstallAction::UpToDate);
    }

    #[test]
    fn already_canonical_when_current_is_target() {
        let (_d, _current, target) = setup_dirs();
        fs::write(&target, b"binary-v2").unwrap();
        let action = ensure_installed(&target, &target, false).unwrap();
        assert_eq!(action, SelfInstallAction::AlreadyCanonical);
        assert_eq!(fs::read(&target).unwrap(), b"binary-v2", "file untouched");
    }

    #[test]
    fn dry_run_reports_but_does_not_write() {
        let (_d, current, target) = setup_dirs();
        let action = ensure_installed(&current, &target, true).unwrap();
        assert_eq!(action, SelfInstallAction::Installed);
        assert!(!target.exists(), "dry run must not create the target");
    }

    #[test]
    fn no_tmp_file_left_behind() {
        let (_d, current, target) = setup_dirs();
        ensure_installed(&current, &target, false).unwrap();
        assert!(!target.parent().unwrap().join(".rpi.tmp").exists());
    }

    #[cfg(unix)]
    #[test]
    fn installed_binary_is_executable() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, current, target) = setup_dirs();
        ensure_installed(&current, &target, false).unwrap();
        let mode = fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk cargo test -p pi self_install`
Expected: FAIL — every test panics with `not implemented: Task 2 Step 3`.

- [ ] **Step 3: Implement `ensure_installed`**

Replace the stub body in `crates/bin/src/agent/self_install.rs`:

```rust
pub fn ensure_installed(
    current: &Path,
    target: &Path,
    dry_run: bool,
) -> Result<SelfInstallAction, String> {
    if is_same_file(current, target) {
        return Ok(SelfInstallAction::AlreadyCanonical);
    }
    let cur_bytes = fs::read(current).map_err(|e| format!("read {}: {e}", current.display()))?;
    match fs::read(target) {
        Ok(t) if t == cur_bytes => return Ok(SelfInstallAction::UpToDate),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("read {}: {e}", target.display())),
    }
    if dry_run {
        return Ok(SelfInstallAction::Installed);
    }
    let dir = target
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", target.display()))?;
    let tmp = dir.join(".rpi.tmp");
    fs::write(&tmp, &cur_bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod {}: {e}", tmp.display()))?;
    }
    fs::rename(&tmp, target)
        .map_err(|e| format!("rename {} -> {}: {e}", tmp.display(), target.display()))?;
    Ok(SelfInstallAction::Installed)
}

/// True when both paths resolve to the same existing file.
fn is_same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test -p pi self_install`
Expected: PASS — 6 tests on Windows (7 on unix), 0 failed.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/bin/src/agent/self_install.rs crates/bin/src/agent/mod.rs && rtk git commit -m "feat(agent): self-install module copies running binary to /usr/local/bin/rpi"
```

---

### Task 3: Wire self-install into `rpi agent setup` + restart + Docker hints

**Files:**
- Modify: `crates/bin/src/agent/setup.rs` (docker group step ~line 210, dependency checks ~line 267, `run_cmd` ~line 330, tests)

**Interfaces:**
- Consumes (from Task 2): `self_install::{AGENT_BIN_PATH, SelfInstallAction, ensure_installed}`.
- Produces: `pub async fn restart_agent_if_active(sys: &dyn Sys) -> Option<String>` in `setup.rs`.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/bin/src/agent/setup.rs`:

```rust
#[tokio::test]
async fn restart_runs_when_unit_active() {
    let mut sys = FakeSys::default();
    sys.ok.insert(FakeSys::key("systemctl", &["is-active", "--quiet", "pi-agent"]), "".into());
    sys.ok.insert(FakeSys::key("systemctl", &["restart", "pi-agent"]), "".into());
    let note = restart_agent_if_active(&sys).await;
    assert!(note.unwrap().contains("restarted"), "reports the restart");
    assert!(sys.calls().iter().any(|c| c == "systemctl restart pi-agent"));
}

#[tokio::test]
async fn restart_skipped_when_unit_inactive() {
    let mut sys = FakeSys::default();
    sys.err.insert(FakeSys::key("systemctl", &["is-active", "--quiet", "pi-agent"]));
    let note = restart_agent_if_active(&sys).await;
    assert!(note.is_none());
    assert!(!sys.calls().iter().any(|c| c.contains("systemctl restart")), "no restart attempted");
}

#[tokio::test]
async fn restart_failure_returns_warning() {
    let mut sys = FakeSys::default();
    sys.ok.insert(FakeSys::key("systemctl", &["is-active", "--quiet", "pi-agent"]), "".into());
    sys.err.insert(FakeSys::key("systemctl", &["restart", "pi-agent"]));
    let note = restart_agent_if_active(&sys).await;
    assert!(note.unwrap().starts_with("warning:"));
}
```

And extend the existing `missing_docker_warns_not_fails` test — replace its final assert with:

```rust
    let w = report.warnings.iter().find(|w| w.contains("docker")).expect("docker warning present");
    assert!(w.contains("curl -fsSL https://get.docker.com | sh"), "warning includes the install command: {w}");
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `rtk cargo test -p pi setup`
Expected: FAIL — `restart_agent_if_active` not found (compile error); after Step 3 compiles, the docker-hint assert would fail until the message changes.

- [ ] **Step 3: Implement**

In `crates/bin/src/agent/setup.rs`:

a) Add at the top, after `use async_trait::async_trait;`:

```rust
use super::self_install::{self, SelfInstallAction};
```

b) Add the restart helper right after the `setup` function (before `CLOUDFLARED_UNIT_PATH`):

```rust
/// Restart pi-agent when it is active, so a replaced binary takes effect.
/// Returns a printable note; None when the unit is not active.
pub async fn restart_agent_if_active(sys: &dyn Sys) -> Option<String> {
    if sys.run("systemctl", &["is-active", "--quiet", "pi-agent"]).await.is_err() {
        return None;
    }
    match sys.run("systemctl", &["restart", "pi-agent"]).await {
        Ok(_) => Some("restarted: pi-agent (new binary)".into()),
        Err(e) => Some(format!("warning: systemctl restart pi-agent failed: {e}")),
    }
}
```

c) Docker hints. In step 2 of `setup()` (usermod docker error), change the error push to:

```rust
            Err(e) => rep.errors.push(format!(
                "usermod pi-agent docker failed: {e}. Install Docker first: curl -fsSL https://get.docker.com | sh"
            )),
```

In step 10 (dependency checks), change the docker warning to:

```rust
        rep.warnings.push(
            "docker not available — install Docker first: curl -fsSL https://get.docker.com | sh".into(),
        );
```

d) Rewrite `run_cmd` to self-install before setup and restart after:

```rust
/// CLI entrypoint: resolve the login user (--user or $SUDO_USER), install the
/// running binary to /usr/local/bin/rpi (npm installs live under node_modules,
/// but the systemd unit expects the canonical path), run setup, and restart
/// an active agent when the binary changed. Must run as root (under sudo).
pub async fn run_cmd(user: Option<String>, with_cloudflared: bool, dry_run: bool) -> anyhow::Result<()> {
    let login_user = user
        .or_else(|| std::env::var("SUDO_USER").ok())
        .filter(|u| !u.is_empty() && u != "root")
        .ok_or_else(|| anyhow::anyhow!(
            "cannot determine the SSH login user; run via `sudo rpi agent setup` or pass --user <name>"
        ))?;
    let opts = SetupOpts { login_user, with_cloudflared, dry_run };

    let current = std::env::current_exe().map_err(|e| anyhow::anyhow!("current_exe: {e}"))?;
    let action = self_install::ensure_installed(
        &current,
        Path::new(self_install::AGENT_BIN_PATH),
        dry_run,
    )
    .map_err(|e| anyhow::anyhow!("self-install {}: {e}", self_install::AGENT_BIN_PATH))?;
    match &action {
        SelfInstallAction::AlreadyCanonical => {
            println!("ok (already present): {} (running from it)", self_install::AGENT_BIN_PATH);
        }
        SelfInstallAction::UpToDate => {
            println!("ok (already present): {} (binary up to date)", self_install::AGENT_BIN_PATH);
        }
        SelfInstallAction::Installed => {
            println!(
                "{}: {} (from {})",
                if dry_run { "would install" } else { "installed" },
                self_install::AGENT_BIN_PATH,
                current.display(),
            );
        }
    }

    run_with(&HostSys, &opts).await?;

    if matches!(action, SelfInstallAction::Installed) && !dry_run {
        if let Some(note) = restart_agent_if_active(&HostSys).await {
            println!("{note}");
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `rtk cargo test -p pi setup`
Expected: PASS — all existing setup tests plus the 3 new restart tests; the docker-hint assert passes.

- [ ] **Step 5: Full workspace regression**

Run: `rtk cargo test --workspace`
Expected: PASS, 0 failed.

- [ ] **Step 6: Commit**

```bash
rtk git add crates/bin/src/agent/setup.rs && rtk git commit -m "feat(agent): setup self-installs binary and restarts active daemon; docker hints"
```

---

### Task 4: npm package — manifest, shim, postinstall, version guard, LICENSE

**Files:**
- Create: `package.json`, `bin/rpi.js`, `scripts/postinstall.js`, `scripts/check-version.js`, `LICENSE`
- Modify: `.gitignore`

**Interfaces:**
- Consumes: binary name `rpi` from Task 1 (`target/release/rpi[.exe]`).
- Produces: postinstall places the binary at `<package>/dist/rpi` (`rpi.exe` on win32) — `bin/rpi.js` depends on exactly that path; `scripts/check-version.js` reads `[workspace.package] version` from `Cargo.toml` and `version` from `package.json` (Task 5 relies on it passing once both are `0.6.0`).

- [ ] **Step 1: Write `package.json`**

```json
{
  "name": "rpi-deploy",
  "version": "0.6.0",
  "description": "Deployment tool for Docker Compose projects on Raspberry Pi. Builds the Rust CLI from source on install.",
  "license": "MIT",
  "repository": {
    "type": "git",
    "url": "git+https://github.com/khmilevoi/rpi-deploy.git"
  },
  "bin": { "rpi": "bin/rpi.js" },
  "files": [
    "bin/",
    "scripts/postinstall.js",
    "scripts/check-version.js",
    "crates/",
    "Cargo.toml",
    "Cargo.lock"
  ],
  "scripts": {
    "postinstall": "node scripts/postinstall.js",
    "prepublishOnly": "node scripts/check-version.js"
  },
  "engines": { "node": ">=18" },
  "os": ["linux", "darwin", "win32"]
}
```

- [ ] **Step 2: Write `bin/rpi.js`**

```js
#!/usr/bin/env node
// Shim installed as the global `rpi` command: runs the native binary that
// scripts/postinstall.js built into dist/.
'use strict';

const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const exe = process.platform === 'win32' ? '.exe' : '';
const bin = path.join(__dirname, '..', 'dist', `rpi${exe}`);

if (!fs.existsSync(bin)) {
  console.error('rpi: binary not built; reinstall without --ignore-scripts: npm install -g rpi-deploy');
  process.exit(1);
}

const r = spawnSync(bin, process.argv.slice(2), { stdio: 'inherit' });
if (r.error) {
  console.error(`rpi: failed to run ${bin}: ${r.error.message}`);
  process.exit(1);
}
if (r.signal) {
  // POSIX convention: terminated by signal N -> exit code 128+N.
  process.exit(128 + (os.constants.signals[r.signal] || 0));
}
process.exit(r.status ?? 1);
```

- [ ] **Step 3: Write `scripts/postinstall.js`**

```js
#!/usr/bin/env node
// rpi-deploy postinstall: builds the rpi binary from the bundled Rust sources.
// Never runs apt/brew/winget and never checks Docker (that is `rpi agent
// setup`'s job). May auto-install rustup when cargo is missing.
'use strict';

const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const pkgDir = path.resolve(__dirname, '..');
const exe = process.platform === 'win32' ? '.exe' : '';
const cargoBinDir = path.join(os.homedir(), '.cargo', 'bin');

function log(msg) {
  console.log(`rpi-deploy: ${msg}`);
}

function fail(msg) {
  console.error(`rpi-deploy: error: ${msg}`);
  process.exit(1);
}

function which(cmd) {
  const probe = process.platform === 'win32' ? 'where' : 'which';
  return spawnSync(probe, [cmd], { stdio: 'ignore' }).status === 0;
}

// Resolve cargo deterministically: a fresh rustup install is not in PATH yet.
function cargoCmd() {
  const local = path.join(cargoBinDir, `cargo${exe}`);
  return fs.existsSync(local) ? local : 'cargo';
}

function hasCargo() {
  return which('cargo') || fs.existsSync(path.join(cargoBinDir, `cargo${exe}`));
}

async function installRustup() {
  log('cargo not found; installing Rust via rustup (https://rustup.rs)...');
  if (process.platform === 'win32') {
    const arch = process.arch === 'arm64' ? 'aarch64' : 'x86_64';
    const url = `https://win.rustup.rs/${arch}`;
    const tmp = path.join(os.tmpdir(), 'rustup-init.exe');
    const res = await fetch(url);
    if (!res.ok) fail(`download ${url}: HTTP ${res.status}`);
    fs.writeFileSync(tmp, Buffer.from(await res.arrayBuffer()));
    const r = spawnSync(tmp, ['-y'], { stdio: 'inherit' });
    if (r.status !== 0) fail('rustup-init failed');
  } else {
    if (!which('curl')) {
      fail('curl is required to install rustup; install curl, then rerun: npm install -g rpi-deploy');
    }
    const r = spawnSync(
      'sh',
      ['-c', "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"],
      { stdio: 'inherit' }
    );
    if (r.status !== 0) fail('rustup install failed');
  }
  if (!hasCargo()) fail(`cargo still not found after rustup install (expected in ${cargoBinDir})`);
}

function checkCToolchain() {
  if (process.platform === 'win32') return; // cargo reports MSVC problems; hint printed on failure
  if (!which('cc')) {
    if (process.platform === 'darwin') {
      fail('a C compiler is required; install Xcode Command Line Tools: xcode-select --install');
    }
    fail('a C toolchain is required; on Debian/Raspberry Pi OS run: sudo apt-get install -y build-essential pkg-config, then rerun: npm install -g rpi-deploy');
  }
  if (process.platform === 'linux' && !which('pkg-config')) {
    fail('pkg-config is required; on Debian/Raspberry Pi OS run: sudo apt-get install -y build-essential pkg-config, then rerun: npm install -g rpi-deploy');
  }
}

async function main() {
  if (!hasCargo()) await installRustup();
  checkCToolchain();

  log('building rpi from source (cargo build --release --locked); this takes a few minutes (about 10 on a Raspberry Pi)...');
  const build = spawnSync(cargoCmd(), ['build', '--release', '--locked'], {
    cwd: pkgDir,
    stdio: 'inherit',
  });
  if (build.status !== 0) {
    if (process.platform === 'win32') {
      console.error('rpi-deploy: hint: building on Windows needs the Visual Studio Build Tools C++ workload.');
    }
    fail('cargo build failed (see output above)');
  }

  const built = path.join(pkgDir, 'target', 'release', `rpi${exe}`);
  const distDir = path.join(pkgDir, 'dist');
  fs.mkdirSync(distDir, { recursive: true });
  fs.copyFileSync(built, path.join(distDir, `rpi${exe}`));
  fs.chmodSync(path.join(distDir, `rpi${exe}`), 0o755);

  log('removing the build directory to save disk space...');
  fs.rmSync(path.join(pkgDir, 'target'), { recursive: true, force: true });

  log('installed. Next steps:');
  log('  developer machine:  rpi setup   then, inside your project:  rpi init');
  log('  Raspberry Pi agent: sudo rpi agent setup   (Docker must already be installed)');
}

main().catch((e) => fail(String(e && e.stack ? e.stack : e)));
```

- [ ] **Step 4: Write `scripts/check-version.js`**

```js
#!/usr/bin/env node
// prepublishOnly guard: package.json version must equal the Cargo workspace
// version, so a published tarball always builds the matching Rust version.
'use strict';

const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const pkg = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
const cargo = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');

const m = cargo.match(/^\[workspace\.package\][^[]*?^version\s*=\s*"([^"]+)"/ms);
if (!m) {
  console.error('check-version: cannot find [workspace.package] version in Cargo.toml');
  process.exit(1);
}
if (m[1] !== pkg.version) {
  console.error(`check-version: package.json is ${pkg.version} but Cargo.toml workspace is ${m[1]}`);
  process.exit(1);
}
console.log(`check-version: ok (${pkg.version})`);
```

- [ ] **Step 5: Write `LICENSE` (MIT) and extend `.gitignore`**

`LICENSE` (year/name per repo owner; adjust the name if the user prefers a full legal name):

```text
MIT License

Copyright (c) 2026 khmilevoi

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

Append to `.gitignore` (after the existing lines):

```text
/dist
/node_modules
```

- [ ] **Step 6: Syntax-check the JS files**

```powershell
node --check bin\rpi.js; node --check scripts\postinstall.js; node --check scripts\check-version.js
```

Expected: no output, all exit 0.

- [ ] **Step 7: Verify tarball contents**

```powershell
rtk npm pack --dry-run
```

Expected listing includes: `package.json`, `LICENSE`, `README.md`, `bin/rpi.js`, `scripts/postinstall.js`, `scripts/check-version.js`, `Cargo.toml`, `Cargo.lock`, and `crates/**/*.rs` sources. Must NOT include: `target/`, `docs/`, `plugins/`, `.worktrees/`, `dist/`. (`prepublishOnly` does not run on pack, so the temporary 0.5.0/0.6.0 version mismatch with `Cargo.toml` is fine until Task 5.)

- [ ] **Step 8: Commit**

```bash
rtk git add package.json bin/rpi.js scripts/postinstall.js scripts/check-version.js LICENSE .gitignore && rtk git commit -m "feat(npm): rpi-deploy package - rpi shim, postinstall source build, version guard"
```

---

### Task 5: Version bump to 0.6.0

**Files:**
- Modify: `Cargo.toml:6` (`[workspace.package] version`)
- Modify: `Cargo.lock` (regenerated versions of workspace crates)

**Interfaces:**
- Consumes: `scripts/check-version.js` from Task 4.
- Produces: workspace version `0.6.0` matching `package.json` (Task 6 packs this version; README in Task 7 references v0.6).

- [ ] **Step 1: Bump the version**

In the root `Cargo.toml`, change line 6 (was `version = "0.5.0"`):

```toml
version = "0.6.0"
```

- [ ] **Step 2: Full workspace test run (also refreshes Cargo.lock)**

Run: `rtk cargo test --workspace`
Expected: PASS, 0 failed. `rtk git status` afterwards shows only `Cargo.toml` and `Cargo.lock` modified.

- [ ] **Step 3: Version guard passes**

```powershell
node scripts\check-version.js
```

Expected output: `check-version: ok (0.6.0)`, exit 0.

- [ ] **Step 4: Commit**

```bash
rtk git add Cargo.toml Cargo.lock && rtk git commit -m "chore: bump workspace version to 0.6.0"
```

---

### Task 6: End-to-end install smoke from a local tarball

**Files:**
- None created/modified in the repo (throwaway tarball + global npm install, removed at the end).

**Interfaces:**
- Consumes: the complete package from Tasks 4–5 (tarball `rpi-deploy-0.6.0.tgz`).
- Produces: nothing — this is the integration gate for the whole npm flow.

- [ ] **Step 1: Pack the tarball**

```powershell
rtk npm pack
```

Expected: creates `rpi-deploy-0.6.0.tgz` in the repo root.

- [ ] **Step 2: Global install from the tarball (real build, several minutes)**

```powershell
npm install -g .\rpi-deploy-0.6.0.tgz
```

Run with a 600000 ms timeout (or `run_in_background` and wait). Expected: postinstall prints `rpi-deploy: building rpi from source...`, cargo output streams, install ends successfully. (Cold release build of the workspace — expect 3–10 minutes.)

- [ ] **Step 3: Verify the shim, the binary, and the cleanup**

```powershell
& "$env:APPDATA\npm\rpi.cmd" --help; echo "exit=$LASTEXITCODE"
Test-Path "$env:APPDATA\npm\node_modules\rpi-deploy\dist\rpi.exe"
Test-Path "$env:APPDATA\npm\node_modules\rpi-deploy\target"
```

Expected: CLI help text (usage shows `rpi …`) with `exit=0`; then `True` (binary in dist); then `False` (`target/` deleted by postinstall).

- [ ] **Step 4: Optional WSL check (skip silently if npm is absent in WSL)**

```powershell
wsl -d Ubuntu -- bash -lc "command -v npm"
```

If npm exists, install with a user-level prefix (no sudo) and check the shim:

```powershell
wsl -d Ubuntu -- bash -lc "NPM_CONFIG_PREFIX=\$HOME/.npm-global npm install -g /mnt/c/Users/Khmil/RustProjects/pi/rpi-deploy-0.6.0.tgz && \$HOME/.npm-global/bin/rpi --help && NPM_CONFIG_PREFIX=\$HOME/.npm-global npm uninstall -g rpi-deploy"
```

Expected: build succeeds, `rpi --help` prints, uninstall cleans up. If npm is missing in WSL, note it and move on — the Linux path is covered by the manual acceptance matrix.

- [ ] **Step 5: Clean up**

```powershell
npm uninstall -g rpi-deploy
Remove-Item .\rpi-deploy-0.6.0.tgz
& "$env:APPDATA\npm\rpi.cmd" --help
```

Expected: uninstall succeeds; the tarball is gone; the last command now FAILS (shim removed) — confirms clean uninstall.

---

### Task 7: README + skills docs — npm section, status, full command sweep

**Files:**
- Modify: `README.md` — title/intro (lines 1–13), new section before `## Build And Install The Binary` (line 165), heading `## Quick Setup (v0.5)` (line 204), and a `pi …` → `rpi …` command sweep across the whole file
- Modify: `plugins/pi/skills/pi-cli/SKILL.md`, `plugins/pi/skills/pi-toml/SKILL.md` — same command sweep

**Interfaces:**
- Consumes: package name/commands exactly as implemented in Tasks 1–6.
- Produces: nothing.

- [ ] **Step 1: Update the title and status paragraph**

README line 1: `# pi` → `# rpi-deploy`. Line 3 intro sentence: change ``` `pi` is a deployment tool… ``` to ``` `rpi` (package `rpi-deploy`) is a deployment tool… ```. Replace the status paragraph (lines 8–13) with:

```markdown
Status: v0.6 (npm install) — everything from v0.1–v0.5 (deploy/env/ingress/CI,
`rpi logs`, `rpi stats`, `rpi start|stop|restart`, `rpi rm`, `rpi status`,
`rpi doctor`, `rpi agent status|logs`, one-command setup) plus
`npm install -g rpi-deploy` for both roles: the CLI command is now `rpi`, the
package builds it from source on install, and `sudo rpi agent setup` installs
the running binary to `/usr/local/bin/rpi` and restarts the agent on updates.
Manual install from source remains as a fallback (see "Build And Install The
Binary" below).
```

- [ ] **Step 2: Insert the new section before `## Build And Install The Binary`**

````markdown
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
sudo npm install -g rpi-deploy    # builds from source, ~10 minutes on a Pi
sudo rpi agent setup              # installs /usr/local/bin/rpi, unit, start
rpi doctor
```

Update (both roles):

```bash
npm install -g rpi-deploy@latest  # with sudo on the Pi
sudo rpi agent setup              # Pi only: swaps the binary and restarts the agent
```

Upgrading a v0.5 install: the command was renamed `pi` → `rpi`. `sudo rpi
agent setup` rewrites the systemd unit (backing up the old one to
`pi-agent.service.bak`); the old `/usr/local/bin/pi` binary is left behind —
remove it with `sudo rm /usr/local/bin/pi`.

The npm package ships the Rust sources and builds them on install
(`cargo build --release --locked`); rustup is installed automatically when
cargo is missing, and the build directory is removed afterwards to save disk
space. Building on Windows needs the Visual Studio Build Tools C++ workload.
Installing with `--ignore-scripts` leaves the CLI unusable (`rpi` will report
that the binary was not built).
````

- [ ] **Step 3: Command sweep across README and skills docs**

In `README.md`, `plugins/pi/skills/pi-cli/SKILL.md`, `plugins/pi/skills/pi-toml/SKILL.md` replace every *command* mention of `pi` with `rpi`: `` `pi <sub>` `` → `` `rpi <sub>` ``, `sudo pi ` → `sudo rpi `, `pi --help` → `rpi --help`, plain-prose "run pi deploy" style mentions, and the manual-install lines (`sudo install -m 755 target/release/pi /usr/local/bin/pi` → `… target/release/rpi /usr/local/bin/rpi`, `scp target/aarch64…/release/pi` → `…/release/rpi`). Rename `## Quick Setup (v0.5)` to `## Quick Setup`. Do NOT touch: `pi.toml`, `pi-agent` (user/group/unit), `/etc/pi`, `/var/lib/pi`, `/var/log/pi`, `/run/pi`, crate names, the git URLs.

- [ ] **Step 4: Verify the sweep**

```
rtk grep -n '`pi ' README.md plugins
rtk grep -n 'sudo pi ' README.md plugins
rtk grep -n 'target/release/pi[^a-z]' README.md
```

Expected: no matches for any pattern. Then confirm the intended survivors still exist: `rtk grep -c 'pi.toml' README.md` (> 0) and `rtk grep -c 'pi-agent' README.md` (> 0).

- [ ] **Step 5: Commit**

```bash
rtk git add README.md plugins && rtk git commit -m "docs: README install via npm, rpi command sweep, status v0.6"
```

---

## Publish (manual, after acceptance — not a plan task)

From the repo root, once the acceptance matrix below passes: `npm login` (once), then `rtk npm publish`. `prepublishOnly` runs `check-version.js` and blocks a version mismatch. The first publish claims the free `rpi-deploy` name.

## Manual Acceptance Matrix (real hardware, before `npm publish`)

- Fresh Pi (Node >= 18, Docker installed): `sudo npm install -g rpi-deploy` → build ok; `sudo rpi agent setup` → `installed: /usr/local/bin/rpi (...)` in the report, unit active; `rpi doctor` clean.
- Fresh Pi without Docker: install ok; `sudo rpi agent setup` fails with the `curl -fsSL https://get.docker.com | sh` hint; after installing Docker, rerun succeeds.
- Upgrade of an existing v0.5 Pi: `sudo npm install -g rpi-deploy` → `sudo rpi agent setup` → unit rewritten with `.bak` backup, binary at `/usr/local/bin/rpi`, daemon restarted; old `/usr/local/bin/pi` removed manually per README.
- Update on the Pi: bump version → `sudo npm install -g rpi-deploy@latest` → `sudo rpi agent setup` → report shows the binary replaced and `restarted: pi-agent (new binary)`.
- Dev machines: Linux/macOS/Windows `npm install -g rpi-deploy` → `rpi setup` / `rpi init` work; repeat install is idempotent.
- `npm install -g --ignore-scripts rpi-deploy` → `rpi` prints the "binary not built" message, exit 1.
- Machine without cargo: rustup auto-installs, build proceeds.
- Debian without build-essential: postinstall fails with the exact apt command; after installing, rerun succeeds.
