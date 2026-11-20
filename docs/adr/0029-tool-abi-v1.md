# ADR-0029: Tool ABI v1 – core WASM module without imports

- **Status:** Accepted (implementation of slice 2, 2026-11-06); refines ADR-0014
- **Date:** 2026-11-06

## Context
ADR-0014 foresees WASM components (`wasm32-wasip2`, WIT) for tool code. For tools without capabilities (e.g. arithmetic, text processing), the WASI component model only adds costs: wit-bindgen in the guest, longer builds, and a larger attack surface through WASI imports.

## Decision
Version 1 of the tool ABI is a **core WASM module** (`wasm32-unknown-unknown`) **without imports**:
- Exports: `memory`, `gw_alloc(len: i32) -> i32`, `gw_call(ptr: i32, len: i32) -> i64`.
- Input: arguments as UTF-8 JSON in guest memory (`gw_alloc`, then `gw_call`).
- Output: `gw_call` returns `(ptr << 32) | len` of a UTF-8 JSON `{"ok": "<text for the LLM>"}` or `{"err": "<error>"}`.
- A module with imports is rejected (no capabilities in v1). Fuel and memory limits always apply; each call runs in a fresh instance.
- The guest code receives `args: serde_json::Value` and returns `Result<String, String>`; the argument schema from the spec describes the tool to the LLM.

## Consequences
- Strongest isolation by construction: no filesystem, network, randomness or clock.
- Tools with capabilities (e.g. HTTP per ADR-0014) require ABI v2 (WASI components with WIT); the port `ToolRuntime` stays the same.
- `grauwolf-sandbox` raises its `rust-version` to 1.97 because the current wasmtime version (50) requires it; the rest of the workspace stays at MSRV 1.85 (ADR-0011).
