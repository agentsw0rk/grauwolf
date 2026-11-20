# grauwolf-ollama

**Layer:** `adapters` · **Status:** `OllamaClient::model_digest(name)`: digest of an installed model from `GET /api/tags` (exact name or `<name>:latest`, case-insensitive; `None` if absent or unreachable, 2 s timeout; ADR-0094); `Completion.model` from the response's `model`; Attachments (ADR-0081): images as base64 in the `images` of the user message; anything else, or images given as a URL, is rejected by `check_attachments` with `LlmError::Client` before any request. `OllamaClient::new(base_url)` (`DEFAULT_BASE_URL`) implements `LlmClient::execute` via `/api/chat` (text messages, `stream: false`, tools as `tools`; the response becomes text (omitted if empty and there are tool calls) and all tool calls as `ToolCall` parts; without an ID the tool name serves as the ID; in history, responses become `assistant` messages with the text and all `tool_calls` (reasoning from other providers is not sent), and results become `role: tool` messages with `tool_name`); HTTP errors via `LlmError::from` (transient errors are retried by the executor). Tests against recorded responses (wiremock). Live test `tests/live.rs` with the conformance suite (opt-in: `GRAUWOLF_OLLAMA_URL`, `GRAUWOLF_OLLAMA_MODEL`, default `qwen3.5:9b`) – passed against Ollama 0.40.14 with `qwen3.5:9b`. `execute_streaming` is real over NDJSON (`stream: true`; text deltas, tool calls, the `model` of the last line as `StreamFrame::Model`, `End` with `done_reason`; ADR-0094). `LlModel.temperature` goes to `options.temperature` and `LlModel.think` to `think` (taking precedence over the client; ADR-0097); `with_think(bool)` sets `think` in all requests (reasoning models such as qwen3.5 respond many times faster without thinking; without the call the server decides). Tool choice (`prompt.params.tool_choice`, ADR-0042) is approximated: `Tool(name)` offers only that tool, `None` offers no tools, `Required`/`Tool` append a system instruction (`chat_request`). `execute_structured` is native via `format` (JSON schema, without tools and without a system instruction, ADR-0031). `OllamaEmbedder::new(base_url, model)` implements `grauwolf_rag::Embedder` via `/api/embed` (all texts in one request; errors → `RagError::Embedding`, ADR-0038). `execute_with_usage` reports `prompt_eval_count`/`eval_count` as usage (without cache information; likewise `execute_structured_with_usage`; in the stream, from the line with `done: true` as `StreamFrame::Usage` before `End`; ADR-0067).

## Purpose
LlmClient for Ollama (chat, model listing/pull) with its own DTOs.

## Features / ADRs
E02 F02.4, ADR-0031, ADR-0038, ADR-0042, ADR-0067, ADR-0094

## Allowed dependencies
- grauwolf crates: grauwolf-llm, grauwolf-llm-http, grauwolf-prompt, grauwolf-rag
- External: serde, serde_json, futures-util; dev: grauwolf-test (conformance suite), tokio, wiremock

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- Passes the LlmClient conformance suite.
- Integration tests against a real Ollama are opt-in only.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-ollama`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
