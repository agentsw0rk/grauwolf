# ADR-0025: Strict spec deserialization, namespaces, IDs, versioning

- **Status:** Accepted (review iteration 1, 2026-11-06)
- **Date:** 2026-11-06

## Context
A typo such as `"wehn"` instead of `"when"` would be silently ignored with serde defaults and produce an unconditional edge. `#[serde(flatten)]` with internally tagged enums prevents `deny_unknown_fields`. A custom Rust enum `NodeKind` would duplicate the node catalog; a closed `Provider` enum would block custom providers.

## Decision
- `#[serde(deny_unknown_fields)]` on all spec structs; no `flatten`, no `untagged`.
- `NodeSpec { id, kind: String, params: Option<Value> }`; `kind` and `params` are validated against the catalog (JSON pointer errors). Conditions analogously as objects (ADR-0021).
- `provider` is a string, validated against the registered providers.
- Versioning: read as `Value` first, check `spec_version`, migrate if needed, then deserialize typed.
- Separate namespaces for nodes, tools, types and subgraphs. IDs are valid Rust identifiers (snake_case); `start` and `finish` are reserved and forbidden as node IDs.
- Nodes in subgraphs have path IDs (`sub/node`); they are the stable key for checkpoints.

## Consequences
Errors in LLM-generated specs are caught reliably. The existing code in `grauwolf-spec` is switched to this in slice 1, test-first.
