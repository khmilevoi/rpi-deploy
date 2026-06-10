# pi

Деплой-тула для Raspberry Pi: `pi deploy` собирает и поднимает docker-compose
проект на Pi через демона-агента (axum на unix-сокете, SSH-туннель, SQLite).

- Спека: docs/superpowers/specs/2026-06-09-pi-deploy-tool-design.md
- План v0.1: docs/superpowers/plans/2026-06-09-pi-mvp-v0.1.md
- Установка агента (v0.1, вручную): docs/install-agent-v0.1.md

Статус: v0.1 (MVP) — `pi deploy` + `pi ls`. Секреты, ingress-автоматика,
health-check и очередь деплоев — следующие версии (§23 спеки).

## Dev

    cargo test --workspace
    cargo run -p pi -- agent run --config dev/agent.toml   # локальный агент (TCP)
    $env:PI_AGENT_URL = "http://127.0.0.1:7700"            # CLI мимо ssh (PowerShell)
