# grauwolf-toolbuild

**Layer:** `adapters` · **Status:** Code of graphs (ADR-0102, F14.14): `ToolBuilder::build_code(&CodePiece)` / `cached_code` / `code_guest_crate(&CodePiece)` build a `code` node, an `on_code` condition or an edge transformation exactly like a tool – same code check, isolated build, cache (the key covers the generated source), tool ABI v1 and `build_sources` – as a guest crate `gw_tool_<piece name>` whose `src/lib.rs` starts with the user's function (`fn run(input: In) -> Result<Out, String>`, `fn check(value: &In) -> bool`, `fn transform(value: In) -> Out`, so compile errors map to the lines of the code at `CODE_FIRST_LINE`), followed by generated Rust types for the spec types with hand-written JSON conversions (`src/code.rs`, no serde derive; the guest keeps only `serde_json`; supported are string, integer, number, boolean, arrays and optional fields, as in the generator) and the shared `ABI_GLUE`. Output is the JSON text of the result (`true`/`false` for a condition); a panic is a trap (`panic = "abort"`), `Err(msg)` is the tool error `msg`. `ToolBuilder::build(&ToolSpec)` generates a guest crate (cdylib, only `serde_json`) and builds it via cargo into a core WASM module per Tool-ABI v1 (ADR-0029). Tool code is tokenized before the build (lexer errors with line/column in the tool code, proc-macro2 `span-locations`) and checked for forbidden constructs (`include*!`, `env!`, `option_env!`, `mod`, `extern`, `unsafe`, attributes). If `rustc`, `cargo`, or the wrapper cannot be started, the result is `BuildError::ToolchainMissing { program }` with a hint about rustup/PATH/CARGO/RUSTC instead of a bare I/O error. Content-addressed cache (`cached()`, `artifacts/<hash>.wasm`); guest crates are named `gw_tool_<name>_<hash>`; each build runs in its own working directory `builds/<crate>-<pid>-<thread>-<nanos>` (crate + target), which is deleted afterwards; artifacts are stored atomically via `rename` – parallel builds (also of the same tool, also from several processes) share no state and do not rely on cargo locks. A time limit (`with_timeout`, default 300 s) terminates the whole cargo process group. cargo runs with an empty environment except for `TOOLCHAIN_ENV` (PATH, HOME, CARGO_HOME, RUSTUP_HOME, RUSTUP_TOOLCHAIN). Dependencies only via `prepare()` (network, without tool code; shared `Cargo.lock`, stored atomically); `build()` runs `--offline` and requires a prior `prepare()` (`BuildError::NotPrepared`). Compile errors come as `BuildError::Compile { diagnostics, output }`: cargo runs with `--message-format=json`, and errors are mapped to line/column in the tool code of the spec (`CodeDiagnostic`; the code starts at line `CODE_FIRST_LINE` of the guest crate; locations outside it have no position), F13.9. Optional OS wrapper `with_wrapper(..)` as a command prefix for cargo (e.g. bwrap, docker). Via the CLI: `--build-wrapper`.

Tool sources (ADR-0095): `guest_crate(&ToolSpec)` returns the guest crate as text (`ToolSourceCrate`: `Cargo.toml` with crate name `gw_tool_<name>`, `src/lib.rs`, `hash` = `source_hash(manifest, source)`, SHA-256 over path, length and content of both files); it runs the code check (forbidden constructs) and needs no toolchain. `ToolBuilder::build_sources(&ToolSourceCrate)` compiles such a crate (offline, needs `prepare()`, not cached, errors as `BuildError::Compile` with lines relative to the tool code); the caller verifies the hash. Module bytes differ from `build()` of the same tool (other crate name) but behave the same.

`ToolBuilder::cache_dir()` exposes the cache directory; tool packages (ADR-0107) are cached below it by grauwolf-toolpkg.

Toolchain stand-in (ADR-0115): the cache key contains `rustc --version`; where there is no `rustc` (the image of a generated service, which carries a copy of the cache) the key uses the file `<cache>/toolchain` (`TOOLCHAIN_FILE`) instead, which `ToolBuilder::export_toolchain(dir)` writes on the build machine; without `rustc` and without the file the result is `BuildError::ToolchainMissing` as before.

`ToolBuilder::rustc_version()` → `rustc --version` of the toolchain (or the exported `toolchain` file), recorded in packed tool packages (ADR-0124).


## Purpose
Builds tool code from specs into WASM modules (`ToolArtifact`: name, bytes, hash) with a content-addressed cache.

## Features / ADRs
E13 (F13.11 build, F13.15 build isolation), ADR-0024, ADR-0029

## Allowed dependencies
- grauwolf crates: grauwolf-spec, grauwolf-tools; dev: grauwolf-sandbox, grauwolf-test (`TestDir`)
- External: thiserror, sha2 (hash of tool sources), proc-macro2 (token check), serde_json (compiler messages); process invocation via std::process, the build is synchronous; dev: futures-executor, serde_json

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- Cache key = hash(template version `GUEST_TEMPLATE_VERSION` (covers the ABI and the crate allowlist), tool name, code, args schema, `rustc --version`). When the guest template changes, raise the template version.
- Isolation in layers per ADR-0030. **Goal:** the build runs hermetically; dependencies only from the allowlist (v1: only `serde_json`) via a vendored offline registry.
- Delivers artifacts, never runs them.
- If the guest template is changed before the tool code, `CODE_FIRST_LINE` must move with it (test `reports_compile_errors_with_lines_relative_to_the_tool_code`).
- Tool code must not access the host during compilation (F13.15); new Rust features that would allow this go into `FORBIDDEN_MACROS`/`FORBIDDEN_KEYWORDS`.

## Platform
- Unix only (process groups via `std::os::unix`, abort via `kill -KILL -<pgid>`).

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-toolbuild`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
