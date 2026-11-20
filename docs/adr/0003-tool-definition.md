# ADR-0003: Tool definition via trait and `#[tool]` macro

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
Tools should be derived from ordinary, annotated functions, including description and argument schema. Rust has no runtime reflection.

## Decision
Typed `trait Tool` with associated `Args`/`Output`; internally an object-safe `DynTool` (JSON in/out). The proc macro `#[tool]` lives in `crates/support/grauwolf-macros` and generates implementations from functions; descriptions come from doc comments. The macro crate depends on no grauwolf crate; the generated code references `grauwolf_tools`.

## Consequences
Good ergonomics, but an additional macro crate has to be maintained.

## Addendum: `#[toolset]` (User, 2026-11-09: "can you continue with this: #[toolset]")
- `#[toolset]` on an `impl` block turns every method with `#[tool]` into a tool; other methods stay unchanged. Tool methods take `&self` (shared state, e.g. connections, counters behind `Mutex`) and return `Result<T, ToolError>`, sync or async; descriptions as with `#[tool]` from doc comments and `#[arg("…")]`.
- Generates per tool the type `<Type><Method>` (holds `Arc<Type>`, impl `Tool`) and `<Type><Method>Args`, plus `fn tools(self: Arc<Self>) -> Vec<Arc<dyn DynTool>>` in method order.
- Not supported (compile error with explanation): generic `impl` blocks, trait implementations, tool methods without `&self` or with `&mut self`.

## Addendum: macros via the facade (User, 2026-11-09: "export the macros via the facade")
The generated code referred hard to `::grauwolf_tools`; users of the facade therefore needed `grauwolf-macros` and `grauwolf-tools` as direct dependencies. Without an additional dependency (for instance for reading `Cargo.toml`) there are now two variants of the same macros: `#[tool]`/`#[toolset]` generate paths under `::grauwolf_tools`, the hidden `facade_tool`/`facade_toolset` generate paths under `::grauwolf::tools`. The facade re-exports the latter as `grauwolf::tool` and `grauwolf::toolset` (feature `macros`, on by default). Anyone who only includes `grauwolf` writes `#[grauwolf::tool]`.
