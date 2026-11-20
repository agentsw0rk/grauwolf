# ADR-0095: Tool sources in generated crates, compiled during the image build

- **Status:** Accepted (ADR-0092), implemented 2026-11-12
- **Date:** 2026-11-10

## Context
`spec generate` compiles tool code to WASM at generation time and embeds the artifacts
(`tools/<name>.wasm`, `include_bytes!`, ADR-0050). AgentWeaver generates at export time inside its
own backend container and can only pass text files through its export pipeline (ZIP, code
explorer, workspace repository, kaniko context). A Rust toolchain with the `wasm32-unknown-unknown`
target in AgentWeaver's container would be large and slow down every export.

## Decision
- **`spec generate --tool-sources`:** instead of building WASM, the generator writes each tool as
  source (`tools/<name>/src/lib.rs` + `tools/<name>/Cargo.toml`, the same guest crate
  `grauwolf-toolbuild` builds today) and a manifest `tools/tools.json` (name, hash of the code). The
  generated `lib.rs` still embeds `tools/<name>.wasm`; these files do not exist yet.
- **`grauwolf tools build <crate-dir>`:** builds every tool source into `tools/<name>.wasm` with the
  isolated tool build (ADR-0030; own target dir, allowlist of crates, no build scripts of
  third-party crates) and checks the hashes against the manifest. The generated `Dockerfile` runs
  it before `cargo build`; the builder image contains the CLI (ADR-0096).
- **Errors:** compile errors of tool code are reported per tool name (JSON with `--format json`),
  so AgentWeaver can map them to the tool node.
- Without `--tool-sources` nothing changes (`spec build`/`spec generate` as before).

## Consequences
- AgentWeaver's container needs only the grauwolf CLI, not a Rust toolchain; exports stay text.
- The image build compiles the tools in a separate step; kaniko can reuse that layer when the tool
  sources did not change (tools are copied before the rest of the crate).
- `cargo build` without `grauwolf tools build` fails with a missing-file error naming the command.

## Implementation notes
- **Where the pieces live:** the guest template stays in `grauwolf-toolbuild` (`guest_crate(&ToolSpec)`,
  `source_hash`, `ToolBuilder::build_sources`); the generator (domain) cannot depend on it, so
  `GenerateOptions::tool_sources` takes `ToolSourceCrate`s (`grauwolf-tools`: name, both files, hash) that the
  CLI produces. The domain has no hash function (`check-arch` forbids `sha2` there), hence the producer
  computes the hash. The manifest type `ToolSourcesManifest` (format 1; per tool `name`, `module`,
  `sha256`) is in `grauwolf-tools`.
- **Hash:** SHA-256 over `Cargo.toml` and `src/lib.rs`, each with path and length. The crate name is
  `gw_tool_<name>` (no cache hash), so the module bytes differ from `spec build` of the same tool
  but behave the same (tests compare behaviour).
- **`tools build`:** `grauwolf tools build <dir> [--format text|json] [--force] [--cache] [--build-wrapper]`.
  Checks hashes, `prepare()` once (offline in the builder image), `build_sources` per tool, writes
  `tools/<name>.wasm` atomically and a stamp `tools/<name>.wasm.src-sha256`; a module with a matching stamp is
  skipped unless `--force`. Exit code 1 if any tool fails. JSON:
  `{ "tools": [ { "name", "ok", "skipped", "errors": [ { "message", "line"?, "column"? } ] } ] }`,
  line and column relative to the tool code (`CodeDiagnostic` mapping of F13.9); other failures (hash
  mismatch, unreadable files, timeouts) are one error without position. The code check for forbidden
  constructs runs when the sources are written (`guest_crate`); `tools build` relies on the hash.
- **Missing modules:** a std-only `build.rs` in the generated crate panics with "tool modules missing:
  tools/add.wasm - run `grauwolf tools build .` in this directory first" (no build dependencies, and
  chosen over a `compile_error!` because it names every missing module before the compiler runs).
  The ADR text above said "missing-file error"; this is the clearer variant.
- **Dockerfile:** `COPY tools/ tools/`, `RUN grauwolf tools build . --cache /tmp/tool-cache`, then `COPY . .`.
  `docker/builder.Dockerfile` needed no change: `ToolBuilder::prepare()` resolves `serde_json` from
  the vendored sources (ADR-0096). The CI job `builder` generates `calculator.json` with
  `--tool-sources --dockerfile`, builds with `--network none`, starts it and checks `/health`
  (CI-verified only, no Docker here).
- **Deviation/limitation:** tool packages (ADR-0107) are refused with `--tool-sources`: OCI packages
  with "OCI tool packages carry built modules; generate without --tool-sources or use a git source";
  git packages are refused too ("not yet written as sources"). Writing them as sources needs a
  resolver entry point that hands out the fetched sources without building, because the lockfile
  records module digests. `--check` is refused with `--tool-sources` (the check runs `build.rs`).
- **Live check (2026-11-12):** `calculator.json` (model `qwen3.5:9b`) generated with `--tool-sources
  --service` by a CLI with an empty environment (`env -i PATH=""`: success, no `tools/add.wasm`);
  `cargo build` failed with the `build.rs` message; `grauwolf tools build` built the module, a second
  run skipped it; `cargo build --features service` and the service answered
  `{"output":"The sum of 1234 and 4321 is 5555."}` (Ollama `host.docker.internal:11434`,
  `OLLAMA_THINK=false`). A deliberate type error in the tool gave, with `--format json`,
  `{"tools":[{"errors":[{"column":14,"line":1,"message":"mismatched types"}],"name":"add","ok":false,"skipped":false}]}`
  and exit code 1.
