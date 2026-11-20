# ADR-0022: Spec type system: nominal types

- **Status:** Accepted (review iteration 1, 2026-11-06)
- **Date:** 2026-11-06

## Context
Edges should be type-checked in the JSON spec as well; generated code needs real Rust types. Structural equality of JSON schemas cannot be decided sensibly.

## Decision
The spec has a `types` section with named JSON schemas (`"types": { "Answer": { … } }`). Edges are checked **nominally** (`Named("Answer")` only matches `Named("Answer")`). The generator creates a Rust struct (serde + schemars) from each type. The signature of a subgraph results from its start and finish.

## Consequences
Unambiguous, simple type checking; the generated code is fully typed.
