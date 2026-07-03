---
name: rpi-toml
description: Use when creating, editing, validating, reviewing, or troubleshooting rpi.toml files for rpi deployments, including schema 1 fields, project/source/build/ingress/healthcheck/env/timeouts sections, Docker Compose service and port mapping, public hostname ingress, worker services, and per-project deploy settings.
---

# Rpi TOML

## Overview

Use this skill for `rpi.toml`, the project-level deployment config read by `rpi deploy`, `rpi deploy --cancel`, `rpi env send`, and `rpi env ls`. Keep config advice aligned with `crates/bin/src/cli/rpitoml.rs` and `README.md`.

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

[env]
file = ".env"
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
| `env.file` | no | `".env"` | Local file read by `rpi env send`. |

Optional per-project stage timeouts:

```toml
[timeouts]
fetch = "3m"
build = "45m"
up = "2m"
```

Valid duration examples are `"60s"`, `"2m"`, and bare seconds such as `"120"`.

## Authoring Workflow

1. Identify the Compose service name and container port first.
2. Set `project.name` to a stable, unique deployment name; changing it creates a different deployed project state.
3. Set `source.repo` to a URL the Raspberry Pi can fetch, not just the developer machine.
4. Use `ingress.hostname` only when the service needs public HTTP routing.
5. Add `[env]` when the service needs secrets from a local env file.
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
- Missing `[build]`, `[healthcheck]`, `[env]`, and `[timeouts]` sections can fall back to defaults.
- `[ingress]`, `[project]`, and `[source]` are required.
- Invalid healthcheck expectation values are rejected.
- Invalid duration strings in `[healthcheck].timeout` and `[timeouts]` are rejected.

When editing the parser or adding fields, update:

- `crates/bin/src/cli/rpitoml.rs`
- `README.md`
- examples in this skill if the public config surface changes
