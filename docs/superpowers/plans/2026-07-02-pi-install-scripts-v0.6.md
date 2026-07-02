# pi v0.6 — Install Scripts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Four self-contained install/update scripts (`scripts/install.sh`, `scripts/update.sh`, `scripts/install.ps1`, `scripts/update.ps1`) so that `curl -fsSL …/install.sh | sh` and `powershell -c "irm …/install.ps1 | iex"` bootstrap pi from source, plus README section and workspace version bump to 0.6.0.

**Architecture:** Each script is standalone (no shared sourced files). `install.sh` resolves the role (interactive `/dev/tty` prompt or `--agent`/`--client` flag), provisions build prerequisites, clones into `${XDG_DATA_HOME:-$HOME/.local/share}/pi/src`, builds, installs per role. `update.sh` pulls an existing branch clone and reinstalls. The `.ps1` pair does the same for the Windows client. Docker is a hard prerequisite for the agent role, checked fail-fast before clone/build; the scripts never install it.

**Tech Stack:** POSIX sh (no bashisms), PowerShell 5.1+/7, git, cargo/rustup. No Rust code changes except the workspace version bump.

**Spec:** `docs/superpowers/specs/2026-06-18-pi-install-scripts-v0.6-design.md` (authoritative for behavior).

## Global Constraints

- Repo: `https://github.com/khmilevoi/rpi-deploy.git`; raw script URLs: `https://raw.githubusercontent.com/khmilevoi/rpi-deploy/master/scripts/<name>`.
- The GitHub repo was renamed `pi-deploy` → `rpi-deploy` (2026-07-02). GitHub redirects the old name, but every URL in scripts, README, and docs must use `rpi-deploy`. If the local `origin` still points to `pi-deploy`, fix it: `git remote set-url origin git@github.com:khmilevoi/rpi-deploy.git`.
- `.sh` scripts are POSIX `sh` — no bash arrays, `[[ ]]`, `local`, or process substitution. They must pass `shellcheck` with zero findings.
- `.ps1` scripts must pass `Invoke-ScriptAnalyzer` with zero findings and work when invoked via `irm | iex` (param defaults) and via `& ([scriptblock]::Create(...)) -Flag`.
- Every script is self-contained: duplicate helper functions rather than sourcing a shared file.
- Clone dir: Linux/macOS `${XDG_DATA_HOME:-$HOME/.local/share}/pi/src`; Windows `%LOCALAPPDATA%\pi\src`. Role marker `.pi-role` in the clone root, added to `$DIR/.git/info/exclude` **before** any clean-worktree check.
- Agent binary path is exactly `/usr/local/bin/pi`; agent setup is invoked as `sudo /usr/local/bin/pi agent setup`.
- Client install: `cargo install --path <dir>/crates/bin --locked --force --target-dir <dir>/target` (the package in `crates/bin` is named `pi`).
- Exit codes: `0` = success or successful dry-run; `1` = any error (missing Docker for agent, no role + no TTY, dirty worktree, unknown ref, pinned tag update, unsupported platform, git/cargo failure). There is **no** exit code `2` in v0.6.
- Never run `git reset --hard`, never delete user files. Dirty worktree is always a hard error.
- Docker is never installed by any script; the failure message prints the manual command `curl -fsSL https://get.docker.com | sh`.
- Rust code/logic unchanged; only `[workspace.package] version` bumps to `0.6.0`.
- Per repo CLAUDE.md, prefix commands with `rtk` (e.g. `rtk git add`, `rtk cargo test`). `rtk` passes unknown commands through unchanged.
- Local verification environment: Windows 11 host; run `.sh` smoke tests inside WSL (`wsl -d Ubuntu -- …`) **from PowerShell** (msys Git Bash mangles `/mnt/...` arguments); repo path in WSL is `/mnt/c/Users/Khmil/RustProjects/pi`.

## File Structure

```
scripts/
  install.sh    # Linux/macOS bootstrap: role prompt, prereqs, clone, build, install
  update.sh     # Linux/macOS update: pull branch clone, rebuild, reinstall, restart daemon
  install.ps1   # Windows client bootstrap: rustup, clone, cargo install
  update.ps1    # Windows client update: pull, cargo install
Cargo.toml      # [workspace.package] version 0.5.0 -> 0.6.0 (line 6)
README.md       # new "Install And Update Via Scripts" section + status line -> v0.6
```

---

### Task 1: `scripts/install.sh`

**Files:**
- Create: `scripts/install.sh`

**Interfaces:**
- Produces: clone at `${XDG_DATA_HOME:-$HOME/.local/share}/pi/src` (or `--dir`), file `<dir>/.pi-role` containing `agent` or `client` (single line), `.pi-role` entry in `<dir>/.git/info/exclude`. Task 2 (`update.sh`) relies on all three.
- Consumes: nothing from other tasks.

- [ ] **Step 1: Run the smoke check to verify it fails (no script yet)**

From PowerShell:

```powershell
wsl -d Ubuntu -- sh /mnt/c/Users/Khmil/RustProjects/pi/scripts/install.sh --client --dry-run
```

Expected: FAIL — `sh: 0: cannot open ...: No such file` (or similar), non-zero exit.

- [ ] **Step 2: Write `scripts/install.sh`**

Full content:

```sh
#!/bin/sh
# pi installer: provisions build prerequisites, clones the repo, builds from
# source, and installs the binary for the chosen role (agent or client).
#
#   curl -fsSL https://raw.githubusercontent.com/khmilevoi/rpi-deploy/master/scripts/install.sh | sh
#
# POSIX sh; Linux and macOS. Docker is a prerequisite for the agent role and
# is never installed by this script.
set -eu

REPO_URL="https://github.com/khmilevoi/rpi-deploy.git"
INSTALL_URL="https://raw.githubusercontent.com/khmilevoi/rpi-deploy/master/scripts/install.sh"

ROLE=""
DIR=""
REF="master"
DRY_RUN=0
APT_UPDATED=0

usage() {
    cat <<'EOF'
usage: install.sh [--agent | --client] [--dir <path>] [--ref <branch-or-tag>] [--dry-run]

Without a role flag the script asks interactively (requires a TTY):
  --agent    Raspberry Pi: build, install to /usr/local/bin/pi, run pi agent setup
  --client   developer machine: build and cargo install into ~/.cargo/bin
EOF
}

fail() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

have() {
    command -v "$1" >/dev/null 2>&1
}

apt_install() {
    if [ "$APT_UPDATED" = "0" ]; then
        sudo apt-get update
        APT_UPDATED=1
    fi
    sudo apt-get install -y "$@"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --agent|--client)
            [ -z "$ROLE" ] || fail "--agent and --client are mutually exclusive"
            ROLE="${1#--}"
            ;;
        --dir)
            [ $# -ge 2 ] || fail "--dir requires a value"
            DIR="$2"
            shift
            ;;
        --ref)
            [ $# -ge 2 ] || fail "--ref requires a value"
            REF="$2"
            shift
            ;;
        --dry-run)
            DRY_RUN=1
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            fail "unknown argument: $1"
            ;;
    esac
    shift
done

# Role: flag, otherwise interactive prompt on /dev/tty (stdin is the curl pipe).
if [ -z "$ROLE" ]; then
    if (exec </dev/tty) 2>/dev/null; then
        printf 'Install pi as:\n' >/dev/tty
        printf '  1) agent  (Raspberry Pi: sudo, systemd, pi agent setup)\n' >/dev/tty
        printf '  2) client (developer machine: cargo install)\n' >/dev/tty
        printf 'Choice [1/2]: ' >/dev/tty
        read -r choice </dev/tty
        case "$choice" in
            1|agent) ROLE="agent" ;;
            2|client) ROLE="client" ;;
            *) fail "unrecognized choice: '$choice' (expected 1, 2, agent, or client)" ;;
        esac
    else
        fail "no TTY for the role prompt; rerun with a role flag: sh -s -- --agent | --client"
    fi
fi

OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS" in
    Linux|Darwin) ;;
    *) fail "unsupported OS: $OS (Linux and macOS only; on Windows use install.ps1)" ;;
esac

HAS_APT=0
have apt-get && HAS_APT=1

if [ "$ROLE" = "agent" ]; then
    [ "$OS" = "Linux" ] || fail "the agent role requires Linux with systemd (detected: $OS)"
    have systemctl || fail "the agent role requires systemd (systemctl not found)"
fi

[ -n "$DIR" ] || DIR="${XDG_DATA_HOME:-$HOME/.local/share}/pi/src"

if [ "$DRY_RUN" = "1" ]; then
    docker_status="missing"
    have docker && docker_status="ok"
    docker_note=""
    if [ "$ROLE" = "agent" ] && [ "$docker_status" = "missing" ]; then
        docker_note=" (required for agent: a real run stops here with exit 1)"
    fi
    git_status="missing"
    have git && git_status="ok"
    cc_status="missing"
    have cc && cc_status="ok"
    cargo_status="missing"
    have cargo && cargo_status="ok"
    if [ "$ROLE" = "agent" ]; then
        install_step="sudo install -m 755 target/release/pi /usr/local/bin/pi && sudo /usr/local/bin/pi agent setup"
    else
        install_step="cargo install --path crates/bin --locked --force"
    fi
    cat <<EOF
pi install plan (dry run: nothing is executed)

  role: $ROLE
  os:   $OS ($ARCH)
  ref:  $REF
  dir:  $DIR

prerequisites:
  docker: $docker_status$docker_note
  git:    $git_status
  cc:     $cc_status
  cargo:  $cargo_status

steps:
  1. check Docker (agent only; never installed by this script)
  2. provision git, build tools, rustup (apt/rustup as needed)
  3. clone or update $REPO_URL ($REF) in $DIR
  4. cargo build --release
  5. $install_step

exit codes: 0 = done, 1 = error (missing Docker for agent, dirty worktree,
unknown ref, unsupported platform, no role + no TTY, git/cargo failure)
EOF
    exit 0
fi

# Docker is a hard prerequisite for the agent role: fail fast before any
# clone/build work. This script never installs Docker.
if [ "$ROLE" = "agent" ] && ! have docker; then
    cat >&2 <<EOF
Docker is required for the agent role. Nothing was installed.

Install Docker first:
  curl -fsSL https://get.docker.com | sh

Then rerun:
  curl -fsSL $INSTALL_URL | sh
EOF
    exit 1
fi

if ! have git; then
    if [ "$HAS_APT" = "1" ]; then
        apt_install git
    else
        fail "git is not installed; install it manually and rerun"
    fi
fi

if [ "$OS" = "Linux" ] && { ! have cc || ! have pkg-config; }; then
    if [ "$HAS_APT" = "1" ]; then
        apt_install build-essential pkg-config
    else
        fail "a C toolchain (cc, pkg-config) is required; install it manually and rerun"
    fi
fi

if [ "$OS" = "Darwin" ] && ! have cc; then
    fail "a C compiler is required; install Xcode Command Line Tools: xcode-select --install"
fi

if ! have cargo; then
    if [ ! -x "$HOME/.cargo/bin/cargo" ]; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    fi
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi

ensure_role_excluded() {
    exclude="$DIR/.git/info/exclude"
    mkdir -p "$DIR/.git/info"
    grep -qxF '.pi-role' "$exclude" 2>/dev/null || printf '.pi-role\n' >>"$exclude"
}

if [ ! -d "$DIR/.git" ]; then
    if [ -e "$DIR" ] && [ -n "$(ls -A "$DIR" 2>/dev/null)" ]; then
        fail "$DIR exists, is not empty, and is not a git clone; choose another --dir"
    fi
    mkdir -p "$DIR"
    git clone --branch "$REF" "$REPO_URL" "$DIR"
    ensure_role_excluded
else
    ensure_role_excluded
    [ -z "$(git -C "$DIR" status --porcelain)" ] || \
        fail "install directory has local changes. Commit/stash/remove them, or choose another --dir."
    git -C "$DIR" fetch --tags origin
    if git -C "$DIR" show-ref --verify --quiet "refs/remotes/origin/$REF"; then
        if git -C "$DIR" show-ref --verify --quiet "refs/heads/$REF"; then
            git -C "$DIR" checkout "$REF"
        else
            git -C "$DIR" checkout -b "$REF" --track "origin/$REF"
        fi
        git -C "$DIR" pull --ff-only
    elif git -C "$DIR" show-ref --verify --quiet "refs/tags/$REF"; then
        git -C "$DIR" checkout "refs/tags/$REF"
    else
        fail "ref not found as branch or tag: $REF"
    fi
fi

printf '%s\n' "$ROLE" >"$DIR/.pi-role"

(cd "$DIR" && cargo build --release)

if [ "$ROLE" = "agent" ]; then
    sudo install -m 755 "$DIR/target/release/pi" /usr/local/bin/pi
    sudo /usr/local/bin/pi agent setup
    cat <<'EOF'

pi agent installed. Next steps:
  pi doctor

Note: pi-agent group membership takes effect in a new SSH session.
EOF
else
    cargo install --path "$DIR/crates/bin" --locked --force --target-dir "$DIR/target"
    cat <<'EOF'

pi client installed to ~/.cargo/bin/pi. Next steps:
  pi setup   # server profile wizard
  pi init    # generate pi.toml in your project
EOF
fi
```

- [ ] **Step 3: Dry-run smoke in WSL**

From PowerShell:

```powershell
wsl -d Ubuntu -- sh /mnt/c/Users/Khmil/RustProjects/pi/scripts/install.sh --client --dry-run; echo "exit=$LASTEXITCODE"
```

Expected: the "pi install plan (dry run…)" block with `role: client`, `os: Linux`, `ref: master`, prerequisite statuses, and `exit=0`.

- [ ] **Step 4: No-TTY, no-flag error path**

`setsid` detaches the controlling terminal, so the `/dev/tty` probe fails the way it does in CI:

```powershell
wsl -d Ubuntu -- setsid --wait sh /mnt/c/Users/Khmil/RustProjects/pi/scripts/install.sh; echo "exit=$LASTEXITCODE"
```

Expected: `error: no TTY for the role prompt; rerun with a role flag: sh -s -- --agent | --client` and `exit=1`.

- [ ] **Step 5: Argument validation error paths**

```powershell
wsl -d Ubuntu -- sh /mnt/c/Users/Khmil/RustProjects/pi/scripts/install.sh --agent --client; echo "exit=$LASTEXITCODE"
wsl -d Ubuntu -- sh /mnt/c/Users/Khmil/RustProjects/pi/scripts/install.sh --bogus; echo "exit=$LASTEXITCODE"
```

Expected: first prints `error: --agent and --client are mutually exclusive`, second prints usage + `error: unknown argument: --bogus`; both `exit=1`.

- [ ] **Step 6: shellcheck**

Install once (PowerShell): `winget install --id koalaman.shellcheck --accept-source-agreements --accept-package-agreements`. Then:

```powershell
& "$env:LOCALAPPDATA\Microsoft\WinGet\Links\shellcheck.exe" scripts/install.sh; echo "exit=$LASTEXITCODE"
```

Expected: no output, `exit=0`. (If winget placed the shim elsewhere, `shellcheck scripts/install.sh` from a fresh shell.) Fix any findings and re-run until clean.

- [ ] **Step 7: Commit**

```bash
rtk git add scripts/install.sh && rtk git commit -m "feat(scripts): add install.sh source-build installer"
```

---

### Task 2: `scripts/update.sh`

**Files:**
- Create: `scripts/update.sh`

**Interfaces:**
- Consumes: clone dir convention `${XDG_DATA_HOME:-$HOME/.local/share}/pi/src`, role marker file `<dir>/.pi-role` (single line `agent`|`client`), `.pi-role` excluded via `<dir>/.git/info/exclude` — all produced by Task 1.
- Produces: nothing consumed by later tasks.

- [ ] **Step 1: Prepare a throwaway clone in WSL and verify the smoke fails**

From PowerShell:

```powershell
wsl -d Ubuntu -- sh -c "rm -rf /tmp/pi-src && git clone --quiet /mnt/c/Users/Khmil/RustProjects/pi /tmp/pi-src && printf 'client\n' > /tmp/pi-src/.pi-role"
wsl -d Ubuntu -- sh /mnt/c/Users/Khmil/RustProjects/pi/scripts/update.sh --dir /tmp/pi-src --dry-run; echo "exit=$LASTEXITCODE"
```

Expected: second command FAILs — `cannot open .../update.sh`, non-zero exit.

- [ ] **Step 2: Write `scripts/update.sh`**

Full content:

```sh
#!/bin/sh
# pi updater: pulls the branch clone, rebuilds, reinstalls, and (for the
# agent role) restarts the pi-agent daemon. Prerequisite provisioning lives
# in install.sh; this script only updates an existing install.
set -eu

ROLE=""
DIR=""
DRY_RUN=0

usage() {
    cat <<'EOF'
usage: update.sh [--agent | --client] [--dir <path>] [--dry-run]

The role is read from <dir>/.pi-role; a flag overrides it.
EOF
}

fail() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

have() {
    command -v "$1" >/dev/null 2>&1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --agent|--client)
            [ -z "$ROLE" ] || fail "--agent and --client are mutually exclusive"
            ROLE="${1#--}"
            ;;
        --dir)
            [ $# -ge 2 ] || fail "--dir requires a value"
            DIR="$2"
            shift
            ;;
        --dry-run)
            DRY_RUN=1
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            fail "unknown argument: $1"
            ;;
    esac
    shift
done

[ -n "$DIR" ] || DIR="${XDG_DATA_HOME:-$HOME/.local/share}/pi/src"
[ -d "$DIR/.git" ] || fail "no git clone at $DIR; run install.sh first"

if [ -z "$ROLE" ] && [ -f "$DIR/.pi-role" ]; then
    ROLE="$(cat "$DIR/.pi-role")"
fi
case "$ROLE" in
    agent|client) ;;
    *) fail "cannot determine role (no flag, no $DIR/.pi-role); pass --agent or --client" ;;
esac

if [ "$DRY_RUN" = "1" ]; then
    docker_status="missing"
    have docker && docker_status="ok"
    branch="$(git -C "$DIR" symbolic-ref --quiet --short HEAD || printf 'DETACHED')"
    if [ "$ROLE" = "agent" ]; then
        install_step="sudo install -m 755 target/release/pi /usr/local/bin/pi && sudo /usr/local/bin/pi agent setup && restart pi-agent if active"
    else
        install_step="cargo install --path crates/bin --locked --force"
    fi
    cat <<EOF
pi update plan (dry run: nothing is executed)

  role:   $ROLE
  dir:    $DIR
  branch: $branch (update requires a branch checkout; DETACHED means a pinned install)
  docker: $docker_status (required for the agent role)

steps:
  1. check Docker (agent only; never installed by this script)
  2. verify clean worktree and branch checkout
  3. git pull --ff-only
  4. cargo build --release
  5. $install_step

exit codes: 0 = done, 1 = error (pinned/detached checkout, dirty worktree,
missing Docker for agent, non-ff pull, git/cargo failure)
EOF
    exit 0
fi

# Docker is a hard prerequisite for the agent role; never installed here.
if [ "$ROLE" = "agent" ] && ! have docker; then
    cat >&2 <<'EOF'
Docker is required for the agent role.

Install Docker first:
  curl -fsSL https://get.docker.com | sh
EOF
    exit 1
fi

exclude="$DIR/.git/info/exclude"
mkdir -p "$DIR/.git/info"
grep -qxF '.pi-role' "$exclude" 2>/dev/null || printf '.pi-role\n' >>"$exclude"

[ -z "$(git -C "$DIR" status --porcelain)" ] || \
    fail "install directory has local changes. Commit/stash/remove them first."

git -C "$DIR" symbolic-ref --quiet HEAD >/dev/null || \
    fail "This install is pinned (tag/detached checkout); rerun install.sh with --ref <new-tag>"

git -C "$DIR" pull --ff-only

if ! have cargo && [ -x "$HOME/.cargo/bin/cargo" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi
have cargo || fail "cargo not found; run install.sh first"

(cd "$DIR" && cargo build --release)

if [ "$ROLE" = "agent" ]; then
    sudo install -m 755 "$DIR/target/release/pi" /usr/local/bin/pi
    sudo /usr/local/bin/pi agent setup
    if systemctl is-active --quiet pi-agent; then
        sudo systemctl restart pi-agent
    fi
    printf '\npi agent updated. Check: pi doctor\n'
else
    cargo install --path "$DIR/crates/bin" --locked --force --target-dir "$DIR/target"
    printf '\npi client updated.\n'
fi
```

- [ ] **Step 3: Dry-run smoke against the throwaway clone**

```powershell
wsl -d Ubuntu -- sh /mnt/c/Users/Khmil/RustProjects/pi/scripts/update.sh --dir /tmp/pi-src --dry-run; echo "exit=$LASTEXITCODE"
```

Expected: "pi update plan" block with `role: client` (read from `.pi-role`), a real branch name, and `exit=0`. Note `.pi-role` is untracked — the dry run must NOT be blocked by it later; that is what the exclude logic guarantees.

- [ ] **Step 4: Dirty worktree error path (also proves `.pi-role` exclusion works)**

```powershell
wsl -d Ubuntu -- sh -c "touch /tmp/pi-src/junk.txt"
wsl -d Ubuntu -- sh /mnt/c/Users/Khmil/RustProjects/pi/scripts/update.sh --dir /tmp/pi-src; echo "exit=$LASTEXITCODE"
wsl -d Ubuntu -- rm /tmp/pi-src/junk.txt
```

Expected: `error: install directory has local changes. Commit/stash/remove them first.` and `exit=1`. It must fail because of `junk.txt`, not `.pi-role` — after removing `junk.txt` the next step must get past the clean check.

- [ ] **Step 5: Pinned/detached error path**

```powershell
wsl -d Ubuntu -- git -C /tmp/pi-src checkout --detach --quiet
wsl -d Ubuntu -- sh /mnt/c/Users/Khmil/RustProjects/pi/scripts/update.sh --dir /tmp/pi-src; echo "exit=$LASTEXITCODE"
wsl -d Ubuntu -- rm -rf /tmp/pi-src
```

Expected: `error: This install is pinned (tag/detached checkout); rerun install.sh with --ref <new-tag>` and `exit=1`. (Both error paths exit before `git pull`/`cargo build`, so nothing heavy runs.)

- [ ] **Step 6: shellcheck**

```powershell
& "$env:LOCALAPPDATA\Microsoft\WinGet\Links\shellcheck.exe" scripts/update.sh; echo "exit=$LASTEXITCODE"
```

Expected: no output, `exit=0`.

- [ ] **Step 7: Commit**

```bash
rtk git add scripts/update.sh && rtk git commit -m "feat(scripts): add update.sh pull-rebuild-reinstall updater"
```

---

### Task 3: `scripts/install.ps1`

**Files:**
- Create: `scripts/install.ps1`

**Interfaces:**
- Produces: clone at `%LOCALAPPDATA%\pi\src` (or `-Dir`), file `<dir>\.pi-role` containing `client`, `.pi-role` entry in `<dir>\.git\info\exclude`. Task 4 (`update.ps1`) relies on the clone dir convention and the exclude entry.
- Consumes: nothing from other tasks.

- [ ] **Step 1: Run the smoke check to verify it fails (no script yet)**

```powershell
pwsh -File scripts\install.ps1 -DryRun
```

Expected: FAIL — file does not exist.

- [ ] **Step 2: Write `scripts/install.ps1`**

Full content:

```powershell
#Requires -Version 5.1
<#
pi client installer for Windows: provisions rustup, clones the repo, and
builds/installs the pi CLI from source. The only role on Windows is client.

Default one-liner (from cmd or another shell; inside PowerShell just irm|iex):
  powershell -c "irm https://raw.githubusercontent.com/khmilevoi/rpi-deploy/master/scripts/install.ps1 | iex"

Parameterized run:
  & ([scriptblock]::Create((irm https://raw.githubusercontent.com/khmilevoi/rpi-deploy/master/scripts/install.ps1))) -DryRun
  & ([scriptblock]::Create((irm https://raw.githubusercontent.com/khmilevoi/rpi-deploy/master/scripts/install.ps1))) -Ref v0.6.0
#>
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '',
    Justification = 'installer talks to the interactive console')]
param(
    [string]$Dir = "$env:LOCALAPPDATA\pi\src",
    [string]$Ref = 'master',
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$RepoUrl = 'https://github.com/khmilevoi/rpi-deploy.git'

function Fail([string]$Message) {
    Write-Host "error: $Message" -ForegroundColor Red
    exit 1
}

function Have([string]$Name) {
    return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

function Add-RoleExclude([string]$CloneDir) {
    $infoDir = Join-Path $CloneDir '.git\info'
    if (-not (Test-Path $infoDir)) {
        New-Item -ItemType Directory -Force $infoDir | Out-Null
    }
    $exclude = Join-Path $infoDir 'exclude'
    if (-not (Test-Path $exclude) -or
        -not (Select-String -Path $exclude -Pattern '^\.pi-role$' -Quiet)) {
        Add-Content -Path $exclude -Value '.pi-role'
    }
}

if ($DryRun) {
    $gitStatus = if (Have git) { 'ok' } else { 'missing' }
    $cargoStatus = if ((Have cargo) -or (Test-Path "$env:USERPROFILE\.cargo\bin\cargo.exe")) { 'ok' } else { 'missing' }
    Write-Host @"
pi install plan (dry run: nothing is executed)

  role: client (the only role on Windows)
  ref:  $Ref
  dir:  $Dir

prerequisites:
  git:   $gitStatus (not auto-installed; if missing: winget install Git.Git)
  cargo: $cargoStatus (auto-installed via rustup-init if missing)

steps:
  1. check git, install rustup if cargo is missing
  2. clone or update $RepoUrl ($Ref) in $Dir
  3. cargo install --path crates\bin --locked --force

exit codes: 0 = done, 1 = error (missing git, dirty worktree, unknown ref,
git/cargo failure)
"@
    exit 0
}

if (-not (Have git)) {
    Fail 'git is not installed. Install it first: winget install Git.Git'
}

if (-not (Have cargo)) {
    $cargoExe = "$env:USERPROFILE\.cargo\bin\cargo.exe"
    if (-not (Test-Path $cargoExe)) {
        $arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
        $rustupInit = Join-Path $env:TEMP 'rustup-init.exe'
        Invoke-WebRequest -Uri "https://win.rustup.rs/$arch" -OutFile $rustupInit
        & $rustupInit -y
        if ($LASTEXITCODE -ne 0) { Fail "rustup-init failed with exit code $LASTEXITCODE" }
    }
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
}

if (-not (Test-Path (Join-Path $Dir '.git'))) {
    if ((Test-Path $Dir) -and (Get-ChildItem -Path $Dir -Force | Select-Object -First 1)) {
        Fail "$Dir exists, is not empty, and is not a git clone; choose another -Dir"
    }
    New-Item -ItemType Directory -Force $Dir | Out-Null
    git clone --branch $Ref $RepoUrl $Dir
    if ($LASTEXITCODE -ne 0) { Fail 'git clone failed' }
    Add-RoleExclude $Dir
} else {
    Add-RoleExclude $Dir
    $dirty = git -C $Dir status --porcelain
    if ($dirty) {
        Fail 'install directory has local changes. Commit/stash/remove them, or choose another -Dir'
    }
    git -C $Dir fetch --tags origin
    if ($LASTEXITCODE -ne 0) { Fail 'git fetch failed' }
    git -C $Dir show-ref --verify --quiet "refs/remotes/origin/$Ref"
    if ($LASTEXITCODE -eq 0) {
        git -C $Dir show-ref --verify --quiet "refs/heads/$Ref"
        if ($LASTEXITCODE -eq 0) {
            git -C $Dir checkout $Ref
        } else {
            git -C $Dir checkout -b $Ref --track "origin/$Ref"
        }
        if ($LASTEXITCODE -ne 0) { Fail 'git checkout failed' }
        git -C $Dir pull --ff-only
        if ($LASTEXITCODE -ne 0) { Fail 'git pull --ff-only failed' }
    } else {
        git -C $Dir show-ref --verify --quiet "refs/tags/$Ref"
        if ($LASTEXITCODE -ne 0) { Fail "ref not found as branch or tag: $Ref" }
        git -C $Dir checkout "refs/tags/$Ref"
        if ($LASTEXITCODE -ne 0) { Fail 'git checkout failed' }
    }
}

Set-Content -Path (Join-Path $Dir '.pi-role') -Value 'client'

cargo install --path (Join-Path $Dir 'crates\bin') --locked --force --target-dir (Join-Path $Dir 'target')
if ($LASTEXITCODE -ne 0) { Fail 'cargo install failed' }

Write-Host ''
Write-Host "pi installed to $env:USERPROFILE\.cargo\bin\pi.exe (on PATH via rustup)."
Write-Host "If 'pi' is not found, open a new PowerShell session."
Write-Host 'Next steps:  pi setup   then, inside your project:  pi init'
```

- [ ] **Step 3: DryRun smoke via -File and via the scriptblock pattern**

```powershell
pwsh -File scripts\install.ps1 -DryRun; echo "exit=$LASTEXITCODE"
& ([scriptblock]::Create((Get-Content -Raw scripts\install.ps1))) -DryRun
```

Expected: both print the "pi install plan (dry run…)" block with `role: client`, `git: ok`, `cargo: ok`; first reports `exit=0`. (The scriptblock invocation proves the remote-parameterized pattern parses.) Do NOT run without `-DryRun` — a real run clones and builds into `%LOCALAPPDATA%\pi\src`.

- [ ] **Step 4: PSScriptAnalyzer**

Install once: `Install-Module -Name PSScriptAnalyzer -Scope CurrentUser -Force`. Then:

```powershell
Invoke-ScriptAnalyzer -Path scripts\install.ps1
```

Expected: no output (zero findings). Fix and re-run until clean.

- [ ] **Step 5: Commit**

```bash
rtk git add scripts/install.ps1 && rtk git commit -m "feat(scripts): add install.ps1 Windows client installer"
```

---

### Task 4: `scripts/update.ps1`

**Files:**
- Create: `scripts/update.ps1`

**Interfaces:**
- Consumes: clone dir convention `%LOCALAPPDATA%\pi\src` and `.pi-role` exclude behavior from Task 3.
- Produces: nothing consumed by later tasks.

- [ ] **Step 1: Prepare a throwaway clone and verify the smoke fails**

```powershell
$t = Join-Path $env:TEMP 'pi-src-test'
if (Test-Path $t) { Remove-Item -Recurse -Force $t }
git clone --quiet C:\Users\Khmil\RustProjects\pi $t
Set-Content -Path (Join-Path $t '.pi-role') -Value 'client'
pwsh -File scripts\update.ps1 -Dir $t -DryRun
```

Expected: last command FAILs — file does not exist.

- [ ] **Step 2: Write `scripts/update.ps1`**

Full content:

```powershell
#Requires -Version 5.1
<#
pi client updater for Windows: pulls the existing branch clone and
rebuilds/reinstalls the pi CLI. Prerequisite provisioning lives in
install.ps1; this script only updates an existing install.

  & "$env:LOCALAPPDATA\pi\src\scripts\update.ps1"
  & "$env:LOCALAPPDATA\pi\src\scripts\update.ps1" -DryRun
#>
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '',
    Justification = 'installer talks to the interactive console')]
param(
    [string]$Dir = "$env:LOCALAPPDATA\pi\src",
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'

function Fail([string]$Message) {
    Write-Host "error: $Message" -ForegroundColor Red
    exit 1
}

function Have([string]$Name) {
    return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

function Add-RoleExclude([string]$CloneDir) {
    $infoDir = Join-Path $CloneDir '.git\info'
    if (-not (Test-Path $infoDir)) {
        New-Item -ItemType Directory -Force $infoDir | Out-Null
    }
    $exclude = Join-Path $infoDir 'exclude'
    if (-not (Test-Path $exclude) -or
        -not (Select-String -Path $exclude -Pattern '^\.pi-role$' -Quiet)) {
        Add-Content -Path $exclude -Value '.pi-role'
    }
}

if (-not (Test-Path (Join-Path $Dir '.git'))) {
    Fail "no git clone at $Dir; run install.ps1 first"
}

if ($DryRun) {
    $branch = git -C $Dir symbolic-ref --quiet --short HEAD
    if ($LASTEXITCODE -ne 0) { $branch = 'DETACHED' }
    Write-Host @"
pi update plan (dry run: nothing is executed)

  role:   client (the only role on Windows)
  dir:    $Dir
  branch: $branch (update requires a branch checkout; DETACHED means a pinned install)

steps:
  1. verify clean worktree and branch checkout
  2. git pull --ff-only
  3. cargo install --path crates\bin --locked --force

exit codes: 0 = done, 1 = error (pinned/detached checkout, dirty worktree,
non-ff pull, git/cargo failure)
"@
    exit 0
}

Add-RoleExclude $Dir

$dirty = git -C $Dir status --porcelain
if ($dirty) {
    Fail 'install directory has local changes. Commit/stash/remove them first.'
}

git -C $Dir symbolic-ref --quiet HEAD | Out-Null
if ($LASTEXITCODE -ne 0) {
    Fail 'This install is pinned (tag/detached checkout); rerun install.ps1 with -Ref <new-tag>'
}

git -C $Dir pull --ff-only
if ($LASTEXITCODE -ne 0) { Fail 'git pull --ff-only failed' }

if (-not (Have cargo)) {
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
}
if (-not (Have cargo)) { Fail 'cargo not found; run install.ps1 first' }

cargo install --path (Join-Path $Dir 'crates\bin') --locked --force --target-dir (Join-Path $Dir 'target')
if ($LASTEXITCODE -ne 0) { Fail 'cargo install failed' }

Write-Host ''
Write-Host 'pi client updated.'
```

- [ ] **Step 3: DryRun smoke against the throwaway clone**

(Shell state does not persist between tool invocations — re-define `$t` in every step.)

```powershell
$t = Join-Path $env:TEMP 'pi-src-test'
pwsh -File scripts\update.ps1 -Dir $t -DryRun; echo "exit=$LASTEXITCODE"
```

Expected: "pi update plan" block with a real branch name and `exit=0`.

- [ ] **Step 4: Dirty worktree error path (proves `.pi-role` exclusion)**

```powershell
$t = Join-Path $env:TEMP 'pi-src-test'
New-Item -ItemType File -Path (Join-Path $t 'junk.txt') | Out-Null
pwsh -File scripts\update.ps1 -Dir $t; echo "exit=$LASTEXITCODE"
Remove-Item (Join-Path $t 'junk.txt')
```

Expected: `error: install directory has local changes. Commit/stash/remove them first.` and `exit=1` (caused by `junk.txt`; `.pi-role` alone must not trigger it).

- [ ] **Step 5: Pinned/detached error path, then cleanup**

```powershell
$t = Join-Path $env:TEMP 'pi-src-test'
git -C $t checkout --detach --quiet
pwsh -File scripts\update.ps1 -Dir $t; echo "exit=$LASTEXITCODE"
Remove-Item -Recurse -Force $t
```

Expected: `error: This install is pinned (tag/detached checkout); rerun install.ps1 with -Ref <new-tag>` and `exit=1`. (Error paths exit before `git pull`/`cargo install`.)

- [ ] **Step 6: PSScriptAnalyzer**

```powershell
Invoke-ScriptAnalyzer -Path scripts\update.ps1
```

Expected: no output (zero findings).

- [ ] **Step 7: Commit**

```bash
rtk git add scripts/update.ps1 && rtk git commit -m "feat(scripts): add update.ps1 Windows client updater"
```

---

### Task 5: Version bump to 0.6.0

**Files:**
- Modify: `Cargo.toml:6` (`[workspace.package] version`)
- Modify: `Cargo.lock` (regenerated versions of workspace crates)

**Interfaces:**
- Consumes: nothing.
- Produces: workspace version `0.6.0` (README in Task 6 references v0.6 status).

- [ ] **Step 1: Bump the version**

In the root `Cargo.toml`, change line 6:

```toml
version = "0.6.0"
```

(was `version = "0.5.0"`).

- [ ] **Step 2: Regression — full workspace test run (also refreshes Cargo.lock)**

```bash
rtk cargo test --workspace
```

Expected: all tests pass, zero failures. `git status` afterwards shows `Cargo.toml` and `Cargo.lock` modified, nothing else.

- [ ] **Step 3: Commit**

```bash
rtk git add Cargo.toml Cargo.lock && rtk git commit -m "chore: bump workspace version to 0.6.0"
```

---

### Task 6: README updates

**Files:**
- Modify: `README.md` — status paragraph (lines 8–13), new section inserted immediately before `## Build And Install The Binary` (line 165), heading `## Quick Setup (v0.5)` (line 204)

**Interfaces:**
- Consumes: script paths and flags exactly as implemented in Tasks 1–4.
- Produces: nothing.

- [ ] **Step 1: Update the status paragraph**

Replace the current status paragraph (README.md lines 8–13):

```markdown
Status: v0.6 (Install scripts) — everything from v0.1–v0.5 (deploy/env/ingress/CI,
`pi logs`, `pi stats`, `pi start|stop|restart`, `pi rm`, `pi status`, `pi doctor`,
`pi agent status|logs`, one-command setup) plus one-command install and update
from source: `curl … | sh` on Linux/macOS and `irm … | iex` on Windows (see
"Install And Update Via Scripts"). Manual install from source remains as a
fallback (see "Build And Install The Binary" below).
```

- [ ] **Step 2: Insert the new section before `## Build And Install The Binary`**

````markdown
## Install And Update Via Scripts

The fastest path on Linux and macOS — the script asks whether to install the
agent (on the Raspberry Pi) or the client (on a developer machine):

```bash
curl -fsSL https://raw.githubusercontent.com/khmilevoi/rpi-deploy/master/scripts/install.sh | sh
```

Non-interactive (CI or provisioning):

```bash
curl -fsSL https://raw.githubusercontent.com/khmilevoi/rpi-deploy/master/scripts/install.sh | sh -s -- --agent
curl -fsSL https://raw.githubusercontent.com/khmilevoi/rpi-deploy/master/scripts/install.sh | sh -s -- --client
```

The agent role requires Docker to be installed beforehand. The script checks
this before cloning and building, never installs Docker itself, and prints the
manual command (`curl -fsSL https://get.docker.com | sh`) if it is missing.

On Windows (client only; building needs the Visual Studio Build Tools C++
workload):

```powershell
powershell -c "irm https://raw.githubusercontent.com/khmilevoi/rpi-deploy/master/scripts/install.ps1 | iex"
```

Parameterized run (for example a dry run, or pinning a tag):

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/khmilevoi/rpi-deploy/master/scripts/install.ps1))) -DryRun
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/khmilevoi/rpi-deploy/master/scripts/install.ps1))) -Ref v0.6.0
```

To update an existing install later (pull the branch clone, rebuild,
reinstall, and restart the agent daemon for the agent role):

```bash
sh "${XDG_DATA_HOME:-$HOME/.local/share}/pi/src/scripts/update.sh"
```

```powershell
& "$env:LOCALAPPDATA\pi\src\scripts\update.ps1"
```

Useful flags: `--dry-run` / `-DryRun` prints the resolved plan without
executing anything; `--ref <branch-or-tag>` / `-Ref` installs a specific ref
(tag installs are pinned — update refuses them until you rerun install with a
new tag); `--dir <path>` / `-Dir` overrides the clone directory (default
`~/.local/share/pi/src` or `%LOCALAPPDATA%\pi\src`).

Building from source takes several minutes on a Raspberry Pi (roughly ten on
smaller boards).
````

- [ ] **Step 3: Rename the Quick Setup heading**

Change `## Quick Setup (v0.5)` to `## Quick Setup` (the section is no longer version-specific).

- [ ] **Step 4: Verify the one-liners in README match the scripts**

Run:

```bash
rtk grep "install.sh | sh" README.md && rtk grep "install.ps1 | iex" README.md
```

Expected: both hits present, URLs exactly `https://raw.githubusercontent.com/khmilevoi/rpi-deploy/master/scripts/...`.

- [ ] **Step 5: Commit**

```bash
rtk git add README.md && rtk git commit -m "docs: README install/update via scripts, status v0.6"
```

---

## Manual Acceptance Matrix (post-merge, on real hardware)

Not part of the automated tasks — run on a real Pi / dev machines before tagging `v0.6.0` (spec §11):

- Fresh Pi without Docker: `curl … | sh` → role prompt → answer `1` → fail-fast exit `1` with Docker instructions; nothing cloned or built.
- Fresh Pi with Docker: `curl … | sh` → agent installed, `sudo /usr/local/bin/pi agent setup` ran, `pi doctor` has no Docker-related FAIL.
- Rerun `curl … | sh -s -- --agent` on the same Pi → clean pull, rebuild, reinstall.
- `scripts/update.sh` on the Pi → pull, rebuild, reinstall, `systemctl restart pi-agent` when active.
- Linux/macOS `--client` install and repeat install (idempotent via `cargo install --force`).
- macOS without build tools → Xcode Command Line Tools instruction, exit `1`.
- Windows `powershell -c "irm … | iex"` fresh + repeat; `update.ps1` on a branch clone; tag install via `-Ref` then `update.ps1` → pinned error.
