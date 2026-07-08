# Security Review: Per-command `service` override for `[commands]`

**Scope:** PR branch `worktree-per-command-service` (base `master`) — adds a `CommandSpec { argv, service: Option<String> }` domain type with hand-written backward-compatible serde, a `[commands.<name>]` table form in `rpi.toml` (`run` + optional `service`), and threads the per-command `service` through the protocol DTOs, registry persistence, and `RunCommand::execute`, which now execs into `spec.service.unwrap_or(config.service)` instead of always the ingress service.

**Method:** Automated `/security-review` — an agent identified candidate findings against the full PR diff, then each candidate was independently re-verified by a second agent against the false-positive filtering criteria (hard exclusions, precedents, confidence scoring). Only findings scoring ≥ 8/10 on re-verification are treated as qualifying.

## Result

**No qualifying vulnerabilities found** (confidence ≥ 8/10 threshold not met by any candidate finding).

## Analysis Summary

**Command / argument injection** — The new `service` value flows to `crates/infrastructure/src/docker.rs::exec` → `exec_tail(service, argv)`, which builds `["exec", "-T", service, ...argv]` and hands it to `Command::new("docker").args(...)` — a direct argv spawn, never a shell. No user input reaches shell interpretation, so classic shell-metacharacter injection is not in play. The one genuine mechanical change is analyzed and ruled out below.

**Trust boundary** — `service` is fixed by the deployer in the deployed config; it is not influenced by the command *invoker*. `CommandRunRequest` (`crates/bin/src/proto.rs`) carries only `args`, which `RunCommand::execute` appends strictly *after* the declared argv (i.e. after `exec -T <service> <argv[0]>`), so an invoker-supplied token lands as a positional argument to the in-container process and can never be reinterpreted as a `docker`/`exec` option. The deployer already holds arbitrary in-container execution by design (the `[commands]` argv is a deployer-declared allowlist) and verbatim control of their own `docker-compose.yml`, so nothing the `service` field enables crosses a new privilege boundary.

**Cross-tenant reach** — `compose_args` always pins `-p <project_name>` before the `exec` subcommand, and `-p`/`-f`/`--project-directory` are hardcoded ahead of `exec`, so the exec is scoped to the deployer's own compose project and the injected value cannot override top-level project/file selection.

**Deserialization** — `CommandSpec`'s hand-written untagged serde (`crates/domain/src/entities.rs`) maps a JSON array → `Argv` and a JSON object → `Full { argv, service }`. serde_json is not a code-execution deserializer, and empty argv is caught by deploy-time validation (`crates/bin/src/agent/http.rs`); a stray empty argv would at worst yield a no-op `docker compose exec` that errors harmlessly. No injection or type-confusion path.

**Persistence** — Registry writes remain via `rusqlite` bound parameters (`crates/infrastructure/src/repo.rs`); the `commands` column still stores `serde_json::to_string(&commands)`. No SQL string concatenation, no schema/trust change.

## Investigated and Ruled Out

### Unvalidated per-command `service` → `docker compose exec` argument injection

- **File:** validation in `crates/bin/src/agent/http.rs` (`create_deployment` loop over `config.commands`); sink in `crates/infrastructure/src/docker.rs` (`exec_tail`); plumbed via `crates/application/src/command.rs` (`RunCommand::execute`).
- **Initial concern:** Command names and the ingress `config.service` are validated against `^[a-z0-9][a-z0-9_-]*$` (`is_valid_name`), which forbids a leading `-`. This PR's new `spec.service` is validated **only for non-emptiness**, skipping that regex. Because `service` lands positionally as `["exec", "-T", service, ...argv]` and `docker compose exec` parses `[OPTIONS] SERVICE COMMAND...`, a value beginning with `-` (e.g. `service = "--privileged"`, `--user=root`, `--workdir=/`) is consumed as an *option* to `exec` rather than as the service name — a real regression versus `master`'s consistent `is_valid_name` gate.
- **Why it was filtered out (verified confidence 2/10):** The only actor who can set `service` is the deployer, who already has arbitrary in-container execution by design and uses their own `docker-compose.yml` verbatim (no compose sanitization exists in the workspace — grep confirms no stripping of `privileged`/`cap_add`/`user`/host bind mounts). A deployer who wants a privileged or root exec can simply write `privileged: true` / `user: root` directly into their own compose file, so `exec --privileged`/`--user=root` grants no capability the same principal couldn't already obtain. The lower-privileged command *invoker* cannot reach this at all (only `args` are exposed, and they append after the command, never as options), and `-p <project_name>` scopes the exec to the deployer's own project (no cross-tenant reach). This is a legitimate defense-in-depth / consistency gap, not a boundary-crossing vulnerability.

## Recommendation

No blocking action required before merge. As low-cost defense-in-depth and for consistency with the existing `is_valid_name` gate on command names and the ingress service, consider validating `spec.service` with the same `^[a-z0-9][a-z0-9_-]*$` regex on the agent (`create_deployment`) — real Docker Compose service names always pass, and it closes the `docker compose exec` argument-injection footgun cheaply. Mirroring the check client-side in `crates/bin/src/cli/rpitoml.rs::command_spec` is a nicety; the agent is the trust boundary.
