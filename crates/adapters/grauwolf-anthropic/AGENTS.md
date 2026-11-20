# grauwolf-anthropic

**Layer:** `adapters` · **Status:** `LlModel.temperature` is sent (ADR-0097): `temperature` (Chat Completions, Messages API) or `generationConfig.temperature` (Gemini); `think` is Ollama-only. Attachments (ADR-0081): images as `image` blocks and PDFs as `document` blocks (base64 or URL) before the text; anything else (e.g. audio) is rejected by `check_attachments` with `LlmError::Client` before any request. `AnthropicClient::new(api_key)` (`DEFAULT_BASE_URL`, `with_base_url`, `with_max_tokens`, default 16000) implements `LlmClient::execute` via `/v1/messages` (header `x-api-key`, `anthropic-version: 2026-10-21`; every request carries `cache_control: { type: ephemeral }` at the top level (automatic prompt cache, 5 min; ADR-0062); leading system messages → `system` (one text block with its own cache breakpoint, so conversations with the same start share tools and system prompt; ADR-0062); later ones stay in the history; tools with `input_schema`; response: all blocks in order as parts (`text`, `tool_use` → `ToolCall`, `thinking`/`redacted_thinking` → `Reasoning { provider: "anthropic", data: <block> }`, unknown block types are dropped); history: pure text responses as a string, otherwise blocks `text`/`tool_use` and our own thinking blocks unchanged, in the order of the parts; reasoning from other providers is dropped; results as `tool_result`, consecutive results (parallel calls) in one user message); HTTP errors via `LlmError::from` (transient errors are retried by the executor). Tests against recorded responses (wiremock). `execute_streaming` is real over SSE (`stream: true`; `text_delta` immediately, `input_json_delta` and `thinking_delta`/`signature_delta` collected per block until `content_block_stop` – thinking blocks in the same form as without streaming; `message_start` provides the reported model as `StreamFrame::Model` (ADR-0094); `message_delta` provides the reason for `End`, `refusal` becomes an error; `error` events: overload/rate limit/`api_error` → `Transient`, otherwise `Client`). `execute_structured` is native via `output_config.format` (`json_schema`; every object gets `additionalProperties: false`, ADR-0031). `execute_with_usage` reports `usage` (`input_tokens` = uncached remainder + `cache_creation_input_tokens` + `cache_read_input_tokens`, cache fields reported separately; likewise `execute_structured_with_usage`; in the stream, input from `message_start`, output from `message_delta`, as `StreamFrame::Usage` before `End`; ADR-0067). Refusals (`stop_reason: refusal`) become `LlmError::Client` with category and reason. Live test `tests/live.rs` with the conformance suite (opt-in: `GRAUWOLF_ANTHROPIC_API_KEY`, optionally `GRAUWOLF_ANTHROPIC_MODEL`, default `claude-opus-5-5`) – live green with `claude-haiku-5-5` (2026-11-08). `prompt.params.tool_choice` → `tool_choice` (`{type: any|none|tool, name}`, only with tools; `Auto` is not sent; ADR-0042); `forces_tool_choice` is `false` for `claude-fable-5-1`, `claude-mythos-5-1`, `claude-opus-5-5`, `claude-sonnet-5-5` (also with prefix/suffix), which reject `any`/`tool` – there the agent steers via an instruction (ADR-0063).

## Purpose
LlmClient for Anthropic (Messages API, SSE) with its own DTOs.

## Features / ADRs
E02 F02.3, ADR-0031, ADR-0042, ADR-0062, ADR-0063, ADR-0067

## Allowed dependencies
- grauwolf crates: grauwolf-llm, grauwolf-llm-http, grauwolf-prompt
- External: serde, serde_json, futures-util; dev: grauwolf-test, tokio, wiremock

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- Passes the LlmClient conformance suite.
- DTO serialization is protected by snapshot tests.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-anthropic`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
