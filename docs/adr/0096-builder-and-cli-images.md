# ADR-0096: Builder and CLI images

- **Status:** Accepted (User, 2026-11-11; the open points were closed with the owner's decisions of 2026-11-13, see "Closing the open points"): CLI image, runtime image for generated services, offline builder image with `--dockerfile`, version check, arm64 builder on native runners and publishing preparation. Everything about Docker and the workflow is CI-verified only (no Docker in the development container).
- **Date:** 2026-11-10

## Context
Generated crates depend on the grauwolf crates by path (`--grauwolf-crates`) and on many crates from
crates.io. AgentWeaver builds images in the cluster with kaniko (deadline 30 minutes, network to
the registry only guaranteed). A cold Rust build of a crate with the facade takes far longer than a
JVM build and needs network access to crates.io.

## Decision
- **`grauwolf-builder:<version>`** (Debian slim with a pinned Rust toolchain):
  - target `wasm32-unknown-unknown`, the `grauwolf` CLI;
  - grauwolf sources of exactly this version under `/opt/grauwolf/crates`;
  - all dependencies vendored (`cargo vendor`, source replacement in `$CARGO_HOME/config.toml`), so
    builds run with `--offline`;
  - a pre-warmed target directory `/opt/grauwolf/target` from building a template crate with all
    facade features, so only the agent crate and its tools are compiled.
- **`grauwolf:<version>`** (Debian slim): only the `grauwolf` CLI; AgentWeaver copies the binary
  into its backend image (`COPY --from=ghcr.io/agentsw0rk/grauwolf:<version>`).
- **Runtime base** for generated services: `gcr.io/distroless/cc-debian12:nonroot` (C library,
  certificates, user 65532); TLS is rustls, no OpenSSL.
- **Versioning:** both images are built from a release tag `v<version>`; the export writes the exact
  builder version into the `Dockerfile` (`FROM …/grauwolf-builder:<version>`), and the generated
  `Cargo.toml` points to `/opt/grauwolf/crates`. The CLI refuses to generate for a different
  builder version than its own unless told otherwise.
- **Build and publish:** `docker/builder.Dockerfile`, `docker/cli.Dockerfile` and a GitHub Actions
  workflow (tag push → build for `linux/amd64` and `linux/arm64` on native runners → push by
  digest and merge into one multi-arch tag on `ghcr.io/agentsw0rk/…`). Images can also be built locally with the same Dockerfiles.

## Consequences
- An agent image builds offline in minutes; the builder image is large (several GiB) but pulled
  once per node.
- Every grauwolf release produces matching images; AgentWeaver pins the version it was tested with.
- Publishing requires `packages: write` for the workflow; the packages are private, so users and clusters need a pull secret.

## Implementation notes
- **Done:** `.dockerignore`, `docker/cli.Dockerfile`, `docker/agent.Dockerfile` and
  `.github/workflows/images.yml`. `grauwolf --version` was added for the image check. Not done yet: the
  version check of the exporter against the builder (see "Builder image").
- **CLI image:** `grauwolf:<version>` has two targets. The default `runtime` is Debian slim with
  `ca-certificates` and `git` (the git tools need the program, ADR-0104), non-root, **without** a Rust
  toolchain: it serves and runs specs without tool code. Specs with tool code compile at load
  time, so the target `toolchain` (Rust plus `wasm32-unknown-unknown`) exists for them and for
  `spec generate`; it is not published. This deviates from the plan above ("only the CLI binary") by
  adding `git` (dropped again with the git2 addendum of ADR-0104: no `git` in any image).
- **Agent image:** one multi-stage Dockerfile builds the CLI, runs `spec generate --service` and
  `cargo build --release --features service` (with the workspace `Cargo.lock` copied into the crate for
  reproducible versions) and offers `runtime` (distroless `cc-debian12:nonroot`, the last and default
  stage); the variant `runtime-git` of the first version is gone since the git2 addendum of ADR-0104. The spec's directory becomes `/app` (working directory and
  `GRAUWOLF_DATA_DIR`). It builds from sources and the network, i.e. it is **not** the offline
  builder flow of this ADR; that flow stays open.
- **Publishing:** first version: tag `v*` pushed the CLI image with QEMU; superseded, see "Closing the open points" (native runners, three images, private packages). The agent runtime image is
  not published because it contains one spec. The Rust image tag (`rust:1.99-slim-bookworm`)
  was chosen from the toolchain of the development container and has not been pulled here: no
  Docker was available, so CI is the first real build.

### Builder image and `--dockerfile` (implemented)
- **`docker/builder.Dockerfile`** (`--build-arg GRAUWOLF_VERSION=<version>`, checked against
  `grauwolf --version`): `rust:1.99-slim-bookworm`, the target `wasm32-unknown-unknown`, the
  repository (without `.dockerignore`d paths) in `/opt/grauwolf`, the CLI in `/usr/local/bin`,
  `cargo vendor --locked` into `/opt/grauwolf/vendor` and a `$CARGO_HOME/config.toml` with source
  replacement and `net.offline = true` (the registry download is deleted afterwards). Pre-warm: it
  generates a crate from `examples/specs/echo.json` with `--service --full-facade
  --grauwolf-crates /opt/grauwolf/crates`, builds it with `CARGO_TARGET_DIR=/opt/grauwolf/target
  cargo build --release --offline --features service`, then removes the template crate's own
  artifacts and sources, keeping the compiled dependencies. `ENV GRAUWOLF_VERSION` and the label
  `org.opencontainers.image.version` name the version.
- **`--full-facade`** (generator option `full_facade`): the generated crate depends on the facade
  with its default features, i.e. all of them, instead of the list the spec needs. Measured with the
  vendored sources and a fresh `CARGO_HOME` that has no registry: after the template build, crates
  generated from `echo.json` with a changed system prompt, `triage.json` (types), `calculator.json`
  (tools, sandbox), `planner.json`, `notes_reader.json` (built-in tools) and `coordinator.json`
  (sub-agents) each compiled **only the agent crate** (2 to 4 seconds in release mode on this
  machine); the template's own build took 2 min 7 s with `-j 4` (cold: all dependencies; the
  builder image build additionally compiles the CLI and vendors). Vendored sources: 507 MB;
  `agent` binary: 34 MB; target directory: 1.1 GB.
- **`--dockerfile`** implies `--service`, `--full-facade` and `--grauwolf-crates
  /opt/grauwolf/crates`; `--builder-image` overrides `ghcr.io/agentsw0rk/grauwolf-builder:<CLI
  version>`. The generated `Dockerfile` copies `/opt/grauwolf/Cargo.lock` into the crate before it
  builds (**deviation** from "the generator embeds the lock": the builder image is the source of
  truth for the versions, so nothing can drift, and the generator needs no copy of the lock),
  builds offline into the pre-warmed target directory and copies `agent` to the runtime stage
  (distroless; since the git2 addendum of ADR-0104 also for specs with git tools). The runtime files
  come from the directory `app/` of the crate (`/app`, working directory and `GRAUWOLF_DATA_DIR`),
  which the CLI fills with the paths the spec names (knowledge sources and index, memory store,
  file tools root: `companion_paths`; sub-agent specs recursively, paths relative to each spec).
  Paths outside the spec's directory are not copied. The Dockerfile runs on the agent's own
  crate, so it does not copy the spec: the spec is embedded in the binary (`SPEC_JSON`).
- **Tool code:** `spec generate` compiles tool code to WASM on the machine that runs it, before
  the crate is written; the Docker build needs nothing for tools (the artifacts are in the crate).
  `ToolBuilder::prepare()` resolves the guest dependencies (`serde_json`) and works offline with
  the vendored sources: measured with a cold cache, a spec with a tool generated in 2 seconds
  without network. AgentWeaver should run `spec generate` **in the builder image** (CI does).
  Tool sources in generated crates (ADR-0095, `--tool-sources`, `grauwolf tools build`) are
  implemented since 2026-11-12: the generated `Dockerfile` copies `tools/` first and runs
  `grauwolf tools build . --cache /tmp/tool-cache` in a layer of its own (the builder image needs no
  change: `prepare()` works offline with the vendored sources); the CI job `builder` also builds and
  starts `calculator.json` that way (CI-verified only).
- **CI:** a second job `builder` frees runner disk space, builds the image (amd64), generates the
  `echo` agent in it, builds it with `docker build --network none`, prints both durations, starts
  it and checks `/health`. On tags it pushed `grauwolf-builder:<version>` for amd64 only (QEMU would have taken hours); now both architectures run natively, see "Closing the open points". The image size, the build time of the
  builder image and the offline build time in CI could not be measured here (no Docker).
- **Open:** none; see "Closing the open points". (The first CI run may still show that the free runner disk is too small.)
- **Implementation notes (git2, ADR-0104 addendum):** the git tools no longer run the `git` program,
  so no image contains `git`: the target `runtime-git` of `docker/agent.Dockerfile`, the generator's
  `uses_git` (and the Debian slim runtime stage of the generated `Dockerfile`), the `git` package
  of `docker/cli.Dockerfile` and the CI steps for the git image are removed. libgit2 and OpenSSL
  are compiled from their vendored C sources and linked statically, so the build stages of the
  three Dockerfiles install `perl` and `make` (OpenSSL's `Configure` needs them); the builder
  image pre-warms them with the other dependencies (the facade with all features includes
  `grauwolf-builtin-tools`, hence git2), so a generated service does not recompile them. Not
  verified here (no Docker): the build of the three images with the new packages.
  Superseded on 2026-11-11 (ADR-0104 addendum): OpenSSL is gone, the `perl` and `make` installs
  are removed again.

### Closing the open points (2026-11-13)
Owner decisions (2026-11-13): arm64 on native runners, private packages with the owner setting the tag, and the guard design below.
- **Version guard.** The first build step of the generated `Dockerfile` is `RUN test "$GRAUWOLF_VERSION" = "<CLI version>" || { echo …; exit 1; }`; the builder image sets `GRAUWOLF_VERSION`. The message reads `crate generated by grauwolf X, builder is Y - regenerate with grauwolf Y or pass --builder-image with --allow-builder-mismatch`. `DockerfileOptions` has `expected_builder_version: Option<String>`; the CLI passes its own version. With `--builder-image` the guard still compares (a builder of another version builds against other vendored versions); `--allow-builder-mismatch` (new, only with `--dockerfile`) leaves the guard out – the one flag needed for a deliberate override. Tests: snapshot of the guard lines, a test that the guard is the first build step and one that it is absent with `None`; the CI builder job builds the `echo` and `calculator` agents with the guard (positive case, per architecture).
- **`grauwolf version --json`.** A subcommand (clap's `--version` is a fixed flag and does not combine with `--json`; `grauwolf --version` stays) prints `{"version": "<version>", "builder_image": "ghcr.io/agentsw0rk/grauwolf-builder:<version>"}`, so the AgentWeaver exporter reads its CLI's version and the builder it will use without parsing text. Test in the CLI's `tests/commands.rs`; the CI image check runs it too.
- **arm64 builder on native runners.** The builder job is a matrix over `ubuntu-24.04` (amd64) and `ubuntu-24.04-arm` (arm64); no QEMU, no emulated compilation. Each leg builds the image with `docker build`, runs the offline tests and, on tags, pushes its tested image and records the digest as an artifact. Deviation from the textbook pattern: the classic image store cannot push without a name, so the image goes up once as `grauwolf-builder:<version>-<arch>` (a helper tag, unused) and only its digest is used; building with `push-by-digest` through buildx would have meant testing a different build than the pushed one (or building twice, hours each). A `merge` job runs `docker buildx imagetools create -t …:<version> <digest-amd64> <digest-arm64>`.
- **The other images use the same split.** The CLI and the tools image (cold builds of 165 s and 53 s natively, hours under QEMU for 341 crates) are built per architecture natively in a `publish` job on tags and pushed by digest with `build-push-action` (`push-by-digest=true,name-canonical=true`), then merged by the same `merge` job (`:<version>` and `:latest`; the builder gets no `latest`: the guard ties a crate to an exact version). QEMU is gone from the workflow. The agent runtime image stays amd64 and unpublished (it holds one spec). The `publish` job waits for the smoke tests of the `images` job.
- **Publishing.** Packages stay private on `ghcr.io/agentsw0rk`; the owner sets the tag `v*` himself (nobody else creates or pushes it), the workflow fails when the tag differs from the workspace version. Job permissions: `contents: read` globally, `packages: write` only in the jobs that push. Labels `org.opencontainers.image.source`, `.version`, `.revision` on all published images (the `source` label ties a package to the repository). Release procedure, pulling private images (`docker login ghcr.io` with `read:packages`) and the image pull secret for AgentWeaver's cluster: developer guide section 2.10; user guide chapter 9.
- **Native arm64 runners and private repositories.** They are free for public repositories; for a private repository they may need a GitHub plan with arm64 larger runners. The arm64 leg of the builder matrix runs only on tags and manual runs (`workflow_dispatch`); pushes and pull requests build and test amd64 only, so a missing plan does not stall everyday CI – try a manual run before the first release tag (User, 2026-11-14).
- **Not verified (no Docker, no GitHub here):** the workflow (YAML reviewed carefully, no YAML parser was available; only indentation and tabs were checked mechanically), the Dockerfiles, the guard in a real build, the digest extraction and `imagetools create`, the runner label `ubuntu-24.04-arm`.
