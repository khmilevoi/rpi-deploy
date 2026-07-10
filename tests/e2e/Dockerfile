# syntax=docker/dockerfile:1.7
FROM rust:1.88-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --locked -p pi && \
    install -D -m 0755 target/debug/rpi /out/rpi

FROM docker:28-cli AS docker_cli

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
      bash ca-certificates curl git libgcc-s1 openssh-client openssh-server passwd util-linux && \
    rm -rf /var/lib/apt/lists/*
COPY --from=docker_cli /usr/local/bin/docker /usr/local/bin/docker
COPY --from=docker_cli /usr/local/libexec/docker/cli-plugins/docker-compose \
  /usr/local/libexec/docker/cli-plugins/docker-compose
COPY --from=builder /out/rpi /usr/local/bin/rpi

RUN groupadd --system rpi-agent && \
    useradd --system --gid rpi-agent --no-create-home --shell /usr/sbin/nologin rpi-agent && \
    useradd --create-home --shell /bin/bash deploy && \
    passwd --delete deploy && \
    usermod --append --groups rpi-agent deploy && \
    install -d -m 0755 /run/sshd /opt/e2e && \
    docker --version && docker compose version && rpi --version

COPY tests/e2e /opt/e2e
RUN find /opt/e2e -name '*.sh' -exec sed -i 's/\r$//' {} + -exec chmod 0755 {} +
