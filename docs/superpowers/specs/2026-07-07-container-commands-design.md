# Container Commands (`[commands]` + `rpi command`) — Design

Date: 2026-07-07
Status: approved

## Motivation

Operators need to run one-off application admin commands inside a deployed
container: DB migrations, seeding, creating invites, key rotation. Today the
only path is SSH to the Pi and a manual `docker compose exec`. We want a
first-class, declarative, auditable way to do this from the CLI.

## Decision

Server-side allowlist, registered at deploy time (approach A).

Commands are declared in a `[commands]` section of `rpi.toml`. At deploy the
CLI sends them to the agent as part of `ProjectDto`; the agent persists them
in project state. Invocation sends only a command **name** plus optional extra
argv items. The agent executes only commands it has stored — there is no
generic exec endpoint.

A general-purpose `rpi exec` is explicitly **out of scope**. Rationale:

- Privilege-wise it adds nothing (deploy already implies arbitrary code
  execution on the Pi via `docker compose build`/`up`), but it widens the API
  surface: an "execute arbitrary command" endpoint is far more dangerous than
  "deploy a repo" if agent API access ever leaks (the agent has a TCP dev
  mode; deploys are slow and noisy, exec is silent and instant).
- Client-side command resolution can desync from the deployed image (local
  `rpi.toml` references a script that does not exist in the running
  container). Deploy-time registration guarantees the command matches the
  deployed code.
- Incident debugging already has a path: SSH to the Pi and manual
  `docker compose exec`.

Scope is admin commands only: no deploy hooks, no scheduled/cron commands, no
interactive TTY sessions.

## TOML schema

```toml
[commands]
create-invite = "node scripts/create-invite.js"
migrate = ["npx", "prisma", "migrate", "deploy"]
backup = "sh -c 'pg_dump mydb | gzip > /data/backup.gz'"
```

- Value is either a **string** or an **array of strings**.
  - String form is split into argv **client-side at parse time** using
    shell-word rules (quoting works). No variable expansion, no pipes, no
    redirects — a shell is never implied. Users who need shell semantics
    write it explicitly (`sh -c '...'`), as in `backup` above.
  - Array form is taken as argv verbatim.
- Command names must match `[a-z0-9][a-z0-9_-]*`; violations are rpi.toml
  validation errors. CLI-reserved names are not needed: the name is a clap
  positional, not a subcommand, so `ls` or `deploy` are legal command names.
- The section is optional. An empty `[commands]` section, an empty command
  string, or an empty argv array is a validation error.
- Commands run in the `ingress.service` container. Per-command service
  override is deliberately omitted (YAGNI).

## Registration and wire protocol

- `ProjectDto` (crates/bin/src/proto.rs) gains
  `#[serde(default)] pub commands: BTreeMap<String, Vec<String>>`.
  - Old agent + new CLI: unknown field ignored — deploy still works.
  - New agent + old CLI: field defaults to empty map.
- `ProjectConfig` (domain) gains the corresponding `commands` field; the agent
  persists it in project state like every other config field. The deployed
  state is the single source of truth for what is invocable.

## CLI UX

```bash
rpi command create-invite                     # run a declared command
rpi command create-invite -- --email x@y.com  # extra args appended after --
rpi command                                   # no name: list deployed commands
```

- New clap subcommand `Command` in crates/bin/src/main.rs with an optional
  positional `name`, trailing args after `--`, and the standard flattened
  `ConnectOpts`.
- Project name is resolved from local `./rpi.toml` (`project.name`), same as
  `rpi deploy` / `rpi logs`.
- Extra args are appended to the declared argv as separate items. They can
  never replace the program — only extend its arguments.
- List mode queries the **agent** (deployed reality, not the local file). If
  the local `rpi.toml` declares commands missing from the agent's response,
  print a hint: undeployed `[commands]` changes exist — run `rpi deploy`.
- Command output streams live to the terminal (stdout+stderr interleaved,
  like deploy logs). The remote command's exit code becomes the CLI exit code
  (usable in scripts/CI).
- Unknown name → clear error listing available names.

## Agent API

Two routes in crates/bin/src/agent/http.rs, following existing patterns:

- `GET /v1/projects/{name}/commands` → declared commands (name + argv), used
  by list mode and error hints.
- `POST /v1/projects/{name}/commands/{command}` with body
  `{ "args": ["--email", "x@y.com"] }` → SSE stream using the existing
  `sse_log` pattern for output lines, plus a terminal event (modeled on
  `sse_finished`) carrying the exit code.

## Execution semantics

- Agent looks up the command in stored project state. Unknown project or
  command name → 404 (the latter includes available names).
- Final argv = declared argv + request args. No shell at any layer — the
  process is spawned via `Command::args`.
- New `exec` method on the domain runtime contract
  (crates/domain/src/contracts.rs), implemented by `DockerComposeRuntime`
  (crates/infrastructure/src/docker.rs) as:
  `docker compose -p <project> -f <file chain> exec -T <service> <argv...>`
  using the same `file_chain` (compose file + overrides) as build/up.
  `-T` = no TTY; stdin is closed.
- Output is streamed line-by-line into the SSE response (the `run_streamed`
  pattern). Non-UTF-8 output is converted lossily, as existing log streaming
  does.

### Timeout

- Agent-side default: **10 minutes** per command run.
- Per-project override: new `command` field in the existing `[timeouts]`
  section (`command = "30m"`), carried as `command_secs` in `TimeoutsDto`.
- On timeout the agent kills the process and emits an error line plus a
  non-zero exit event.

### Disconnect

If the SSE client disconnects (Ctrl+C), the agent best-effort kills the
`docker compose exec` process. Known limitation (documented): the process
*inside* the container may survive — standard `docker exec` behavior.

### Deploy interaction

Command runs take no deploy lock and are not blocked by deploys. If a
concurrent deploy restarts the container mid-command, the command fails; this
is operator responsibility. Documented, not prevented.

### Audit

The agent writes one line to its log per run: project, command name, args,
exit code, duration.

## Error handling summary

| Situation | Behavior |
| --- | --- |
| Project not deployed | 404; CLI: project not found on agent |
| Command name not declared | 404 + available names |
| Service not running | `docker compose exec` fails; stderr streamed; non-zero exit |
| Invalid name / empty argv / empty section | client-side rpi.toml validation error, before any request |
| Old agent + new CLI | unknown route → CLI: agent does not support commands, update rpi-agent |
| Project deployed before this feature | empty list + hint to deploy |
| Killed by timeout/signal | error message; CLI exits 1 |

## Testing

Follow existing test patterns per layer:

- `cli/rpitoml.rs`: string-form splitting incl. quotes, array form, invalid
  names, empty values, absent section.
- `proto.rs`: `ProjectDto` round-trip with and without `commands`
  (`#[serde(default)]` compatibility).
- `agent/http.rs`: router tests with the fake runtime — run → SSE events +
  exit event; 404 with names on unknown command; GET list.
- `infrastructure/docker.rs`: exec argument-shape test in the style of
  `compose_args_shape`.
- `application`: command-run use case on test doubles — happy path, timeout,
  exit-code propagation.

## Documentation to update

Per the rpi-toml skill rule (parser change → update all three):

- `README.md` — new `[commands]` section + `rpi command` usage.
- rpi-toml skill — schema fields and examples.
- rpi-cli skill — new subcommand.

## Out of scope

- `rpi exec` (generic remote exec), including a feature-flagged variant.
- Deploy hooks (pre/post-deploy commands).
- Scheduled/cron commands.
- Per-command service override.
- Interactive TTY / stdin forwarding.
