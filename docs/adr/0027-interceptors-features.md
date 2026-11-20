# ADR-0027: Interceptors as fold, feature catalog, tool-set enforcement

- **Status:** Accepted (review iteration 1, 2026-11-06)
- **Date:** 2026-11-06
- **Partially superseded by:** ADR-0047, ADR-0061 (see addendum)

## Context
Modifying interceptors with `&mut` in async `dyn` hooks are hard to get right with lifetimes. Subgraph tool subsets must also restrict execution. Features in specs need a description.

## Decision
- Modifying hooks receive owned data and return it (`async fn(LlmCallStarting) -> Result<LlmCallStarting>`), applied as a fold in installation order.
- The environment checks every tool call against the active tool set of the subgraph.
- Features have a `FeatureDescriptor { id, config_schema }`; specs reference features through it.

## Consequences
Simple, safe hook signatures; subgraph isolation also holds at runtime.

## Addendum (2026-11-09): fold and feature catalog superseded
- The fold with owned data (`async fn(LlmCallStarting) -> Result<LlmCallStarting>`) is not implemented. The hooks that intervene are the trait `Interceptor` with synchronous `&mut` hooks (ADR-0047); for asynchronous decisions there is `approve_tool_call(&ToolCall) -> BoxFuture<ToolDecision>` (ADR-0061). Multiple interceptors act in installation order.
- A feature catalog with `FeatureDescriptor { id, config_schema }` does not exist. Features are code (`grauwolf_agents::events::Feature` with `on_event`, e.g. `OtelFeature`, ADR-0049); specs do not reference features but configure capabilities through their own strictly checked sections (e.g. `long_term_memory`, `self_tools`, `limits`).
- Still valid: the `AgentContext` checks every tool call against the active tool subset (`is_tool_allowed`, ADR-0034).
