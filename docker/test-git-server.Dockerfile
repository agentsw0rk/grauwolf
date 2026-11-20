# The test git server with its mTLS mode (ADR-0110): the example `serve_folder` of grauwolf-git
# (feature `test-git-server`) as a container, for the live check of the git tools with SPIFFE.
#
# NOT PART OF THE RELEASE IMAGES. It serves a folder as a writable git repository without any
# real access control beyond the mapping file, keeps the repository in /tmp and is meant only for
# tests in a cluster of your own. Never publish it next to the agent images.
#
#   docker build -f docker/test-git-server.Dockerfile -t test-git-server .
#   # in the cluster: see examples/test-git-server/
#
# Runtime: debian-slim, non-root (uid 65532), no shell is needed but the image keeps one for
# debugging. The arguments are those of `serve_folder`:
#   <folder> <path-in-repo> <port> [--bind <address>] [--spiffe <mapping-file>]
# The default command seeds the repository with the folder /seed (one README.md) and serves it
# on 0.0.0.0:8443 over mTLS with the mapping /etc/test-git-server/mapping.json. /tmp must be
# writable (an emptyDir with a read-only root file system). The Rust version is pinned; keep it
# in step with docker/cli.Dockerfile.
FROM rust:1.99-slim-bookworm AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential cmake \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p grauwolf-git --features test-git-server --example serve_folder

FROM debian:bookworm-slim AS runtime
COPY --from=build /src/target/release/examples/serve_folder /usr/local/bin/serve_folder
RUN mkdir /seed \
    && echo "# test repository of the grauwolf live check" > /seed/README.md \
    && chown -R 65532:65532 /seed
USER 65532:65532
EXPOSE 8443
ENTRYPOINT ["/usr/local/bin/serve_folder"]
CMD ["/seed", "", "8443", "--bind", "0.0.0.0", "--spiffe", "/etc/test-git-server/mapping.json"]
