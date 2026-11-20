# The grauwolf-tools program (ADR-0118): build, pack, validate and push WASM tools without the
# agent runtime. For tool authors and CI pipelines.
#
#   docker build -f docker/tools.Dockerfile -t grauwolf-tools .
#   docker run --rm grauwolf-tools --version
#   docker run --rm -v "$PWD:/work" -w /work grauwolf-tools validate my-package
#   docker run --rm -v "$PWD:/work" -w /work -e GRAUWOLF_REGISTRY_AUTH \
#     grauwolf-tools push my-package ghcr.io/acme/tools/math:1.0
#
# Building tools compiles their code to WASM, so the image is the Rust toolchain with the target
# wasm32-unknown-unknown plus the program (no distroless runtime: `cargo` must be there), and git,
# so that `actions/checkout` keeps the history the package metadata (source, revision) and the
# CI template of `grauwolf-tools new` (`SOURCE_DATE_EPOCH` from the commit) read. The
# Rust version is pinned; keep it in step with docker/cli.Dockerfile.
FROM rust:1.99-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p grauwolf-tools-app

FROM rust:1.99-slim-bookworm AS runtime
RUN rustup target add wasm32-unknown-unknown \
    && apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/grauwolf-tools /usr/local/bin/grauwolf-tools
ENTRYPOINT ["grauwolf-tools"]
