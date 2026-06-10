# Установка pi-agent v0.1 (вручную)

Автоматический `pi agent setup` появится в v0.5 (§13 спеки). Для v0.1 — руками.

## 0. Требования на Pi

- Raspberry Pi OS (64-bit), docker + docker compose plugin, git:
  `docker --version && docker compose version && git --version`

## 1. Сборка бинаря

Вариант А — прямо на Pi (просто, медленно):

    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
    git clone <этот-репозиторий> && cd pi
    cargo build --release            # бинарь в target/release/pi

Вариант Б — кросс-сборка с dev-машины:

    cargo install cross
    cross build --release --target aarch64-unknown-linux-gnu
    scp target/aarch64-unknown-linux-gnu/release/pi pi@<host>:/tmp/pi

## 2. Установка на Pi

    sudo install -m 755 <путь-к-бинарю>/pi /usr/local/bin/pi

    # системный юзер агента (§13)
    sudo useradd --system --no-create-home --shell /usr/sbin/nologin pi-agent || true
    sudo usermod -aG docker pi-agent
    # логин-юзер (через которого ходит ssh) — доступ к сокету (§9)
    sudo usermod -aG pi-agent "$USER"

    # каталоги (§17)
    sudo mkdir -p /var/lib/pi /etc/pi
    sudo chown -R pi-agent:pi-agent /var/lib/pi

    sudo tee /etc/pi/agent.toml >/dev/null <<'EOF'
    data_dir = "/var/lib/pi"
    socket = "/run/pi/agent.sock"
    # port_min = 8000
    # port_max = 8999
    EOF

    sudo tee /etc/systemd/system/pi-agent.service >/dev/null <<'EOF'
    [Unit]
    Description=pi deploy agent
    After=network-online.target docker.service
    Wants=network-online.target

    [Service]
    User=pi-agent
    Group=pi-agent
    ExecStart=/usr/local/bin/pi agent run --config /etc/pi/agent.toml
    RuntimeDirectory=pi
    RuntimeDirectoryMode=0750
    Restart=on-failure

    [Install]
    WantedBy=multi-user.target
    EOF

    sudo systemctl daemon-reload
    sudo systemctl enable --now pi-agent
    systemctl status pi-agent          # active (running)
    journalctl -u pi-agent -f          # логи агента (§14)

Перелогиньтесь (группа pi-agent применяется к новой сессии).

## 3. Клиент (dev-машина)

`~/.config/pi/config.toml` (Windows: `%APPDATA%\pi\config.toml`):

    default = "home"

    [servers.home]
    host = "192.168.1.50"
    user = "pi"
    key = "~/.ssh/id_ed25519"

Проверка: `ssh -i ~/.ssh/id_ed25519 pi@192.168.1.50 true` должна проходить без пароля.

## 4. Проект

В корень репозитория проекта — `pi.toml` (схема §12):

    schema = 1

    [project]
    name = "rateme"

    [source]
    repo = "git@github.com:isskelo/rateme.git"
    branch = "main"

    [build]
    compose = "docker-compose.yml"

    [ingress]
    hostname = "rateme.isskelo.com"
    service = "web"
    port = 3000

## 5. Ручные шаги v0.1 (уйдут в v0.2+)

- **Секреты:** после первого деплоя положить `.env` в
  `/var/lib/pi/workdirs/<project>/.env` (владелец pi-agent, chmod 600) и
  передеплоить. `pi env send` появится в v0.2.
- **Cloudflare Tunnel:** в config.yml туннеля добавить правило
  `hostname: rateme.isskelo.com -> service: http://127.0.0.1:<host-порт>`
  (host-порт показывает `pi ls`; он стабилен — правится один раз), перезапустить
  cloudflared. Автоматика — v0.2.
- **Приватные GitHub-репо:** первый деплой напечатает public deploy-key —
  добавить его в GitHub -> repo Settings -> Deploy keys (read-only) и
  перезапустить деплой (§15.1).

## 6. Приёмка MVP (e2e чек-лист, критерий §23 v0.1)

1. `pi ls` с dev-машины → `no projects deployed yet` (туннель и API работают).
2. `pi deploy` из репозитория проекта → логи clone/build/up стримятся,
   финал `deploy finished: success`.
3. На Pi: `curl http://127.0.0.1:<host-порт>` отвечает приложением;
   `docker compose -p <project> ps` — сервисы running.
4. Повторный `pi deploy` (без изменений) — идемпотентно success.
5. `pi deploy --ref <старый-sha>` — откат на конкретный коммит работает.
6. `pi deploy` при выключенном docker → `failed`, причина видна в логах CLI.
7. `sudo systemctl restart pi-agent` → `pi ls` снова работает; история
   деплоев на месте (SQLite пережил рестарт).
