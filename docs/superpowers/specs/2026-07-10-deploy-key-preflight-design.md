# Deploy key preflight — first deploy without the failure

Date: 2026-07-10
Status: approved

## Context

The first deploy of any project with an SSH repo always fails: the agent
generates a per-project deploy key inside the `fetch` stage
(`GitSource::ensure_key`), prints the pubkey as one log line buried in the
deploy stream, and the clone then dies with an opaque ssh error. The user has
to dig the key out of the logs, add it to GitHub → Deploy keys by hand, and
re-run the deploy. Goal: the first deploy either succeeds with zero manual
steps (GitHub + local `gh`) or pauses with a clear, pretty instruction and
continues by itself once the key is added.

## Scope decisions (settled)

- **CLI-orchestrated preflight.** Before creating the deployment, the CLI asks
  the agent to verify repo access (`git ls-remote` with the project's deploy
  key). The agent stays stateless — no new deployment states, no agent-side
  GitHub calls.
- **Auto-registration via local `gh`.** If the repo is on github.com and `gh`
  is installed and logged in, the CLI registers the pubkey as a **read-only**
  deploy key through `gh api` and re-checks. The GitHub token never leaves the
  user's machine; the private key never leaves the Pi.
- **Manual fallback waits and continues.** No `gh` (missing, logged out, API
  call failed) or a non-GitHub host → pretty box with the pubkey and
  instructions, then poll the access check every 5 s (cap 10 min, Ctrl+C
  aborts). The moment access works the deploy continues — no re-run.
- **SSH repos only.** HTTPS/local repos skip the preflight entirely (no key
  needed today; unchanged).
- **Every deploy runs the check.** ls-remote costs ~1–2 s next to a docker
  build and also catches revoked keys. The pane collapses to a one-liner so
  repeat deploys stay quiet.
- **Old agents degrade silently.** 404 from the check route → skip the
  preflight, behave exactly as today. The existing pubkey hint lines in the
  `fetch` stage stay as the safety net for that path.
- **Non-interactive (no TTY): no polling.** Print the key + instructions as
  plain lines and exit 1 with a clear "action required" message.
- `rpi deploy --no-gh-key` disables auto-registration (straight to the box).
  Registration itself needs no extra confirmation — it announces itself with
  a `registering read-only deploy key via gh…` line.

## Agent API

New route in `agent/http.rs`:

```
POST /v1/projects/{name}/source/check
{ "repo": "git@github.com:owner/repo.git" }
```

Response (DTOs in `proto.rs`):

```
{ "ok": true }
{ "ok": false,
  "pubkey": "ssh-ed25519 AAAA… pi-deploy-myapp",
  "error": "Permission denied (publickey)" }
```

- Generates the project's deploy key first if missing (same path and shape as
  today: `keys/{name}/id_ed25519`, ed25519, comment `pi-deploy-{name}`).
- Runs `git ls-remote <repo>` with the key via the existing
  `git_ssh_command`, ~30 s timeout.
- **Any** ls-remote failure maps to `ok: false` — GitHub answers
  `Repository not found` (not `Permission denied`) for private repos the key
  can't see, so no auth-vs-other classification is attempted. A trimmed stderr
  excerpt travels in `error` and is shown in the box so unrelated causes
  (typo'd URL, DNS) stay visible.
- Non-SSH repo in the body → `ok: true` immediately (defensive; the CLI
  doesn't call it for those).
- Stateless: touches only the key directory, never deployment state. Same
  exposure as the rest of the API (reachable through the SSH tunnel only).

## Domain / infrastructure

- `Source` contract (`domain/contracts.rs`) gains
  `async fn check_access(&self, project: &str, repo: &str) -> Result<SourceAccess, DomainError>`
  with `enum SourceAccess { Ok, Denied { pubkey: String, error: String } }`.
- `GitSource` implements it: key generation moves to a log-free helper shared
  with `fetch`; `fetch` keeps emitting today's hint lines when it generates a
  key (safety net for old-CLI / old-agent mixes).

## CLI

New module `crates/bin/src/cli/sourcekey.rs`; the flow hooks into
`commands::deploy` between the version check and `POST /v1/deployments`, only
when `rpi.toml`'s repo is SSH (`git@…` / `ssh://…`).

Helpers:

- `parse_github_repo(url) -> Option<(owner, repo)>` — accepts
  `git@github.com:owner/repo(.git)` and `ssh://git@github.com/owner/repo(.git)`;
  anything else (incl. GHES) → `None` → manual path.
- gh availability: `gh auth token` (fast, no network). Missing binary → silent
  manual fallback; logged out → muted hint
  `gh is not logged in (run: gh auth login) — add the key manually below`,
  then the box.
- `gh_register`: `gh api repos/{owner}/{repo}/keys -f title="pi-deploy-{project}"
  -f key="<pubkey>" -F read_only=true`. Any failure (no admin rights, key
  already in use, network) → muted hint with gh's stderr, manual fallback.
  Success → one re-check; if that still fails, manual fallback.

### UX (interactive)

Rendered with the existing `LogPane` primitives so every outcome collapses to
a one-line step, consistent with the stage panes:

- access already ok → pane `source access` for the check duration →
  `✓ source access (1.2s)`.
- gh path → `✓ deploy key registered via gh (3.4s)`.
- manual path → box + polling line stay on screen while the user adds the key;
  on success the pane clears and leaves `✓ deploy key added (1m 23s)`.

Manual box:

```
╭─ deploy key needed ────────────────────────────────────────╮
│ The Pi can't read git@github.com:khmil/myapp.git yet.      │
│ Add this read-only deploy key to the repository:           │
│                                                            │
│   ssh-ed25519 AAAAC3Nza… pi-deploy-myapp                   │
│                                                            │
│ → https://github.com/khmil/myapp/settings/keys/new         │
│   (check nothing extra: read-only is the default)          │
│                                                            │
│ agent said: Permission denied (publickey)                  │
╰────────────────────────────────────────────────────────────╯
waiting for access… (checking every 5s, Ctrl+C to abort)
```

Non-GitHub repos: the URL line becomes
`add it as a read-only deploy key in your git hosting`.

Poll timeout (10 min) or Ctrl+C → red one-liner, exit 1. Non-interactive mode
still attempts gh auto-registration (CI with a logged-in gh works end to end);
only the manual path changes — the same content prints as plain lines (no
clearing, no polling) and the deploy exits 1 immediately.

### Compatibility

| CLI \ agent | old agent | new agent |
|---|---|---|
| old CLI | today | never calls the route — today |
| new CLI | 404 → preflight skipped, today's behaviour (fetch-stage hint) | preflight |

## Security

- Private key: generated and stored on the Pi only (unchanged).
- GitHub token: read and used by local `gh` only; never sent to the agent.
- Deploy key registered with `read_only: true` — push access is never granted.
- Pubkey is public material; displaying and transmitting it is safe.
- Per-project keys (unchanged) match GitHub's one-repo-per-deploy-key rule.

## Testing

- `cli/sourcekey`: `parse_github_repo` (both forms, `.git` optional,
  negatives); `gh api` argument construction; preflight gating (https/ssh);
  box rendering incl. the non-GitHub variant; non-interactive output.
- `agent/http`: route test with a fake `Source` — ok and denied bodies, exact
  JSON shape; unknown project name still works (key dir is created on demand).
- `GitSource::check_access`: integration — local file repo → `Ok`;
  nonexistent path → `Denied` with non-empty `error` and a generated pubkey.
- CLI flow: 404 from the route → deploy proceeds as today (skip logic unit
  test on the decision function).

## Non-goals

- GitLab/Gitea/GHES API automation (manual box covers them).
- Agent-side GitHub API calls or tokens on the Pi.
- New deployment states (waiting/paused) or scheduler changes.
- Key rotation/removal commands (`cleanup` already deletes the key dir with
  the project).
- Provisioning at `rpi init` time (can layer on later; preflight already
  guarantees the outcome).
- HTTPS-repo credential handling.
