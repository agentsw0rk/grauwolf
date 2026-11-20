# ADR-0044: Agent storage – named values in the context, nodes `store` and `load`

- **Status:** Accepted (User, 2026-11-07: "yes, keep going.", item 5 of the agent features)
- **Date:** 2026-11-07

## Context
Agents need a typed key-value store (F04.10) through which nodes exchange data that does not travel along edges (F05.8), and which is persisted with checkpoints (F07.18). In grauwolf, data so far flows only along edges.

## Decision
- `AgentContext` gains a store `String → serde_json::Value` with `store(key, &impl Serialize)` and `load::<T>(key)`. It belongs to the run: fresh per run, part of `ContextSnapshot` (serde default empty), and thus included in checkpoints.
- Node kinds:
  - `store` (`params { key, value }`, signature `value → value`): stores the passing value under `key` and passes it on.
  - `load` (`params { key, value, type }`, signature `value → type`): ignores the input and returns the stored value; if it is missing or does not match the type, that is a node error.
- Values are stored as JSON; the interpreter and generated code see the same data.

## Consequences
- Typical patterns (remembering the original task, intermediate results for later steps) need no detours via the history.
- Type errors between `store` and `load` surface only at runtime; spec validation does not check keys.
