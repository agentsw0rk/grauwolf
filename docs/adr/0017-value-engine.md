# ADR-0017: One graph engine on serde_json::Value for all entry points

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
Typed builder, interpreter and generated code should behave identically.

## Decision
`grauwolf-graph` contains a generic `Engine<Ctx>` that processes node data as `serde_json::Value`, plus the graph model and the typed builder. The node implementations (incl. the interpreter factory) live in `grauwolf-agents`, because they need `AgentContext`.
- Node: `Fn(&mut Ctx, Value) -> BoxFuture<Result<Value>>`.
- Edge: condition `Fn(&Ctx, Value) -> BoxFuture<Result<Option<Value>>>` (filter map, includes `transformed`).
- A node's edges are evaluated in declaration order; the first match wins; if none matches, that is a runtime error.
- The typed builder (ADR-0002) (de)serializes at the node boundaries; the interpreter feeds `Value` in directly; generated code uses the typed builder.
- Session state (prompt, model, tools) is **not** node data but part of the context (ADR-0020). Streaming is a side channel (ADR-0023).

## Consequences
One behavior for all entry points, checkpoints are trivial. Serialization cost per node transition (negligible compared to LLM latency); a later optimization is possible without breaking the API.
