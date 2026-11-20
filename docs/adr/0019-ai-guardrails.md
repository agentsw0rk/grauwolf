# ADR-0019: Guardrails for AI-assisted development

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
The project is developed with AI agents that work with limited context.

## Decision
Each crate has an `AGENTS.md` (symlink `CLAUDE.md`) with purpose, allowed dependencies, features, invariants and status; invariants not yet implemented are marked as "goal". Decisions are recorded as ADRs in `docs/adr/`. In addition: architecture fitness test, conformance suites per port, equivalence tests interpreter ⇔ generator on `examples/specs/`, snapshot tests (`insta`), `#![deny(missing_docs)]` (as soon as crates have content), feature IDs in tests/docs. Each feature ID from the catalog is assigned to exactly one crate.

## Consequences
More documentation upkeep; in return, the context of one crate is enough for an AI agent.
