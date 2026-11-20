# ADR-0049: OpenTelemetry – spans from agent events following the GenAI conventions

- **Status:** Accepted (User, 2026-11-08: "continue with OpenTelemetry (tracing)")
- **Date:** 2026-11-08

## Context
Agent runs should be exported as OpenTelemetry spans following the GenAI Semantic Conventions, also to Langfuse, W&B Weave and Datadog (F06.9–F06.11). grauwolf already reports all lifecycle points as `AgentEvent` (ADR-0007) and writes them as JSONL with `--events`.

## Decision
- New adapter crate `grauwolf-otel`: `OtelFeature` is a `Feature` that turns the events into spans (API crate `opentelemetry` 0.33, the tracer is passed in):
  - `invoke_agent <name>` for the run (`gen_ai.operation.name = invoke_agent`, `gen_ai.agent.name`),
  - `node <path>` per node (`grauwolf.node.path`), nested under the agent,
  - `chat <model>` per LLM call (`gen_ai.operation.name = chat`, `gen_ai.provider.name`, `gen_ai.request.model`, offered tools) under the active node,
  - `execute_tool <name>` per tool call (`gen_ai.tool.name`, `gen_ai.tool.call.id`), matched via the call ID,
  - errors set the span status; planner events become span events.
- Content (prompts, answers, tool arguments and results) only on explicit request (`with_content(true)`), because it can contain personal data.
- The facade sets up the export: `otel::init_from_env()` with OTLP over HTTP (`opentelemetry-otlp`), configured through the standard variables `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS`, `OTEL_SERVICE_NAME`. This makes Collector, Jaeger, Langfuse, Datadog and others work without dedicated presets. The CLI enables this with `--otel`.

## Consequences
- Spans are created without changes to the engine and nodes.
- LLM calls carry no ID; in parallel branches they are assigned to the most recently started open call and can appear swapped there.

## Addendum (2026-11-09): setup in the facade
There is no function `otel::init_from_env()`. The facade sets up the export with `grauwolf::otel::OtelTracing::init(endpoint: Option<&str>)`: without an endpoint according to the standard variables, otherwise to `<endpoint>/v1/traces`. `OtelTracing::feature(agent_name, content)` returns the feature per agent, `shutdown()` sends open spans. With `--otel` the CLI calls `OtelTracing::init(None)`; it records content with `--otel-content`.
