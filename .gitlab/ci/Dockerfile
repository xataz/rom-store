FROM node:22-bookworm-slim AS node
FROM orhunp/git-cliff:2.10.1 AS cliff
FROM rust:1.100-bookworm

COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=cliff /usr/local/bin/git-cliff /usr/local/bin/git-cliff

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl git jq make pkg-config libssl-dev \
        libsdl2-dev fonts-dejavu-core && \
    rm -rf /var/lib/apt/lists/*

RUN rustup component add rustfmt clippy

RUN cargo install cargo-release --version 1.0.0 --locked \
    --no-default-features --features vendored-libgit2


ENTRYPOINT []
