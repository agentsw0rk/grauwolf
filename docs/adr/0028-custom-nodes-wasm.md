# ADR-0028: Custom nodes in specs only as WASM transformations

- **Status:** Accepted (review iteration 1, 2026-11-06)
- **Date:** 2026-11-06

## Context
Custom nodes with code would need context access (LLM, storage) that cannot sensibly be sandboxed.

## Decision
In version 1, custom nodes in specs are either pure transformations `Value → Value` as a WASM component (WIT interface `transform`, built via ADR-0024), or compositions of catalog nodes and subgraphs. Nodes with context access exist only in the Rust builder.

## Consequences
Safe, and the same in both modes; complex logic is modeled via subgraphs.
