# grauwolf-sandbox

**Layer:** `adapters` · **Status:** `SandboxRuntime` (wasmtime 50) implements `ToolRuntime` per Tool-ABI v1 (ADR-0029); a fresh instance per call, fuel and memory limit (`Limits`), modules with imports are rejected at load time. Own `rust-version` 1.97 (wasmtime)

Package limits (ADR-0124): `Limits.time` (wall clock per call, default none) and `Limits::stricter(&ToolLimits)`; `instantiate_limited` loads a tool with the stricter of the host's and the package's limits (memory in MiB, fuel, `time_ms`). Time limits use wasmtime's epoch interruption: the engine is built with it, every store gets a deadline (far away without a time limit), and a ticker thread (started once per engine with the first time-limited tool, `TICK` = 5 ms, ends with the engine via `EngineWeak`) advances the epoch; `Trap::Interrupt` becomes "ran longer than its time limit (N ms)", a memory above the limit fails the instantiation with a message naming the limit.


## Purpose
ToolRuntime via wasmtime: loads prebuilt tool modules (core modules per Tool-ABI v1, ADR-0029) and enforces the fuel and memory limit.

## Features / ADRs
E13 (F13.11, F13.15 execution), ADR-0014, ADR-0029

## Allowed dependencies
- grauwolf crates: grauwolf-tools
- External: wasmtime (without default features: cranelift, runtime, std, wat), serde_json, futures-util, thiserror; dev: futures-executor

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- Modules without imports (ADR-0029): no file system, no network, no host access; modules with imports are rejected at load time.
- The fuel and memory limits always apply; a fresh instance per call.
- **Goal:** pass the ToolRuntime conformance suite (not yet in grauwolf-test).

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-sandbox`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
