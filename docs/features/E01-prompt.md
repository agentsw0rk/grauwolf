# E01 – Prompt & message model


| ID | Feature | Rust approach | Prio |
|----|---------|-------------|------|
| F01.1 | Message types (System, User, Assistant, Tool-Call, Tool-Result, Reasoning) with parts | `enum Message` + `enum MessagePart`, `serde`-tagged | M1 |
| F01.2 | Metadata (token usage, timestamps, finish reason) | Structs, `chrono`/`time` | M1 |
| F01.3 | Prompt (ID, messages, params) | `struct Prompt` (Clone, Serialize) | M1 |
| F01.4 | Prompt DSL (`prompt("id") { system(); user(); assistant() }`) | Fluent builder: `Prompt::builder("id").system("..").user("..").build()`; optional `prompt!` macro | M1 |
| F01.5 | Multimodal content (image, audio, video, file – URL/bytes/Base64) | `enum Attachment { Image, Audio, Video, File }` with `enum Source { Url, Bytes, Base64 }` – ✅ phase 1 (ADR-0081): `Attachment { media_type, data: Base64 \| Url }` on user messages; images, PDF, audio (OpenAI); video open | M2 |
| F01.6 | Cache control (Anthropic/Bedrock prompt caching) | Field on messages/parts | M2 |
| F01.7 | LLM parameters (temperature, maxTokens, toolChoice, schema, speculation, user, include-thoughts…) | `struct LlmParams` with `Option<_>` fields + provider-specific extension (`extra: serde_json::Map`) | M1 |
| F01.8 | Models & provider catalog | `struct LlModel { provider, id, capabilities, context_length, max_output }` + `const` catalogs per provider | M1 |
| F01.9 | Model capabilities (tools, vision, audio, schema, embed, moderation, speculation…) | `enum Capability` + `HashSet`/bitflags; checked before the request | M1 |
| F01.10 | Structured output – schema from type | `schemars::JsonSchema` derive; strategies *native* (provider JSON schema) vs. *manual* (schema in the prompt) | M2 |
| F01.11 | Structured output – parsing + fix retry (fixing parser via LLM) | `serde_json` + optional retry with a correction prompt | M2 |
| F01.12 | Markdown/XML text builders for prompts | Small builder utilities (`MarkdownBuilder`, `XmlBuilder`) | M3 |
| F01.13 | Streaming Markdown parser for structured data | Parser over `Stream<Item = StreamFrame>` | M3 |
| F01.14 | Prompt processor (pre-/post-processing of prompts) | `trait PromptProcessor` | M3 |
| F01.15 | Moderation API (content moderation) | `trait Moderator`, provider implementation (OpenAI) | M3 |

## Rust specifics
