# grauwolf-codegen

**Layer:** `support` · **Status:** Fork import of codegen 0.3.0 (crates.io, source gitlab.com/yovoslav/codegen) unchanged apart from the crate names; upstream tests in `tests/upstream.rs`. Extension `Scope::to_formatted_string()` (syn check + prettyplease, `FormatError` with line/column) in `src/formatted.rs`. A source map (originally a target in ADR-0013) was discarded (ADR-0053).

## Purpose
Generic Rust code builder (fork of carllerche/codegen) with syn validation and prettyplease formatting (there is no source map, ADR-0053).

## Features / ADRs
ADR-0013

## Allowed dependencies
- grauwolf crates: none (must not depend on any grauwolf crate)
- External: indexmap, syn, proc-macro2, prettyplease

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- No knowledge of agents or specs – keep it upstream-capable.
- License MIT (`license = "MIT"`, `LICENSE-MIT`, notice in `NOTICE`); upstream changelog in `CHANGELOG-UPSTREAM.md`.
- Keep our changes relative to upstream small and traceable (upstream-capable); put extensions in their own files (e.g. `src/formatted.rs`) and keep upstream files as unchanged as possible.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-codegen`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
