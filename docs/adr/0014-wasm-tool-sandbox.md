# ADR-0014: Tool code from specs runs in a WASM sandbox

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06
- **Refined by:** ADR-0029 (v1: core module without imports)

## Context
Tool code in specs may come from an LLM and is executed.

## Decision
Tool code from specs is executed in both modes (interpreter and generated code) as a WASM component (`wasm32-wasip2`) via the `ToolRuntime` port of `grauwolf-sandbox` in `wasmtime`. Building the components is a separate step (ADR-0024); `ToolRuntime` only receives finished artifacts plus capabilities, never source code. Capabilities (e.g. allowed HTTP hosts) are declared by the spec; fuel/epoch, memory and time limits always apply. What a sandbox tool may see of the agent context is defined by a WIT interface. Rust tools written by the developer still run in-process.

## Consequences
Strong isolation, uniform for both modes. Requires the toolchain for `wasm32-wasip2` at build time.
