# grauwolf-otel

**Layer:** `adapters` · **Status:** `gen_ai.response.model` from `llm_call_completed.model` (ADR-0094); `llm_fallback` as a span event on the open LLM call (`grauwolf.fallback.from`/`.to`, `error.message`; ADR-0088); `OtelFeature::new(BoxedTracer, agent_name)` is a `Feature`: `invoke_agent <name>` → `node <path>` → `chat <model>` / `execute_tool <name>` (GenAI attributes `gen_ai.operation.name`, `gen_ai.agent.name`, `gen_ai.provider.name`, `gen_ai.request.model`, `gen_ai.tool.name`, `gen_ai.tool.call.id`; `grauwolf.node.path`, `grauwolf.tools`); errors as span status (tool results with "Error:"); costs as `grauwolf.cost` on the chat and agent spans (ADR-0071); token usage on the chat span and as a sum on the `invoke_agent` span (`gen_ai.usage.input_tokens`, `.output_tokens`, `.cache_read.input_tokens`, `.cache_creation.input_tokens`; ADR-0067); planner events and automatic memory (`memory_recalled`/`memory_remembered` with `grauwolf.memory.facts`, `memory_failed`; ADR-0069; `knowledge_injected` with `grauwolf.knowledge.sections`, `knowledge_failed`; ADR-0113) as span events; moderations as `moderate <model>` under the node (`gen_ai.operation.name = moderation`, provider, model, `grauwolf.moderation.harmful`/`.categories`, usage and `grauwolf.cost` as for `chat`, errors as status; the moderated text is never recorded, not even with `with_content`; ADR-0066); decisions as `decide <node>` under the node (`grauwolf.decision.node`/`.answer`/`.probability`/`.confidence`/`.model`; errors as status; the LLM call of a model-based decider is nested in it; the state is never recorded; ADR-0100); `with_content(true)` records responses, tool arguments/results, and plans. The end of an agent closes all open spans.

## Purpose
OpenTelemetry tracing for agent runs (E06 F06.9, F06.10).

## Features / ADRs
E06 F06.9–F06.11, ADR-0049, ADR-0066, ADR-0100, ADR-0067, ADR-0069, ADR-0071

## Allowed dependencies
- grauwolf crates: grauwolf-agents, grauwolf-prompt
- External: opentelemetry (API, trace), opentelemetry_sdk (only `propagation::TraceContextPropagator` for the W3C headers, ADR-0123), serde_json; dev: opentelemetry_sdk (testing)

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- The application sets up the exporter and SDK (facade `grauwolf::otel`); this crate uses the SDK only for the W3C `TraceContextPropagator` (nowhere else in the workspace, ADR-0123).
- `Feature::trace_headers(call)` gives `traceparent`/`tracestate` of the open tool call span `call` (or of the innermost open node or the agent) for calls out of the run (A2A, MCP).
- Content only with `with_content(true)`.
- LLM calls have no ID; they are attributed to the most recently started open call (imprecise in parallel branches).

## Tests
- Against the SDK's `InMemorySpanExporter` with a fixed event sequence.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-otel`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.

`agent_starting.on_behalf_of` (ADR-0111) becomes `grauwolf.on_behalf_of.oid`, `.name`, `.upn`, `grauwolf.txn` and, for a grant run, `grauwolf.grant` on the `invoke_agent` span; the Txn-Token itself never reaches the feature.
