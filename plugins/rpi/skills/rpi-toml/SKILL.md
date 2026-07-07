---
name: rpi-toml
description: Use when creating, editing, validating, reviewing, or troubleshooting rpi.toml files for rpi deployments, including schema 1 fields, project/source/build/ingress/healthcheck/env/timeouts sections, Docker Compose service and port mapping, public hostname ingress, worker services, and per-project deploy settings.
---

# Rpi TOML

## Overview

Use this skill for `rpi.toml`, the project-level deployment config read by `rpi deploy`, `rpi deploy --cancel`, `rpi secrets send`, and `rpi secrets ls`. Keep config advice aligned with `crates/bin/src/cli/rpitoml.rs` and `README.md`.

## Minimal Shape

Public web service:

```toml
schema = 1

[project]
name = "example-web"

[source]
repo = "git@github.com:owner/example-web.git"
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
timeout = "60s"

[secrets]
env = ".env"                     # optional, default ".env"
files = [                        # optional; recreated at the same paths on the Pi
  "certs/server.pem",
]
```

Worker, bot, or internal service without public HTTP ingress:

```toml
schema = 1

[project]
name = "example-worker"

[source]
repo = "git@github.com:owner/example-worker.git"
branch = "main"

[ingress]
service = "app"
port = 3000
```

## Fields

| Field | Required | Default | Notes |
| --- | --- | --- | --- |
| `schema` | yes | none | Must be `1`. |
| `project.name` | yes | none | Compose project name and agent state key. |
| `source.repo` | yes | none | Git URL fetched by the Pi. |
| `source.branch` | no | `"main"` | Default ref for `rpi deploy`. |
| `build.compose` | no | `"docker-compose.yml"` | Compose file inside the project repo. |
| `ingress.service` | yes | none | Compose service managed by rpi. |
| `ingress.port` | yes | none | Container port, not host port. |
| `ingress.hostname` | no | none | Public hostname for Cloudflare/manual ingress. |
| `healthcheck.path` | no | none | HTTP path; omitted means TCP probe. |
| `healthcheck.expect` | no | none | `"2xx"`, `"3xx"`, or exact 3-digit code. |
| `healthcheck.timeout` | no | `"60s"` | Duration string or bare seconds. |
| `secrets.env` | no | `".env"` | Local env file read by `rpi secrets send`. |
| `secrets.files` | no | none | Optional list of local secret file paths (certs, keys), forward-slash relative, `..` rejected; recreated verbatim on the Pi on every deploy. |
| `commands.<name>` | no | none | String (shell-word split, quotes only) or argv array. Name: `[a-z0-9][a-z0-9_-]*`. Registered at deploy, run via `rpi command`. |
| `timeouts.command` | no | `"600s"` | Budget for one `rpi command` run. |

Optional per-project stage timeouts:

```toml
[timeouts]
fetch = "3m"
build = "45m"
up = "2m"
```

Valid duration examples are `"60s"`, `"2m"`, and bare seconds such as `"120"`.

Optional one-off admin commands, run inside the service container with `rpi command`:

```toml
[commands]
create-invite = "node scripts/create-invite.js"
migrate = ["npx", "prisma", "migrate", "deploy"]
backup = "sh -c 'pg_dump mydb | gzip > /data/backup.gz'"
```

## Authoring Workflow

1. Identify the Compose service name and container port first.
2. Set `project.name` to a stable, unique deployment name; changing it creates a different deployed project state.
3. Set `source.repo` to a URL the Raspberry Pi can fetch, not just the developer machine.
4. Use `ingress.hostname` only when the service needs public HTTP routing.
5. Add `[secrets]` when the service needs an env file and/or secret files (certs, keys) delivered from the developer machine.
6. Add `[healthcheck]` when the service has an HTTP readiness endpoint; otherwise rely on the TCP probe.
7. Add `[timeouts]` only for project-specific overrides; prefer agent defaults for normal projects.

## Compose Compatibility

The agent writes an override mapping the allocated host port to `ingress.port`, roughly:

```yaml
services:
  web:
    ports:
      - "127.0.0.1:8000:3000"
```

Recommended Compose pattern:

```yaml
services:
  web:
    build:
      context: .
    expose:
      - "3000"
```

Avoid fixed host ports for the rpi-managed service:

```yaml
services:
  web:
    ports:
      - "127.0.0.1:3000:3000"
```

That can conflict with rpi's stable host port allocator.

For mutable runtime files, mount directories instead of individual files that may not exist in a fresh clone:

```yaml
services:
  app:
    environment:
      DATABASE_URL: file:///data/app.db
    volumes:
      - ./data:/data
```

## Validation Notes

`rpi.toml` is parsed by `crates/bin/src/cli/rpitoml.rs`:

- Unknown schema versions are rejected.
- Missing `[build]`, `[healthcheck]`, `[secrets]`, `[timeouts]`, and `[commands]` sections can fall back to defaults.
- `[env]` is rejected with a parse error pointing at `[secrets]`; it was replaced by `[secrets]` (`env` + `files`), a hard cutover with no fallback in `rpi.toml`.
- `[ingress]`, `[project]`, and `[source]` are required.
- Invalid healthcheck expectation values are rejected.
- Invalid duration strings in `[healthcheck].timeout` and `[timeouts]` are rejected.
- An empty `[commands]` section, an empty argv, bad command names, and unbalanced quotes in a string command are all rejected by `crates/bin/src/cli/rpitoml.rs`.

When editing the parser or adding fields, update:

- `crates/bin/src/cli/rpitoml.rs`
- `README.md`
- examples in this skill if the public config surface changes
