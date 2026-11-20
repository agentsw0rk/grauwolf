# ADR-0001: Async runtime tokio

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
Agents are IO-heavy (LLM calls, tools, streaming). Rust needs an explicit async runtime.

## Decision
`tokio` is the runtime for adapters and apps. Domain crates do **not** depend on tokio (not even `sync`); they use `async fn`, `futures-core`/`futures-util`, `async-lock` and the ports `Clock` and `Spawn` (ADR-0026). At `dyn` boundaries (`LlmClient`, `ToolRuntime`) `BoxFuture`/`BoxStream<'static, _>` are used, because `async fn` in traits is not dyn-compatible.

## Consequences
Largest ecosystem (reqwest, axum, sqlx, wasmtime-async). Other runtimes are not supported.
