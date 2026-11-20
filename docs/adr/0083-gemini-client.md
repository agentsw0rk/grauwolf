# ADR-0083: Gemini client

- **Status:** Accepted (User, 2026-11-09: "next, do Gemini")
- **Date:** 2026-11-09

## Context
Google Gemini is missing as a provider (F02.5). Gemini's OpenAI-compatible interface only partly covers thought signatures, attachments as `fileData`, and embeddings; the native API `generateContent` is stably documented.

## Decision
- New adapter crate `grauwolf-gemini` with its own DTOs (ADR-0005), provider name `gemini`, key `GEMINI_API_KEY`, address optionally `GEMINI_BASE_URL` (default `https://generativelanguage.googleapis.com`). Header `x-goog-api-key`.
- **Requests:** `POST /v1beta/models/{model}:generateContent` (the `models/` prefix in the model ID is removed). Leading system messages → `systemInstruction`, later ones as user text. Roles `user`/`model`. Tools as `functionDeclarations`; tool results as `functionResponse` with `response: { "result": <text> }`, consecutive ones in one content.
- **Tool calls:** `functionCall` → `ToolCall`. If the `id` is missing, the adapter assigns `gemini-call-<n>` and does not send it back.
- **Thought signatures:** Gemini requires that `thoughtSignature` comes back in the history at the same position. The adapter stores it as `Reasoning { provider: "gemini", data: { "thoughtSignature": … } }` directly before the part it belongs to, and re-attaches it to the following part when sending. Thought parts (`thought: true`) are kept as `Reasoning` with the whole part and returned unchanged. Reasoning of other providers is dropped.
- **Usage:** `usageMetadata`: input `promptTokenCount`, output `candidatesTokenCount + thoughtsTokenCount`, `cachedContentTokenCount` as the cache read (ADR-0067).
- **Blocked:** `promptFeedback.blockReason`, or a candidate without content with a `finishReason` such as `SAFETY` → `LlmError::Client` with the reason.
- **Tool choice** (ADR-0042): `toolConfig.functionCallingConfig.mode` `AUTO`/`ANY`/`NONE`; `Tool(name)` → `ANY` with `allowedFunctionNames`.
- **Structured** (ADR-0031): natively via `generationConfig.responseMimeType: application/json` and `responseJsonSchema`.
- **Streaming** (ADR-0033): `:streamGenerateContent?alt=sse`; text immediately, function calls whole, usage from the last event before `End`.
- **Attachments** (ADR-0081): all media types; Base64 → `inlineData`, URL → `fileData.fileUri`. Whether Gemini understands a type is reported by the service.
- **Embeddings** (ADR-0038): `:batchEmbedContents`.
- Facade: Cargo feature `gemini` (on by default), name in `providers::BUILTIN`.

## Consequences
- Without a Gemini key in the development environment, the tests are written against simulated responses in the documented format; the live test with the conformance suite is opt-in (`GRAUWOLF_GEMINI_API_KEY`).
- Vertex AI (other addresses and authentication) is not part of this decision.
