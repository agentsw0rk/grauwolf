# ADR-0030: Layered isolated tool build

- **Status:** Accepted (user request "isolated build", 2026-11-06); refines ADR-0024
- **Date:** 2026-11-06

## Context
Tool code from specs may come from an LLM and is compiled with cargo. Risks during the build: reading host files or environment at compile time, network access, and resource exhaustion (e.g. endless const evaluation). Operating-system-level isolation (namespaces, bwrap, nsjail) is not available everywhere – for example not in the development environment (Docker with the default seccomp profile).

## Decision
The build is secured in layers; each layer works independently of the others:
1. **Code check** before the build: forbidden constructs (`include*!`, `env!`, `option_env!`, `mod`, `extern`, `unsafe`, attributes) are rejected.
2. **Hermetic cargo invocation:** empty process environment except toolchain paths (`PATH`, `CARGO_HOME`, `RUSTUP_HOME`, `RUSTUP_TOOLCHAIN`); `--offline`. Dependencies of the guest crate (v1: only `serde_json`) are fetched by an upstream, trusted step, `ToolBuilder::prepare()`, before any foreign code is involved; it creates the shared `Cargo.lock` that every guest crate receives and that pins the versions. (`--locked` is dropped because the name of the root crate differs per tool.) Each build runs in its own working directory (guest crate + target), which is deleted afterwards; the artifact is stored atomically via `rename`. Thus builds share no state – not even two concurrent builds of the same tool – and cargo does not have to lock a shared directory (on mounted filesystems without exclusive `flock`, parallel builds failed).
3. **Time limit** per build; when it is exceeded, the process is terminated.
4. **Optional OS wrapper:** a configurable command prefix (e.g. `bwrap …` or `docker run --network none …`) under which cargo runs. Recommended for production environments with LLM-generated specs.

## Consequences
- Layers 1–3 apply everywhere and are tested; layer 4 depends on the environment and is configured there.
- Before the first build, `prepare()` (or `grauwolf spec build`, which calls it) must run with network access.
- Residual risk without layer 4: bugs in rustc/cargo itself. Therefore tool execution additionally stays in the WASM sandbox (ADR-0014, ADR-0029).

## Addendum (2026-11-09): `HOME` in the build environment
The code also passes `HOME` through to cargo, in addition to `PATH`, `CARGO_HOME`, `RUSTUP_HOME` and `RUSTUP_TOOLCHAIN` (`TOOLCHAIN_ENV` in `crates/adapters/grauwolf-toolbuild/src/lib.rs`); rustup and cargo derive their default directories from it when `CARGO_HOME`/`RUSTUP_HOME` are not set. All other variables stay out.
