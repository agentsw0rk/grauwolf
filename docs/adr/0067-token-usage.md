# ADR-0067: Token usage per LLM call

- **Status:** Accepted (self-prioritized; User, 2026-11-08: "then continue"; costs matter to the user)
- **Date:** 2026-11-08

## Context
So far no adapter reports how many tokens a call consumes. The costs of a run are not traceable, and whether the prompt cache (ADR-0062) hits can only be checked from outside grauwolf. All providers include the numbers in their response (Anthropic `usage` incl. cache fields, OpenAI `usage` incl. `prompt_tokens_details.cached_tokens`, Ollama `prompt_eval_count`/`eval_count`).

## Decision
- `grauwolf_prompt::Usage { input_tokens, output_tokens, cache_read_tokens, cache_write_tokens }` (cache fields optional). `input_tokens` includes all input tokens, including the cached ones, so that the number means the same across providers; Anthropic reports the cached ones separately and they are converted.
- Port: `LlmClient::execute_with_usage(prompt, model, tools) → Completion { message, usage: Option<Usage> }`. Default: call `execute`, usage unknown, so existing clients and mocks remain valid. Adapters that know the numbers override the method and derive `execute` from it. `PromptExecutor::execute_with_usage` routes and retries as usual.
- The usage does not belong to the `Message`: otherwise it would end up in the stored history and be sent back to providers.
- `AgentEvent::LlmCallCompleted` carries `usage` (absent in the JSON if unknown). OpenTelemetry sets `gen_ai.usage.input_tokens`, `gen_ai.usage.output_tokens`, `gen_ai.usage.cache_read.input_tokens` and `gen_ai.usage.cache_creation.input_tokens`.

## Consequences
- Costs and cache hits are visible in the event log and in tracing.
- Added (User 2026-11-08: "keep going"): `execute_structured_with_usage` for structured responses (the default calls `execute_structured`, so native paths are preserved); `StreamFrame::Usage` before `End` for streaming (the default path without real streaming passes on the usage from `execute_with_usage`; OpenAI requests `stream_options.include_usage` for it, which Ollama's `/v1` also understands); `AgentContext::usage()` sums all requests of a run, including forks of parallel branches (shared counter); `agent_completed`/`agent_failed` carry the sum, and OpenTelemetry sets it on the `invoke_agent` span.
- Still without usage: multiple response variants (`execute_choices`) and moderation; after resuming a checkpoint, the sum only counts from there on.

## Addendum: Moderation and multiple responses (User, 2026-11-09: "make costs and budget work for moderation and best-of too")
Moderation (ADR-0066) and multiple response variants (`llm_request_best_of`, ADR-0042) reported no usage and therefore counted toward neither the sum nor costs and budget (ADR-0071, ADR-0072).
- `Moderation` carries `usage: Option<Usage>`. The default moderation (structured classification) reports the usage of its request; a dedicated moderation API (OpenAI) reports none, because it does not charge tokens.
- `LlmClient::execute_choices` returns `Choices { messages, usage }` instead of only the responses. Default: `n` requests via `execute_with_usage`, usage summed (unknown as soon as one request reports none); OpenAI reads the `usage` of the native response with `n`.
- `AgentContext` adds both to the sum and calculates costs; for moderation, with the price of the moderation model, which may differ from the active one. `moderation_completed` carries `usage`/`cost`; with multiple responses, usage and costs are on the event of the first response.
