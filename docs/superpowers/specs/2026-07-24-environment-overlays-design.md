# Environment Overlays — Design

Date: 2026-07-24
Status: approved design; supersedes the idea sketch in
`docs/potential-features/environment-overlays.md`.

## Goal

Deploy production-like test and per-branch preview environments from the same
repository, with isolated runtime state, dedicated secrets, an optional
create-time hook, and an optional TTL after which a preview environment is
removed automatically.

## Architecture summary ("thin agent")

All configuration resolution — overlay loading, variable interpolation,
merging, and project-key derivation — happens in the CLI, which already owns
`rpi.toml` parsing. The agent receives an ordinary `ProjectConfig` whose
`name` is the derived environment key, plus a small `environment` metadata
block in `DeployRequest`. Agent-side additions are: environment entries in
the project registry, deploy-time guards, the `on_create` hook, `rpi env`
endpoints, and a background TTL reaper.

Isolation falls out of the existing "project name is the key" model: workdir
(`workdirs/<name>`), compose project (`-p <name>`), deploy keys, the
compose override file (`<name>.yml`), and the secrets bundle
(`PUT /v1/projects/{name}/secrets`) are all keyed by name, so a derived key
gets an isolated copy of each for free.

## Configuration model

### Files

- Base: `./rpi.toml` (unchanged; remains the production configuration).
- Overlay: `./rpi.<env>.toml`, where `<env>` matches `^[a-z][a-z0-9-]*$` and
  is not a reserved word. Reserved: `show`, `ls`, `destroy`, `reset-data`
  (current and likely future `rpi config` / `rpi env` subcommand names).
- `rpi deploy --env test` requires `rpi.test.toml` to exist. A missing
  overlay file is an error (listing the `rpi.*.toml` files found), never a
  silent deploy of the base configuration.

### Overlay schema

A new `RpiTomlOverlay` struct in `crates/bin/src/cli/rpitoml.rs`: the same
sections as `RpiToml`, but every field optional, plus one new section that is
valid **only** in overlays:

```toml
[environment]
ttl = "7d"            # optional; duration format shared with timeouts
on_create = "seed"    # optional; name of a command declared in [commands]
```

Rules:

- `schema` is forbidden in overlays (the schema version is a property of the
  base file).
- `[project].name` is forbidden in overlays (the key is CLI-derived).
- `[environment]` is forbidden in the base file.
- Unknown fields are an error, as in the base file today.

### Example

```toml
# rpi.test.toml
[source]
branch = "develop"

[ingress]
hostname = "test.example.com"

[secrets]
env = ".env.test"

[environment]
on_create = "seed"
```

```toml
# rpi.branch.toml
[source]
branch = "${BRANCH_NAME}"

[ingress]
hostname = "${RPI_ENV_SLUG}.preview.example.com"

[secrets]
env = ".env.preview"

[environment]
ttl = "7d"
on_create = "seed"
```

## Resolution pipeline (CLI)

1. Parse base and overlay separately (syntax + per-file structural
   validation).
2. Interpolate `${VAR}` in the overlay. Interpolation is allowed in exactly
   two fields: `source.branch` and `ingress.hostname`. A `${...}` reference
   anywhere else is an overlay parse error.
3. Typed merge (schema-aware, not generic TOML deep-merge):
   - a scalar set in the overlay replaces the base value;
   - tables (`[source]`, `[ingress]`, …) merge field-wise;
   - arrays (`[secrets].files`) replace wholesale, never concatenate;
   - `[commands]` is the one exception to field-wise table merging: if the
     overlay declares `[commands]` at all, the overlay's table replaces the
     base table as a whole (no per-command merging);
   - an explicit empty string (`hostname = ""`) resets an optional field to
     absent. This is the only deletion mechanism.
4. Validate the merged result through the same code path as today's
   `RpiToml::parse` (durations, healthcheck expect, secret paths, command
   argv, and — post-substitution — `[ingress].hostname` as an RFC-1123-style
   DNS name), plus environment-specific checks: `on_create` must name a
   command that exists in the merged `[commands]`; `ttl` must parse as a
   duration; the merged hostname, if present, must differ from the base
   file's hostname (an environment must override it or clear it with
   `hostname = ""` — see "Production-key protection" below).

## Variables and interpolation

- The `RPI_` prefix is a reserved namespace for tool-provided variables.
  User variables must not use it; `--vars RPI_X=...` is an error.
- v1 defines exactly two variables:
  - `BRANCH_NAME` — user-supplied via `--vars BRANCH_NAME=<value>`;
  - `RPI_ENV_SLUG` — derived by the CLI from `BRANCH_NAME`; cannot be set
    manually.
- `--vars KEY=VALUE` is repeatable and syntactically generic
  (names `[A-Z][A-Z0-9_]*`), but in v1 any key other than `BRANCH_NAME` is
  an error. Rationale: instance identity is `(env, RPI_ENV_SLUG)`; allowing
  arbitrary variables would let two deploys with the same branch but
  different values collide silently under one key.
- An unresolved variable reference is an error before any agent contact —
  never an empty substitution.
- An overlay is **parameterized** if it references any variable.
  `BRANCH_NAME` is required exactly when the overlay is parameterized;
  passing `--vars` to a non-parameterized overlay is an error.
- After substitution, values pass the full validation of their field
  (hostname rules, git-ref rules).

### Slug derivation

`RPI_ENV_SLUG` = `BRANCH_NAME` lowercased; every character outside
`[a-z0-9]` becomes `-`; runs of `-` collapse; leading/trailing `-` trimmed;
deterministically truncated to 30 characters (then trailing `-` trimmed
again). An empty result is an error. The 30-character cap keeps
`<slug>.preview.example.com` within the 63-character DNS label limit and
keeps compose project names manageable.

## Identity and isolation

### Key derivation

- Named environment: `<base>--<env>` → `myapp--test`.
- Parameterized environment: `<base>--<env>--<RPI_ENV_SLUG>` →
  `myapp--branch--feature-login`.

### Production-key protection (enforced on both sides)

- CLI: the environment key is always built by appending a `--<env>` suffix,
  and `[project].name` is forbidden in overlays — reaching the base key is
  syntactically impossible. New validation: `--` is forbidden inside a base
  `[project].name` (otherwise a base project named `myapp--test` would
  collide with an environment key). This is a deliberate breaking change for
  any existing project whose name contains `--`: the CLI rejects it with an
  error asking to rename the project.
- Agent: the registry records each project's kind — `base` or
  `environment` (plus base name, env name, slug). Deploying a base config
  into a key registered as an environment is rejected, and vice versa. This
  covers stale or modified CLIs sending the wrong key.
- Hostname: an environment's resolved `[ingress].hostname` must differ from
  its base project's hostname — otherwise the environment's first
  successful deploy would re-route the production hostname to the
  environment's own host port. The CLI enforces this at resolve time
  (comparing the merged hostname against the base file's, before the key
  even exists on the agent); the agent enforces it again at deploy time
  (comparing the incoming hostname against the registered base project's,
  by `environment.base`) with a 409, covering a stale or hand-crafted CLI
  that skips the local check.

### Secrets

Isolation is inherited from the existing per-name bundle storage.
`rpi secrets send --env test` reads the overlay-resolved `[secrets].env`
file (e.g. `.env.test`) and sends it to the derived key. A fresh environment
has no secrets until they are sent explicitly; the production bundle is
never copied.

## CLI surface

`--env <name>` and `--vars KEY=VALUE` are added to every command that
resolves the project from `./rpi.toml` today: `deploy` (including
`--cancel`), `secrets send`, `secrets ls`, `command`. Resolution is shared:
load base, apply overlay, derive the key, then run unchanged against the
derived name. Commands that take an explicit project name (`logs`, `stats`,
`start`/`stop`/`restart`, `rm`, `status`) are unchanged — the derived key
can be passed directly (visible in `rpi env ls` / `rpi config show`).

New commands:

- `rpi config show [--env <name>] [--vars ...]` — local only (no agent):
  prints the resolved configuration as TOML, including the derived project
  name and the `[environment]` block. `rpi.toml` contains paths, not secret
  values, so there is nothing to mask; secret file paths print as-is.
- `rpi env ls [--all]` — lists environments from the agent registry: key,
  base, env, slug, created, last deploy, TTL, time to expiry, status.
  Default scope is the current project (base name from `./rpi.toml`);
  `--all` lists every environment on the agent. Without an `rpi.toml` in the
  current directory, `--all` is required.
- `rpi env destroy <env> [--vars ...] [--yes]` — full teardown (see
  below). Interactive confirmation shows the derived key; `--yes` for CI.
- `rpi env reset-data <env> [--vars ...] [--yes]` — stops the stack,
  removes volumes, clears the `on_create_done` flag; the next deploy brings
  the stack up and re-runs `on_create`.

`rpi env destroy`/`reset-data` need only `./rpi.toml` (for the base name)
and the agent; they do not require a concurrent deploy context.

## Wire protocol and agent changes

### Protocol

`DeployRequest` gains an optional block:

```text
environment: {
  env: String,             # overlay name, e.g. "test", "branch"
  base: String,            # base project name, e.g. "myapp"
  slug: Option<String>,    # RPI_ENV_SLUG for parameterized envs
  ttl_seconds: Option<u64>,
  on_create: Option<String>,
}
```

`ProjectDto` is unchanged (`name` already carries the derived key). A
request without the block is treated as a base deploy — backward compatible
in both directions.

### Registry

Project entries are extended with: `kind` (base | environment), `base`,
`env`, `slug`, `created_at`, `last_success_at`, `ttl`, `on_create_done`.
The kind guard lives in `create_deployment`.

### `on_create` hook

After a fully successful deploy (including healthcheck), if the project is
an environment with `on_create` set and `on_create_done == false`, the agent
runs the named command through the existing `rpi command` execution path
(exec in the compose service). Success sets `on_create_done = true`.
Failure marks the deploy failed with an "on_create hook" stage; the flag
stays false and the next deploy retries the hook. The stack is not rolled
back, matching current healthcheck-failure behavior.

### Endpoints

- `GET /v1/environments[?base=<name>]` — list for `rpi env ls`.
- `DELETE /v1/environments/{key}` — full teardown.
- `POST /v1/environments/{key}/reset-data` — stop, remove volumes, clear
  `on_create_done`.

Both destructive endpoints reject when `{key}` is a base project, and when a
deploy for `{key}` is currently in progress.

### TTL reaper

A background task started during agent bootstrap (alongside the metrics
sampler). Interval configurable in `agent.toml` (default 1 hour). Each tick
scans the registry; an environment with a `ttl` whose
`last_success_at + ttl < now` is torn down via the same code path as
`DELETE`. TTL is sliding: every successful deploy updates
`last_success_at` and thus resets the clock. No TTL means no automatic
expiry. A key with a deploy in progress is skipped until the next tick. A
teardown failure is logged and does not stop the remaining teardowns.

### Teardown

`compose down` with volumes → ingress rule removal from the cloudflared
config **and DNS record deletion via the Cloudflare API** (DNS deletion is a
new capability; only upsert exists today) → workdir and override file →
secrets bundle → registry entry last. Every step treats "already absent" as
success. A failure at any step leaves the registry entry in place, so the
reaper or a repeated `env destroy` finishes the remainder later — no
orphaned state without a registry entry.

## Error handling

CLI (all before any agent contact):

- `--env <name>` with no matching overlay file → error listing found
  `rpi.*.toml` files.
- Forbidden fields (`schema`, `[project].name` in overlay, `[environment]`
  in base), unknown fields, `${...}` outside the two allowed fields → parse
  error naming the file and field.
- Unknown variable, `--vars` with an `RPI_` prefix, missing `BRANCH_NAME`
  for a parameterized overlay, `--vars` for a non-parameterized overlay,
  empty slug after normalization → resolution error.
- Post-merge validation failure → the message shows the resolved value and
  notes it came from the overlay merge.

Agent:

- A base (no `environment` block) deploy whose `project.name` contains `--`
  is rejected 400 before the registry is ever consulted — `--` is reserved
  for derived environment keys, so a base-into-environment-key collision
  can't reach the kind check at all. Kind mismatch (base vs environment)
  → 409 with a clear message fires only for a key that passes name
  validation yet is already registered as the other kind (e.g. a legacy
  registry entry predating this validation).
- destroy/reset-data during an in-progress deploy → 409; reaper skips until
  the next tick.

## Safety invariants

1. An overlay cannot target the production key: syntactically (CLI builds
   keys only by suffixing; `name` forbidden in overlays) and on the agent
   (kind guard).
2. Production secrets are never copied into an environment — every key has
   its own bundle, filled only by an explicit `secrets send --env`.
3. The reaper touches **only** registry entries with `kind = environment`
   and a `ttl` set. A base project can never be removed automatically,
   regardless of registry contents.
4. Interpolation is limited to two fields; substituted values pass full
   field validation; `RPI_*` is a closed namespace.
5. All destructive operations are explicit, with confirmation or `--yes`.

## Testing

Unit (CLI): overlay parsing (forbidden/unknown fields); typed merge (scalar
replacement, field-wise table merge, wholesale array replacement,
empty-string reset); interpolation (allow-list, unknown variable, `RPI_*`
namespace); slug derivation (normalization, truncation to 30, empty
result); key derivation; `ttl` parsing.

Unit/integration (agent): kind guards; `on_create` logic (first success
only; failure → retry on next deploy); reaper target selection (expired
ttl; skip in-progress; environments only); teardown ordering and
idempotency.

E2E (existing docker harness):

1. Base + `--env test`: two isolated stacks, separate secrets,
   `config show` reflects the resolution.
2. Branch preview: deploy with `BRANCH_NAME`; hostname carries the slug;
   `on_create` runs exactly once; a redeploy does not re-run it;
   `reset-data` makes the next deploy re-run it.
3. `env destroy`: stack, volumes, workdir, secrets, and registry entry are
   gone; a repeated destroy succeeds (idempotency).
4. TTL: an environment with a short ttl expires on its own; the base
   project and a no-ttl environment survive. The reaper interval is
   configurable in `agent.toml` to make this testable.

## Implementation phases (one spec, three phases)

1. **CLI resolution** — overlays, merge, interpolation, keys,
   `rpi config show`, `--env`/`--vars` flags. Useful standalone:
   environments deploy as ordinary projects (no metadata, no guards yet).
2. **Env-aware agent** — `environment` block in the protocol, registry
   kinds + guards, `on_create`, `rpi env` command group and endpoints,
   Cloudflare DNS record deletion.
3. **TTL + reaper.**

Documentation updates (per phase, as behavior lands): `docs/architecture/`
(deploy flow, overview, a new flow for environment lifecycle) and the
`rpi-toml` / `rpi-cli` skills.

## Non-goals (v1)

- Variables beyond `BRANCH_NAME` / `RPI_ENV_SLUG`, and interpolation outside
  `source.branch` and `ingress.hostname`.
- Hooks beyond `on_create` (e.g. `on_destroy`, `on_deploy`).
- Copying or anonymizing production data into environments.
- A general template syntax in `rpi.toml`.
- Environment-specific behavior in commands that take an explicit project
  name (they operate on derived keys as plain names).
