# Per-command service override for `[commands]`

Date: 2026-07-08

## Problem

`rpi command <name>` always executes the deployed `[commands]` entry inside the
**ingress service** container. `RunCommand::execute` hardcodes the exec target as
`registered.config.service` (`crates/application/src/command.rs`), and the
infrastructure layer runs `docker compose exec -T <service> <argv>` against the
already-running stack (`crates/infrastructure/src/docker.rs`).

For multi-service stacks this is wrong. Example: a project whose
`ingress.service = "client"` is a plain `nginx:alpine` image (no Node, no app
env vars), while the command it needs to run — e.g. `create-invite` — must run
in the `server` container that has Node and the required environment
(`VALKEY_URL`, `PUBLIC_APP_URL`). There is currently no way to point a command
at any service other than the ingress one.

Because `docker compose exec` targets the whole running stack, the `server`
container is already up and reachable at command time. The only missing piece is
the ability to name a different service per command.

## Goal

Allow a `[commands]` entry to declare which compose service it runs in, defaulting
to `ingress.service` when unspecified. Preserve full backward compatibility of the
persisted registry and the CLI↔agent protocol.

Non-goals: validating the service name against the compose file (kept consistent
with the existing un-validated `ingress.service`); running commands against
stopped/one-off services; arbitrary argv (the allowlist model is unchanged).

## TOML schema

The existing string and array forms are unchanged and continue to mean "run in
`ingress.service`":

```toml
[commands]
migrate = "npx prisma migrate deploy"
seed    = ["node", "seed.js"]
```

A new **table form** pins the service:

```toml
[commands.create-invite]
run     = "node dist/scripts/create-invite.cjs"   # string OR array; same rules as the shorthand forms
service = "server"                                 # optional; omitted => ingress.service
```

`run` accepts the same two shapes as the shorthand value (shell-word string split
with `shlex`, or explicit argv array) and is validated identically (non-empty, no
empty items, balanced quotes). `service`, if present, must be a non-empty string;
empty string is rejected at parse time.

### Parsing (`crates/bin/src/cli/rpitoml.rs`)

`CommandValue` gains a third variant for the table form:

```rust
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CommandValue {
    Line(String),
    Argv(Vec<String>),
    Table { run: CommandRun, #[serde(default)] service: Option<String> },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CommandRun {
    Line(String),
    Argv(Vec<String>),
}
```

`command_argv` is generalized to resolve a `CommandValue`/`CommandRun` to
`(argv, service)`. Validation in `RpiToml::parse` gains: `service` present ⇒
non-empty. `to_project_config` produces `CommandSpec { argv, service }`.

Note on untagged ordering: serde tries variants top-to-bottom. A TOML table maps
to neither `Line` (string) nor `Argv` (array), so it falls through to `Table`.
A bare string/array never matches `Table`. Ordering is therefore safe.

## Domain model (`crates/domain/src/entities.rs`)

`ProjectConfig.commands` changes type:

```rust
// before: BTreeMap<String, Vec<String>>
pub commands: BTreeMap<String, CommandSpec>,
```

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// Declared argv (declared line/array plus nothing until invocation-time extra args).
    pub argv: Vec<String>,
    /// Compose service to exec into. None => ingress service (ProjectConfig.service).
    pub service: Option<String>,
}
```

### Backward-compatible serialization (Option A)

`CommandSpec` gets hand-written `Serialize`/`Deserialize` so that:

- **Deserialize** accepts *both* the legacy bare-array shape `["node","seed.js"]`
  and the new struct shape `{"argv":[...],"service":"server"}`. Legacy rows and
  legacy-CLI payloads decode with `service = None`.
- **Serialize** emits the **bare array** when `service` is `None`, and the struct
  form only when `service` is `Some`. Commands without a pinned service are thus
  byte-identical on the wire and on disk to today's format.

This keeps:
- existing SQLite rows (`commands TEXT`, JSON via `serde_json`) readable after an
  agent upgrade — no data loss, no migration needed;
- the CLI↔agent protocol compatible in both directions for service-less commands;
  a service-pinned command sent to an old agent degrades predictably (old agent
  can't parse the struct and ignores/rejects it — documented, not silently wrong).

Implementation approach: an untagged helper enum
`enum Repr { Argv(Vec<String>), Full { argv, service } }` used only for
(de)serialization, with `From`/`Into` between it and `CommandSpec`, or a manual
`Deserialize` via `deserialize_any`. Either is acceptable; the untagged-helper
route is simplest and is the plan's default.

## Protocol DTOs (`crates/bin/src/proto.rs`)

The two DTOs currently typed `BTreeMap<String, Vec<String>>` (deploy request
carrying commands, and the `rpi command` list response) change to carry
`CommandSpec`/an equivalent DTO. The list response additionally surfaces the
resolved service so `rpi command` (list mode) can show where each command runs.
Wire compatibility is preserved by the same Option-A serialization.

## Persistence (`crates/infrastructure/src/repo.rs`, `sqlite.rs`)

No schema migration. The `commands TEXT` column already stores
`serde_json::to_string(&config.commands)`; with Option-A serialization the JSON
for service-less commands is unchanged, and service-pinned commands serialize to
the struct shape within the same column. `serde_json::from_str(...).unwrap_or_default()`
on read continues to work for both old and new rows.

## Execution (`crates/application/src/command.rs`)

`lookup` returns the `CommandSpec` (not just argv). `execute` resolves the target
service:

```rust
let service = spec.service.as_deref().unwrap_or(&registered.config.service);
// ... argv = spec.argv + extra_args ...
self.runtime.exec(&stack, service, &argv, log)
```

`list` returns `BTreeMap<String, CommandSpec>` (or a view including the resolved
service) so the CLI can display the target.

## Error handling

- Unknown command / unknown project: unchanged (`DomainError::NotFound`).
- Bad service name (typo, not in compose): surfaces at exec time as docker's
  error via the existing streamed-output path — same behavior as a bad
  `ingress.service` today. No new upfront validation.
- Empty `service` string in `rpi.toml`: rejected at parse time with a clear
  message, consistent with the other `[commands]` validations.

## Testing

- `rpitoml.rs`: table form parses (string `run` and array `run`); `service`
  omitted ⇒ `None`; empty `service` rejected; shorthand forms still parse to
  `service = None`; untagged ordering doesn't misroute string/array.
- `entities.rs` (or a serde test module): `CommandSpec` round-trips; legacy bare
  array JSON deserializes to `service = None`; `None` serializes back to a bare
  array; `Some` serializes to the struct shape.
- `repo.rs`: upsert/get round-trips a service-pinned command and a service-less
  command; a pre-seeded legacy-shape row still loads.
- `command.rs`: `execute` targets `spec.service` when set, and falls back to
  `ingress.service` when `None`; extra args still appended; exit code and timeout
  behavior unchanged.
- Workspace: `cargo test -p pi` then `cargo test --workspace`.

## Documentation

- `README.md`: document the table form and the `service` key.
- `.claude/skills/rpi-cli` and `rpi-toml` skills: add the per-command service form
  to the `[commands]` sections and the "Running Admin Commands" notes.
