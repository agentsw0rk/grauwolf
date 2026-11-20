# ADR-0011: Edition 2024, MSRV 1.85

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
Async closures and `async fn` in traits are central to the API.

## Decision
Rust edition 2024, minimum Rust version 1.85.

## Consequences
Older toolchains are not supported. Exception: `grauwolf-sandbox` requires Rust 1.97 because the current wasmtime version requires it (ADR-0029).
