# ADR-0016: Layers domain/adapters/support/apps (ports & adapters)

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
Domain logic and technology should be developed and tested separately.

## Decision
Crates live in `crates/{domain,adapters,support,apps}`.
- **domain** defines ports, without IO. Allowed external dependencies: `serde`, `serde_json`, `schemars`, `thiserror`, `futures-core`, `futures-util`, `async-lock`, `pin-project-lite`. Forbidden: `tokio` (in any form), `reqwest`, `sqlx`, `wasmtime`, file system and process access.
- **adapters** implement ports.
- **support** is generic and depends on no grauwolf crate. Exception: the test kit `grauwolf-test` may depend on domain crates. Domain crates use the test kit only in integration tests (`tests/`), never in unit tests, so that no duplicate types arise.
- **apps** compose adapters.
An architecture test in CI checks these rules.

## Addendum (2026-11-12, ADR-0108 stage 4d): order within apps
Among the crates of `apps`, the order is `grauwolf-host` (composition root) → `grauwolf-server` →
`grauwolf-cli`; an app crate depends only on those below it. The facade `grauwolf` is the public API on
top (re-exports, macros, Cargo features, no logic) and no crate in the workspace depends on it.
Domain, adapters and support crates never depend on an app crate, not even as a `dev-dependency`.
The architecture test checks this for every kind of dependency.

## Consequences
Clear boundaries, smaller contexts for AI-assisted development; somewhat more crates.
