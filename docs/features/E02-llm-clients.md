# E02 – LLM clients, executor, streaming


| ID | Feature | Rust approach | Prio |
|----|---------|-------------|------|
| F02.1 | LLM client abstraction (execute, executeStreaming, executeMultipleChoices, moderate, embed, models) | `trait LlmClient: Send + Sync` with `async fn`; streaming returns `BoxStream<'static, Result<StreamFrame>>` | M1 |
| F02.2 | OpenAI client (Chat Completions + Responses API) | `reqwest` + own DTOs | M1 |
| F02.3 | Anthropic client | own DTOs, SSE | M1 |
| F02.4 | Ollama client (incl. model pull/listing) | own DTOs | M1 |
| F02.5 | Google Gemini client | own DTOs – ✅ ADR-0083: `grauwolf-gemini` (tools, thought signatures, streaming, structured, attachments, embeddings), provider `gemini` | M2 |
| F02.6 | OpenRouter, DeepSeek, DashScope, Mistral clients | Reuse of the OpenAI base – ✅ ADR-0082: `openrouter`, `deepseek`, `mistral`, `groq`, `dashscope`, `xai` with `<NAME>_API_KEY`/`<NAME>_BASE_URL`, embeddings too | M2 |
| F02.7 | AWS Bedrock client (Converse/InvokeModel) | ✅ ADR-0105 (F14.18): `grauwolf-bedrock` on the Converse API with its own SigV4 signing and event-stream decoder instead of the AWS SDK; provider `bedrock` | M2 |
| F02.8 | LiteRT (on-device) | **dropped** (Android-specific) | – |
| F02.9 | PromptExecutor (single client) | `struct SingleLlmExecutor` | M1 |
| F02.10 | MultiLLMPromptExecutor (routing by provider, fallback provider/model) | `HashMap<Provider, Arc<dyn LlmClient>>` + fallback config – ✅ ADR-0088: `fallbacks` per model in the spec, switch in the `AgentContext` with event `llm_fallback` | M1 |
| F02.11 | Convenience factories (`simpleOpenAIExecutor(key)` …) | `grauwolf::executors::openai(key)` | M1 |
| F02.12 | Retry with backoff + error patterns | Decorator `RetryingClient<C>`; backoff with jitter; patterns as a predicate on the error enum | M2 |
| F02.13 | Response caching (memory, files, Redis) | `trait PromptCache` + `CachedExecutor`; backends: in-memory, file, `redis` (feature flag) – ✅ ADR-0086: port `PromptCache` in the `PromptExecutor`, `MemoryPromptCache`, `FilePromptCache`, CLI `--llm-cache` (no Redis) | M2 |
| F02.14 | Streaming frames (TextDelta, ReasoningDelta, ToolCallDelta, complete frames, end with usage) | `enum StreamFrame`; aggregation into `Message` via helper | M1 |
| F02.15 | Parallel tool calls in the response | represented in the message model | M1 |
| F02.16 | Embedding endpoints (OpenAI, Ollama, Google, Bedrock, Mistral) | `trait EmbeddingClient` | M2 |
| F02.17 | Model-specific params (reasoning effort, thinking budget, web search …) | Typed extension structs per provider, serialized into `LlmParams.extra` | M2 |
| F02.18 | Swappable HTTP client | **dropped** – only `reqwest` | – |
| F02.19 | Token usage/cost reporting | Usage in `ResponseMeta` | M1 |

## Rust specifics
- **Decided:** own provider integrations (own DTOs, no third-party crates). Full control over cache control, reasoning, tool formats. M1: OpenAI, Anthropic, Ollama.
