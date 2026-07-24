# Environment overlays (idea)

> Superseded by the approved design and implementation:
> [`docs/superpowers/specs/2026-07-24-environment-overlays-design.md`](../superpowers/specs/2026-07-24-environment-overlays-design.md)
> (see `docs/architecture/flows/environments.md` for how it works). This
> page is kept for historical context only.

Status: potential feature; not an implementation specification.

## Goal

Deploy a production-like test or preview environment from the same repository,
with isolated runtime state, dedicated secrets, and test data.

## Configuration model

The base `rpi.toml` remains the production-like configuration. An optional
environment file is an overlay: it changes only the fields that differ from
the base configuration.

```text
myapp/
├── rpi.toml
├── rpi.test.toml
└── rpi.branch.toml
```

```toml
# rpi.toml
schema = 1

[project]
name = "myapp"

[source]
repo = "git@github.com:acme/myapp.git"
branch = "main"

[build]
compose = "docker-compose.yml"

[ingress]
hostname = "app.example.com"
service = "web"
port = 3000

[healthcheck]
path = "/health"
expect = "200"

[secrets]
env = ".env"
```

```toml
# rpi.test.toml
[source]
branch = "develop"

[ingress]
hostname = "test.example.com"

[secrets]
env = ".env.test"

[environment]
seed_command = "npm run db:seed"
```

```toml
# rpi.branch.toml
[source]
branch = "${BRANCH_NAME}"

[ingress]
hostname = "${ENV_SLUG}.preview.example.com"

[secrets]
env = ".env.preview"

[environment]
ttl = "7d"
seed_command = "npm run db:seed"
```

## Proposed commands

```bash
rpi deploy --env test
rpi deploy --env branch --vars BRANCH_NAME=feature/login
rpi config show --env test
rpi env reset-data test
rpi env destroy branch --vars BRANCH_NAME=feature/login
```

## Intended behaviour

- `rpi deploy --env <name>` loads `rpi.toml` and overlays
  `rpi.<name>.toml` when it exists.
- Environment overlays inherit unspecified values from `rpi.toml`.
- Deployments are isolated by a CLI-derived project key. For example,
  `myapp` plus `test` becomes `myapp--test`.
- The `branch` environment uses `rpi.branch.toml` for every branch. The user
  supplies `BRANCH_NAME`; the CLI derives a filesystem-, Compose-, and
  hostname-safe `ENV_SLUG` (for example, `feature/login` becomes
  `feature-login`).
- `rpi config show` displays the resolved configuration and masks secrets.
- Test-data initialization runs when an environment is first created, not on
  every deploy. `rpi env reset-data` is an explicit destructive operation.
- A branch environment may have a TTL. Expiration removes its runtime,
  volumes, ingress route, and environment metadata.

## Safety boundaries

- An environment overlay must not be able to target the base production
  project key.
- Production secret files must never be copied to test or preview
  environments.
- Test data must be generated or imported through an explicit anonymisation
  process; production personal data is not a default source.
- Variable interpolation needs a narrow, documented allow-list of fields and
  strict value validation.

## Open questions

- Whether `seed_command` belongs in `rpi.toml`, or should reuse/extend the
  existing project command mechanism.
- Exact deep-merge semantics for TOML tables, arrays, and deletion of a base
  value.
- Whether a first release needs persisted environment metadata and TTL, or
  should start with deploy/config resolution only.
