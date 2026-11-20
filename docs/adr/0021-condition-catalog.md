# ADR-0021: Condition catalog and expression language for edges

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
In the builder, edge conditions are Rust closures; in JSON there is no representation for them, and `on_condition` "with expression" has no defined language.

## Decision
Edge conditions in the spec are objects from a **condition catalog** in `grauwolf-graph`: `ConditionDescriptor { kind, params_schema, fn signature(&params, input: DataType) -> Result<DataType> }`, e.g. `{ "kind": "on_tool_call", "tool": "add" }` (`Message → ToolCall`), `on_assistant_message` (`Message → String`).
Free-form conditions and transformations use **CEL** (Common Expression Language) over `Value`, evaluated by an embedded library, identically in both modes. No Rust code in conditions.

## Consequences
Conditions are the same in builder, interpreter and generator.

## Addendum (2026-11-09): no CEL, only catalog conditions
- CEL is not implemented; there are no free-form expressions in conditions. Edge conditions come exclusively from the condition catalog in `grauwolf-graph` (`ConditionCatalog::builtin()`): `on_assistant_message`, `on_multiple_tool_calls`, `on_tool_call`, `on_field { field, equals }` (ADR-0041) and `on_history_longer { messages }` (ADR-0037). Branching on data goes through structured outputs and `on_field`.
- In the spec a condition is written as `"when": { "kind": "…", "params": { … } }` (`ConditionSpec`). `on_tool_call` has no parameters (schema with `additionalProperties: false`); the example above with `"tool": "add"` would be invalid. Valid is `{ "kind": "on_tool_call" }`.
