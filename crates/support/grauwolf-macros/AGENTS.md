# grauwolf-macros

**Layer:** `support` · **Status:** Compile-fail test `tests/compile_fail.rs`: one crate with all misuses (arguments to `#[tool]`/`#[toolset]`, no `Result`, pattern instead of name, trait impl, generic block, `&mut self`) under `<target>/macro-compile-fail`; `cargo check` must fail and contain each of our own messages – independent of rustc's wording, hence without trybuild. `#[tool]` on (async) functions `fn name(a: A, …) -> Result<T, ToolError>` generates `Name` (impl `grauwolf_tools::Tool`) and `NameArgs` (serde + schemars via `grauwolf_tools::__private`); the description comes from the function's doc comments, argument descriptions from `#[arg("…")]` (Rust does not allow doc comments on parameters). `#[toolset]` on our own non-generic `impl` blocks: each method with `#[tool]` and `&self` becomes a tool `<Type><Method>` (holds an `Arc<Type>`) with `<Type><Method>Args`, plus `fn tools(self: Arc<Self>) -> Vec<Arc<dyn DynTool>>`; other methods remain; `&mut self`, missing `self`, generics, and trait impls → compile errors (ADR-0003 addendum); shared helper `arguments` for functions and methods. The hidden variants `facade_tool`/`facade_toolset` generate paths under `::grauwolf::tools` instead of `::grauwolf_tools` (`Paths`) and are re-exported by the facade as `grauwolf::tool`/`grauwolf::toolset` (ADR-0003 addendum). Tests in `grauwolf-tools/tests/tool_macro.rs` and `toolset_macro.rs`.

## Purpose
Proc macros `#[tool]` (tools from functions) and `#[toolset]` (several tools from one `impl` block with shared state); descriptions come from doc comments.

## Features / ADRs
E03 F03.5, ADR-0003

## Allowed dependencies
- grauwolf crates: none (the generated code references `grauwolf_tools`)
- External: syn (full), quote, proc-macro2

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- Proc-macro crate without runtime dependencies.
- Error messages point at the user's function (`syn::Error::to_compile_error`).

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-macros`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
