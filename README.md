# pi

Deploy tool for Raspberry Pi: `pi deploy` builds and brings up a docker-compose
project on Pi through a daemon agent (axum on unix-socket, SSH tunnel, SQLite).

- Spec: docs/superpowers/specs/2026-06-09-pi-deploy-tool-design.md
- Plan v0.1: docs/superpowers/plans/2026-06-09-pi-mvp-v0.1.md
- Agent installation (v0.1, manual): docs/install-agent-v0.1.md

Status: v0.1 (MVP) — `pi deploy` + `pi ls`. Secrets, ingress automation,
health-check and deploy queue — next versions (§23 spec).

## Dev

    cargo test --workspace
    cargo run -p pi -- agent run --config dev/agent.toml   # local agent (TCP)
    $env:PI_AGENT_URL = "http://127.0.0.1:7700"            # CLI via ssh (PowerShell)
