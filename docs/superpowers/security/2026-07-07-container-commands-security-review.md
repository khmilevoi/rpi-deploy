# Security Review: `[commands]` / `rpi command` Feature

**Scope:** PR branch `worktree-feat-container-commands` (base `123b82d`..`e9d5eae`) — adds a `[commands]` section to `rpi.toml` and an `rpi command` CLI subcommand that runs deploy-time-registered admin commands inside a project's service container via `docker compose exec -T`.

**Method:** Automated `/security-review` — an agent identified candidate findings against the full PR diff, then each finding was independently re-verified by a second agent against the false-positive filtering criteria (hard exclusions, precedents, confidence scoring). Only findings scoring ≥ 8/10 on re-verification are treated as qualifying.

## Result

**No qualifying vulnerabilities found** (confidence ≥ 8/10 threshold not met by any candidate finding).

## Analysis Summary

**Command injection** — `exec_tail` (`crates/infrastructure/src/docker.rs`) builds `["exec", "-T", service, ...argv]` and hands it to `tokio::process::Command::args(...)` — never a shell. No user input reaches shell interpretation; `sh -c '...'` only occurs if the project owner deliberately declares it as their own trusted command at deploy time, and HTTP-supplied extra args are appended as separate argv items after it, not string-interpolated into the script.

**Authorization / allowlist bypass** — Command and project names are validated against `^[a-z0-9][a-z0-9_-]*$` on both the deploy path (`create_deployment`) and the run path (`list_commands`/`run_command`). `RunCommand::lookup` does an exact map lookup against whatever is currently persisted for the project — there is no way to invoke a name that wasn't registered at deploy time, and the regex forbids the characters needed for path traversal or Unicode/case-confusion tricks.

**Path traversal** — The `{name}`/`{command}` path parameters are regex-constrained to ASCII lowercase/digit/`_`/`-`, never reaching a filesystem path.

**Flag injection via extra args** — `RunCommand::execute` always appends HTTP-supplied args strictly after the full deploy-trusted argv; a non-empty declared command (enforced both client- and server-side) always separates `service` from any HTTP-supplied token, so an appended arg can't be reinterpreted as a `docker compose` flag.

**SSRF** — No outbound requests are made based on attacker-controlled host/protocol; `docker compose exec` targets only a fixed, deploy-time-registered service.

**SQL injection** — `crates/infrastructure/src/repo.rs` persists `commands`/`command_timeout_secs` exclusively via `rusqlite` bound parameters; no string concatenation into SQL.

## Investigated and Ruled Out

### `GET /v1/projects/{name}/commands` returns full argv, unredacted

- **File:** `crates/bin/src/agent/http.rs` (`list_commands`), `crates/application/src/command.rs` (`RunCommand::list`)
- **Initial concern:** The endpoint echoes the verbatim declared argv for every registered command, unlike the codebase's existing `GET /v1/projects/{name}/env` pattern (`EnvKeysResponse`), which deliberately returns only key names, never values, to avoid leaking secret material. If an admin embeds a secret directly in a command line (e.g. a webhook URL with a token — the only way to parameterize a secret here, since `[commands]` values don't support shell variable expansion), this endpoint would echo it back to any caller who can reach the agent.
- **Why it was filtered out (verified confidence 3/10):** The agent binds only a Unix socket, reachable exclusively via an SSH-forwarded tunnel — the same access level required by every other sensitive endpoint on this router (deploy, env send, lifecycle, logs, project remove). That access level already grants strictly more than this endpoint leaks: the same actor can read the argv directly from the sqlite registry on disk, or simply invoke `POST /v1/projects/{name}/commands/{command}` to execute the command outright — a strictly more powerful primitive than reading its argv string. The literal command also originates from the project's own `rpi.toml` in the deployed repo, which the authoring admin already has access to. This is a legitimate inconsistency with the `EnvKeysResponse` pattern and a reasonable low-cost hardening item (redact argv in the list response, or document that `[commands]` values must never contain secrets), but it does not cross a new trust boundary introduced by this PR.

## Recommendation

No blocking action required before merge. Optionally, for defense-in-depth consistency with the existing `EnvKeysResponse` pattern, consider redacting or eliding argv values in `GET /v1/projects/{name}/commands`, or documenting explicitly that `[commands]` values must not contain secrets.
