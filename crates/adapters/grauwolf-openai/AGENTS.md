# grauwolf-openai

**Layer:** `adapters` · **Status:** `LlModel.temperature` is sent (ADR-0097): `temperature` (Chat Completions, Messages API) or `generationConfig.temperature` (Gemini); `think` is Ollama-only. Attachments (ADR-0081): user messages with attachments become content parts (`Content::Parts`) – images as `image_url` (URL or data URL), audio wav/mp3 as `input_audio` (base64), PDF as `file` (base64); anything else, or audio/PDF given as a URL, is rejected by `dto::messages` before any request with `LlmError::Client`. `OpenAiClient::new(api_key)` (`DEFAULT_BASE_URL`, `with_base_url` for compatible gateways/tests) implements `LlmClient::execute` via `/chat/completions` (bearer token; tools as `tools`; response → text (omitted if empty and there are tool calls) and all tool calls as `ToolCall` parts with the real ID, arguments parsed from the JSON text; in history, `assistant` with text (`null` for pure tool calls) and all `tool_calls`, or `role: tool` with `tool_call_id`; reasoning from other providers is not sent); HTTP errors via `LlmError::from` (transient errors are retried by the executor). Tests against recorded responses (wiremock). `execute_streaming` is real over SSE (`stream: true`; text deltas immediately; tool-call fragments collected per `index` and emitted as complete calls at `[DONE]` or the end of the stream, then `End` with `finish_reason`; the chunk's `model` is emitted once as `StreamFrame::Model`, ADR-0094). `execute_structured` is native via `response_format` (`json_schema` with name and schema, without `strict`; ADR-0031). Live test `tests/live.rs` with the conformance suite (opt-in: `GRAUWOLF_OPENAI_API_KEY`, optionally `GRAUWOLF_OPENAI_BASE_URL`, `GRAUWOLF_OPENAI_MODEL`, default `gpt-4o-mini`) – passed against Ollama's OpenAI-compatible API (`/v1`, `qwen3.5:9b`). `prompt.params.tool_choice` → `tool_choice` (`required`/`none`/`{type: function, function: {name}}`, only with tools; `Auto` is not sent; ADR-0042). `execute_with_usage` reports `usage` (`prompt_tokens` including cached, `prompt_tokens_details.cached_tokens` as the cache read; likewise `execute_structured_with_usage`; streaming requests `stream_options.include_usage` and emits the usage from the last chunk as `StreamFrame::Usage` before `End` – also live with Ollama's `/v1`; ADR-0067). `execute_choices` is native via `n` (usage from the response's `usage` as `Choices.usage`; `/moderations` reports no usage) (all `choices` as responses). `moderate` is native via `/moderations` only with a moderation model (ID contains `moderation`; `flagged` → `harmful`, set `categories` as names); otherwise `grauwolf_llm::classify_moderation` via the chat model (ADR-0066). `OpenAiEmbedder::new(api_key, model).with_base_url(..)` implements `grauwolf_rag::Embedder` via `/embeddings` (ordered by `index`).

## Purpose
LlmClient for OpenAI (Chat Completions, Responses API) with its own DTOs.

## Features / ADRs
E02 F02.2, ADR-0031, ADR-0042, ADR-0066, ADR-0067

## Allowed dependencies
- grauwolf crates: grauwolf-llm, grauwolf-llm-http, grauwolf-prompt, grauwolf-rag
- External: serde, serde_json, futures-util; dev: grauwolf-test, tokio, wiremock

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- Passes the LlmClient conformance suite.
- DTO serialization is protected by snapshot tests.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-openai`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
