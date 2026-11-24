# syntax=docker/dockerfile:1
# Headless Linux nessa-server gateway with native Ghostty terminals (ADR 1417, #1418).
# Build from the repository root:
#
#   docker build -f docs/guides/headless-gateway/Dockerfile -t nessa-gateway-headless .
#
# Run with persistent data and optional mounted vault passphrase secret:
#
#   docker run -d --name nessa-gateway \
#     -v nessa-data:/data \
#     -v nessa-workspace:/workspace \
#     -v /path/to/vault-passphrase:/run/secrets/nessa_vault_passphrase:ro \
#     nessa-gateway-headless
FROM rust:1-bookworm AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends python3 xz-utils ca-certificates git \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN bash -lc 'eval "$(scripts/terminal/ghostty.sh)" && cargo build --release --locked -p nessa-server --bin nessa --features nessa-server/terminals'

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl bash jq \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --create-home --shell /bin/bash --uid 10001 nessa \
    && mkdir -p /data /workspace /opt/nessa/bin \
    && chown nessa:nessa /data /workspace
COPY --from=build /src/target/release/nessa /usr/local/bin/nessa
COPY docs/guides/headless-gateway/nessa-gateway-bootstrap /opt/nessa/bin/nessa-gateway-bootstrap
RUN chmod 0755 /opt/nessa/bin/nessa-gateway-bootstrap
USER nessa
ENV NESSA_DATA_DIR=/data \
    NESSA_STAGE=prod \
    NESSA_HOST=127.0.0.1 \
    NESSA_PORT=18789
WORKDIR /workspace
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${NESSA_PORT}/health" >/dev/null || exit 1
ENTRYPOINT ["sh", "-c", "if [ -e /run/secrets/nessa_vault_passphrase ] || [ -L /run/secrets/nessa_vault_passphrase ]; then exec nessa server --provision-local --vault-passphrase-file /run/secrets/nessa_vault_passphrase; else exec nessa server --provision-local; fi"]
