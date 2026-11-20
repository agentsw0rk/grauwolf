# grauwolf-llm

**Layer:** `domain` · **Status:** Response cache (ADR-0086): port `PromptCache` (`get(key) -> Option<Message>`, `put`; the implementation handles errors), `MemoryPromptCache`, `PromptExecutorBuilder::cache`; the key = canonical JSON of kind (`execute`, `structured`), provider, model, `temperature`/`think` of the model (only when set, ADR-0097), prompt, tools, schema – cached are `execute`/`execute_with_usage`, `execute_structured`(`_with_usage`) and `execute_streaming` (key as for `execute`; hits as frames with usage 0; new responses are stored after `End` via `aggregate`; nothing after an error), not `execute_choices` and not moderation; hits report usage 0 and do not query the provider. Port `LlmClient` (BoxFuture; prompt, model, and offered tools); `PromptExecutor` with routing by provider (`UnknownProvider` if no client is registered). Structured responses via `execute_structured(.., &StructuredOutput)`; default = schema as a system instruction (ADR-0031); adapters may override natively. Streaming (ADR-0033): `StreamFrame::{TextDelta, ToolCall, Reasoning, Usage, Model, End}` (`Usage` before `End`, ADR-0067; `Model(name)` = the model the provider reports, anywhere before `End`, ignored by `aggregate`, replayed from `Completion.model` by the default streaming, not stored by the prompt cache, ADR-0094; sinks that only look at `TextDelta` see no change); `LlmClient::execute_streaming` with a default implementation via `execute_with_usage` (one response in one piece, usage as a `Usage` frame); `aggregate(frames)` reassembles the response. `PromptExecutor::execute_streaming` routes and retries transient errors only before the first frame. `RetryPolicy::next_delay(attempt, retry_after)` (exponential from `initial_delay`, capped by `max_delay`, a server hint takes precedence, default 3 retries). `LlmError::Transient { source, retry_after }` for transient errors; `LlmError::BudgetExceeded { reason }` (ADR-0072, never retried). `PromptExecutor::builder().retry(policy, clock)` retries only those and waits via the port `Clock` (it lives here rather than in grauwolf-agents, because the executor needs it; without a clock there is no retry). Token usage (ADR-0067): `LlmClient::execute_with_usage` → `Completion { message, usage: Option<Usage>, model: Option<String> }` (`model`: the model the provider reports, filled by the OpenAI, Anthropic, Gemini and Ollama adapters from their responses, ADR-0094) (default: `execute`, usage unknown); `PromptExecutor::execute_with_usage` with retries; likewise `execute_structured_with_usage` (default: `execute_structured`, so native paths are kept). Moderation (ADR-0066): `LlmClient::moderate(text, model) → Moderation { harmful, categories, usage }`; default = its own classification request via `execute_structured_with_usage` (reports its usage, ADR-0067 addendum) (fixed prompt, code blocks allowed, an unusable verdict → `LlmError::Client`); `PromptExecutor::moderate` with retries; `classify_moderation(client, text, model)` is public as a fallback for adapters with their own API. `LlmClient::forces_tool_choice(model)` (default `true`) and `PromptExecutor::forces_tool_choice(model)` state whether the provider can force tool calls (ADR-0063). `execute_choices(prompt, model, tools, n)` returns `Choices { messages, usage }` – n alternative responses with usage (default: n single requests via `execute_with_usage`, usage summed, unknown as soon as one is missing; ADR-0067 addendum; `PromptExecutor::execute_choices` with retries; ADR-0042).

## Purpose
Port `LlmClient` (execute, stream, …) and executor logic: routing by provider, fallback, retry policy – without HTTP.

## Features / ADRs
E02 (F02.1, F02.9–F02.12, F02.19), F01.10; ADR-0031, ADR-0033, ADR-0042, ADR-0063, ADR-0066, ADR-0067, ADR-0072, ADR-0086

## Allowed dependencies
- grauwolf crates: grauwolf-prompt
- External: futures-util, serde_json (schemas), thiserror; dev: futures-executor

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- No HTTP and no tokio – transport lives in adapters/.
- The retry policy only provides `next_delay`; waiting goes through the port `Clock` (ADR-0026).
- At `dyn` boundaries: `BoxFuture`/`BoxStream<'static, _>`.
- Every LlmClient adapter must pass the conformance suite in grauwolf-test.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-llm`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
