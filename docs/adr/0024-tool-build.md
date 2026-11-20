# ADR-0024: Tool build separated from execution

- **Status:** Accepted (review iteration 1, 2026-11-06)
- **Date:** 2026-11-06
- **Refined by:** ADR-0029 (ABI v1), ADR-0030 (isolated build)

## Context
Tool code from specs must be compiled to WASM. This needs cargo, rustc, the `wasm32-wasip2` target and dependencies, and takes 10–60 s from a cold start. So far no layer owned the build step.

## Decision
New adapter crate `grauwolf-toolbuild`: produces a `ComponentArtifact` (bytes, WIT world, hash) from a spec tool.
- Content-addressed cache, key = hash(tool code, args schema, crate allowlist, toolchain version).
- Explicit step `grauwolf spec build` (CLI); the interpreter loads only finished artifacts. "Without compiling" for the interpreter refers to the graph, not to tool code.
- The crate allowlist is served from a vendored offline registry; the build runs isolated.
- The generator uses the same artifacts.

## Consequences
Runtime is possible without a toolchain; repeated builds are cached. The build step needs a toolchain.

## Addendum (2026-11-09): artifact, cache key and dependencies
- Instead of a `ComponentArtifact` (bytes, WIT world, hash), the build yields a `ToolArtifact { name, bytes, hash }` (`grauwolf-tools`): a core WASM module for `wasm32-unknown-unknown` following tool ABI v1 (ADR-0029), not a WASI component for `wasm32-wasip2`.
- The cache key is a hash over the template version `GUEST_TEMPLATE_VERSION` (covers ABI and crate allowlist), the tool name, code, args schema and `rustc --version` (see `crates/adapters/grauwolf-toolbuild/AGENTS.md`).
- There is no vendored offline registry: `ToolBuilder::prepare()` fetches the dependencies of the guest crates (only `serde_json`) in advance, with network access, and creates a shared `Cargo.lock`; `build()` then runs with `--offline` (ADR-0030).
- The interpreter no longer loads only finished artifacts: tool folders (ADR-0074) and self-built tools (ADR-0075) are built at runtime with the `ToolBuilder`; a toolchain is then required.
