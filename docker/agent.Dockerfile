# An agent from a spec as a container: the generated crate with its HTTP service (ADR-0093,
# ADR-0096). Build context: the repository root.
#
#   docker build -f docker/agent.Dockerfile --build-arg SPEC=examples/specs/echo.json \
#     --target runtime -t my-agent .
#   docker run --rm -p 8080:8080 -e GRAUWOLF_SERVER_TOKEN=secret my-agent
#
# Targets:
#   runtime      (default, last stage) distroless, non-root (uid 65532); no shell. The git tools
#                (ADR-0104) are part of the binary (libgit2), no `git` program is needed.
#
# SPEC is a path inside the build context. The whole directory of the spec goes into the image
# as /app, which is the working directory and GRAUWOLF_DATA_DIR: knowledge sources, the memory
# store, the root of the file tools and other files that the spec names relative to itself must
# lie next to the spec. /app is writable for the service user; mount a volume there to keep
# long-term memory.
#
# The Rust version is pinned; keep it in step with docker/cli.Dockerfile.
FROM rust:1.99-slim-bookworm AS build
RUN rustup target add wasm32-unknown-unknown
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p grauwolf-cli
ARG SPEC=examples/specs/echo.json
# Tools of the spec are built to WASM here and embedded in the crate.
RUN ./target/release/grauwolf spec generate "$SPEC" --service \
        --out /agent --grauwolf-crates /src/crates \
    && cp Cargo.lock /agent/Cargo.lock \
    && cargo build --release --features service --manifest-path /agent/Cargo.toml \
    && mkdir /app \
    && cp -r "$(dirname "$SPEC")"/. /app/ \
    && cp /agent/target/release/agent /usr/local/bin/agent

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
COPY --from=build /usr/local/bin/agent /usr/local/bin/agent
COPY --from=build --chown=65532:65532 /app /app
WORKDIR /app
ENV GRAUWOLF_DATA_DIR=/app SERVER_PORT=8080
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/agent"]
