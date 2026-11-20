# grauwolf-strategies

**Layer:** `domain` · **Status:** the strategy templates, split off from `grauwolf-spec` in ADR-0108 stage 6.

## Purpose
`expand(&StrategySpec) -> Result<Expansion, String>` turns `strategy { kind, params }` (ADR-0041) into an ordinary graph plus the types it needs (`Expansion { graph, types }`): the templates `single_run` (with `parallel_tools`, `moderate`), `react`, `verified` and `chat` (F05.12, ADR-0066, ADR-0076). The parameters are checked with `check_params` from `grauwolf-graph`; errors are plain messages that `grauwolf-spec` wraps into `SpecError::Strategy`. `VERIFICATION_TYPE` and `MODERATION_TYPE` name the types a template adds.

Not here: merging the expansion into an `AgentSpec` (user type collisions, "strategy next to graph") – that is `AgentSpecExt::from_json` in `grauwolf-spec`.

## Where to add a strategy
A new template is a function building a `GraphSpec` in `src/lib.rs`, a `match` arm with its parameter schema in `expand`, and a test in `tests/expand.rs`; tests that need loading and validation of the expanded spec go to `grauwolf-spec/tests/strategies.rs`.

## Features / ADRs
F05.12, ADR-0041, ADR-0066, ADR-0076, ADR-0108

## Allowed dependencies
- grauwolf crates: grauwolf-spec-model, grauwolf-graph (`check_params`) only – **not** `grauwolf-spec` (checked by `check-arch`)
- External: serde_json

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- Pure functions, no IO; the same input gives the same graph.
- Node kinds and parameters only as the node catalog knows them (ADR-0018); the sequential loops route answers `on_tool_call`, `on_multiple_tool_calls`, `on_assistant_message` in this order (ADR-0041 addendum).

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-strategies`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
