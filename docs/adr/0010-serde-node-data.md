# ADR-0010: All node data is serde-capable; persistence via sqlx

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
Checkpoints must store node inputs/outputs. Rust has no dynamic serialization of arbitrary types.

## Decision
All node input/output types must be `Serialize + DeserializeOwned + Send + 'static` (trait bound in the builder). Database stores use `sqlx` (Postgres, MySQL, SQLite).

## Consequences
Every node transition is serializable; enables ADR-0017. The full checkpoint scope (session, storage, path) is described in ADR-0020. Non-serializable handles belong in `Extensions` (ADR-0020), not in node data or storage. Streams never travel over edges (ADR-0023).
