# E06 – Feature system & observability


| ID | Feature | Rust approach | Prio |
|----|---------|-------------|------|
| F06.1 | Installable features with configuration (`install(Feature) { ... }`) | `trait Feature: Send + Sync { fn install(self, pipeline: &mut Pipeline) }`; `builder.install(Tracing::new().with(..))` | M1 |
| F06.2 | Pipeline interceptors for all lifecycle points (agent, strategy, node, subgraph, LLM call, streaming, tool, planner) | `Pipeline` with lists of async hooks per event type; hooks receive `&EventContext` | M1 |
| F06.3 | Typed events (AgentStarting/Completed/Failed/Closing, StrategyStarting/Completed, NodeExecution*, Subgraph*, LLMCall*, LLMStreaming*, ToolCall*, ToolValidationFailed) | `enum AgentEvent` (serde-serializable) | M1 |
| F06.4 | Modifying interceptors (e.g. change the prompt before an LLM call, rewrite tool args) | Hooks with `&mut` on modifiable parts; order = installation order | M2 |
| F06.5 | Feature access from nodes (`featureOrThrow(X)`) | `ctx.feature::<T>() -> Option<&T>` (TypeId map) | M2 |
| F06.6 | Event filter per feature (`setEventFilter`) | Predicate closure in feature config | M2 |
| F06.7 | EventHandler feature (callbacks: onToolCall, onAgentFinished, …) | `EventHandler::new().on_tool_call_starting(|e| async {..})…` | M1 |
| F06.8 | Message processor/writer (log, file, remote) | `trait MessageProcessor`; log via `tracing`, file via JSONL | M2 |
| F06.9 | Tracing feature (complete execution trace) | `Tracing` feature that passes events to processors | M2 |
| F06.10 | OpenTelemetry (GenAI semantic conventions, spans for agent/node/LLM/tool, MCP attributes) | `opentelemetry` + `opentelemetry-otlp`; span hierarchy derived from pipeline events | M2 |
| F06.11 | Exporter presets: Langfuse, W&B Weave, Datadog | Config helper (endpoint + headers) via OTLP | M2 |
| F06.12 | Remote debugger (stream agent events via HTTP/SSE, a client reads along) | axum SSE server + client; own event format | M3 |
| F06.13 | Tokenizer feature (count tokens per message, for compression/limits) | `trait Tokenizer`; `tiktoken-rs` + fallback heuristic – ✅ ADR-0087: port `Tokenizer`, `EstimatingTokenizer` (without tiktoken), condition `on_history_tokens_above`, `limits.max_prompt_tokens` | M2 |

## Rust specifics
- Rust already has `tracing` as its ecosystem standard. Proposal: a custom event system **in addition to** `tracing` – pipeline events are structured, typed data (for persistence, debugger, tests), while `tracing` spans serve observability. The OTel feature can build on `tracing-opentelemetry` or use the OTel API directly (open).
