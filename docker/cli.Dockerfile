# The grauwolf CLI (ADR-0096).
#
#   docker build -f docker/cli.Dockerfile -t grauwolf .
#   docker run --rm grauwolf --version
#   docker run --rm -v "$PWD:/work" -w /work grauwolf spec validate spec.json
#
# Targets:
#   runtime    (default) the CLI with ca-certificates, no Rust toolchain. Enough for
#              `spec validate|serve|run|chat|mcp` of specs without tool code; the git tools
#              (ADR-0104) are part of the binary (libgit2).
#   toolchain  the CLI on top of the Rust toolchain with the wasm32 target, for specs with tool
#              code (`spec build`, `spec run`, `spec generate`), which compile at run time.
#
# The Rust version is pinned; keep it in step with docker/agent.Dockerfile.
FROM rust:1.99-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p grauwolf-cli

FROM rust:1.99-slim-bookworm AS toolchain
RUN rustup target add wasm32-unknown-unknown
COPY --from=build /src/target/release/grauwolf /usr/local/bin/grauwolf
ENTRYPOINT ["grauwolf"]

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 65532 --create-home --shell /usr/sbin/nologin nonroot
COPY --from=build /src/target/release/grauwolf /usr/local/bin/grauwolf
USER nonroot
WORKDIR /home/nonroot
ENTRYPOINT ["grauwolf"]
