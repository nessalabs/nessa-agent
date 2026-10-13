# syntax=docker/dockerfile:1
# An example image for a paired machine that serves one gateway, and so one
# organization, from a container. How to run it, grant it and replace it is in
# docs/guides/local-auth.md, on a paired machine in a container. Build it from
# the repository root:
#
#   docker build -f docs/guides/env-worker/Dockerfile -t nessa-env-worker .
#
# It holds `nessa` alone. The agents the gateway may run here, and the
# config.json naming them, are yours to add on top of it.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p nessa-server --bin nessa

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --create-home --uid 10001 nessa \
    && mkdir /data /workspace \
    && chown nessa:nessa /data /workspace
COPY --from=build /src/target/release/nessa /usr/local/bin/nessa
USER nessa
# The pairing, its grants, the lease ledger and config.json live here: one
# volume per container, never shared by two at once.
ENV NESSA_DATA_DIR=/data NESSA_STAGE=prod
WORKDIR /workspace
# The same command on every start: enrolls with the code only the first time,
# then dials. The drain deadline stays below the container's stop grace.
ENTRYPOINT ["sh", "-c", "exec nessa env dial \"$NESSA_GATEWAY\" --enroll-code-file /run/secrets/nessa_enroll_code --drain-deadline \"${NESSA_DRAIN_DEADLINE:-20s}\""]
