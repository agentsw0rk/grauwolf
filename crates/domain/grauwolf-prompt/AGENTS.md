# grauwolf-prompt

**Layer:** `domain` · **Status:** `LlModel.fallbacks` (fallback models, not serialized when empty), `with_fallbacks(..)`, `without_fallbacks()` (ADR-0088); `LlModel.temperature` (0–2) and `think` (Ollama), `with_temperature`, `with_think`, both part of equality and hash (ADR-0097); tokens in advance (ADR-0087): port `Tokenizer` (`count(text)`), `EstimatingTokenizer` (one token per started 4 characters), `count_prompt(tokenizer, prompt)` (text, tool calls with name and argument JSON, tool results, +4 per message, +1000 per attachment, reasoning 0); `Message` (System; User with optional `attachments: Vec<Attachment { media_type, data: AttachmentData::{Base64, Url} }>` – empty ones are not serialized – `user_with`, `is_image`/`is_audio`/`is_pdf`, `to_url` (data URL); ADR-0081 –; Assistant built from parts `AssistantPart::{Text, ToolCall, Reasoning}` per ADR-0032; ToolResult; JSON with a `role` tag; `AssistantMessage::text()`/`tool_calls()`; `Reasoning { provider, data }` is opaque), `Prompt` with builder, `LlModel`; `Prompt.params: LlmParams { tool_choice: ToolChoice::{Auto, Required, None, Tool(name)} }` (serde default `Auto`; old prompts without `params` stay readable; ADR-0042). `Usage { input_tokens (incl. cached), output_tokens, cache_read_tokens?, cache_write_tokens? }` (ADR-0067; not part of `Message`; `+` sums, cache fields stay unknown only if both sides are unknown). `Prices { currency, models: { "<provider>:<id>": ModelPrice { input, output, cache_read?, cache_write? } } }` (per million tokens) with `cost(model, usage)` (uncached input, cache reads, cache writes, and output priced separately; cache prices fall back to the input price; unknown model → `None`; ADR-0071).

`Prompt::extend_system(addition)` appends text to the first system message (or adds one; ADR-0103).

## Purpose
Conversation model: Message/MessagePart, Prompt, LlModel, LlmParams, Capabilities, StreamFrame.

## Features / ADRs
E01 (F01.1–F01.9), E02 F02.14, F02.15 (parallel tool calls in the message model), ADR-0032, ADR-0042, ADR-0067, ADR-0071, ADR-0097

## Allowed dependencies
- grauwolf crates: none
- External: serde, serde_json

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- Pure data model: no I/O, no async runtime.
- All types are `Clone + Serialize + Deserialize`.
- Provider-specific things do not belong here (only the generic `extra` extension).

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-prompt`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
