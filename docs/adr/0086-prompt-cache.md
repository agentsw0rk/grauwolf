# ADR-0086: Response cache for LLM requests

- **Status:** Accepted (User, 2026-11-09: "do 2. response cache for LLM requests (F02.13), in memory or as a file")
- **Date:** 2026-11-09

## Context
Repeated identical requests (development, tests, demos, runs started again) cost time and money every time. (F02.13)

## Decision
- **Port** `PromptCache` in `grauwolf-llm`: `get(key) -> Option<Message>`, `put(key, message)`; the implementation handles errors itself: a cache may never make a request fail.
- **Location:** in the `PromptExecutor` (`builder().cache(cache)`), before the retries. This way it works the same for the interpreter, generated code, sub-agents and auxiliary requests (compression, facts).
- **Key:** canonical JSON made of the request kind, the provider, the model ID, the entire prompt (messages, parameters, attachments), the offered tools and, for structured requests, the schema. Any change to these is a new key.
- **Cached:** `execute`/`execute_with_usage`, `execute_structured`(`_with_usage`) and `execute_streaming` (same key as `execute`; hits are replayed as frames; failures are stored after the end, aggregated). Not cached: `execute_choices` ("best of n" wants variety) and moderation.
- **Usage on hits:** zero tokens: nothing was consumed; budget and costs count nothing.
- **Implementations:** `MemoryPromptCache` (domain, for one process), `grauwolf::cache::FilePromptCache` (facade; one file per key under `<dir>/<fnv1a>.json` with key and response, written atomically; another key in the file counts as a miss).
- **CLI:** `--llm-cache <dir>` for `run`, `chat`, `serve`, `mcp`, `resume`.
- No expiry date in phase 1; the folder can be deleted at any time.

## Consequences
- Same input → same response, even with models that involve randomness. Desired for development and tests; enable it deliberately in operation.
- Conversations hit the cache only if the entire history is the same.
