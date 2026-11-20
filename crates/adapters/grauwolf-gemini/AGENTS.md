# grauwolf-gemini

**Layer:** `adapters` · **Status:** `LlModel.temperature` is sent (ADR-0097): `temperature` (Chat Completions, Messages API) or `generationConfig.temperature` (Gemini); `think` is Ollama-only. Attachments (ADR-0081): all media types before the text, base64 as `inlineData`, URL as `fileData.fileUri` (whether Gemini understands the type is reported by the service). `GeminiClient::new(api_key)` (`DEFAULT_BASE_URL`, `with_base_url`) implements `LlmClient::execute` via `/v1beta/models/{model}:generateContent` (header `x-goog-api-key`; the `models/` prefix of the model ID is dropped; leading system messages → `systemInstruction`, later ones as user text; roles `user`/`model`; tools as `functionDeclarations` with `parametersJsonSchema`; `functionCall` → `ToolCall`; without `id` as `gemini-call-<n>`, such IDs are not sent back; results as `functionResponse` with `response: { result }`, consecutive ones in one content; thought parts (`thought: true`) → `Reasoning { provider: "gemini", data: <part> }`; `thoughtSignature` → its own `Reasoning { data: { thoughtSignature } }` directly before the part – when sending, thoughts come back unchanged and signatures come back on the following part (without a following part, on an empty text); reasoning from other providers is dropped); tool choice (`prompt.params.tool_choice`, ADR-0042, only with tools) → `toolConfig.functionCallingConfig` (`ANY`, `NONE`, `Tool(name)` as `ANY` with `allowedFunctionNames`; `Auto` is not sent). `execute_streaming` is real via `streamGenerateContent?alt=sse` (text immediately, function calls whole, signatures/thoughts as `Reasoning`; `modelVersion` once as `StreamFrame::Model`, ADR-0094; after the last event, usage from the last `usageMetadata` as `StreamFrame::Usage`, and `End` with the last `finishReason`; after an error, no further frames). `execute_structured` is native via `generationConfig` (`responseMimeType: application/json`, `responseJsonSchema`, without tools; ADR-0031). `execute_with_usage` reports `usageMetadata` (input `promptTokenCount`, output `candidatesTokenCount + thoughtsTokenCount`, `cachedContentTokenCount` as the cache read; ADR-0067). Blocked (`promptFeedback.blockReason`, a candidate without parts and with a `finishReason` other than `STOP`/`MAX_TOKENS`) or no candidates → `LlmError::Client` with the reason. HTTP errors via `LlmError::from`. `GeminiEmbedder::new(api_key, model)` (`with_base_url`) implements `grauwolf_rag::Embedder` via `:batchEmbedContents` (all texts in one request; errors → `RagError::Embedding`, ADR-0038). Tests against reproduced responses (wiremock). Live test `tests/live.rs` with the conformance suite (opt-in: `GRAUWOLF_GEMINI_API_KEY`, optionally `GRAUWOLF_GEMINI_MODEL`, default `gemini-2.5-flash`) – not yet run against the real API.

## Purpose
LlmClient for the native Gemini API with its own DTOs (ADR-0083).

## Features / ADRs
E02 F02.5, E08 F08.2, ADR-0038, ADR-0083

## Allowed dependencies
- grauwolf crates: grauwolf-llm, grauwolf-llm-http, grauwolf-prompt, grauwolf-rag
- External: serde, serde_json, futures-util; dev: grauwolf-test (conformance suite), tokio, wiremock

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- Passes the LlmClient conformance suite (live, opt-in).
- Integration tests against the real API are opt-in only.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-gemini`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
