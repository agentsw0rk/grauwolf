# ADR-0058: Long-term memory through the tools `remember` and `recall`

- **Status:** Accepted (User, 2026-11-08: "continue with long-term memory")
- **Date:** 2026-11-08

## Context
Chat memory (ADR-0037) keeps the history of *one* conversation. What an agent should know across conversations (preferences, facts, decisions) is lost or would have to be dragged along as an entire history.

## Decision
- New spec section `long_term_memory` with `store` (JSON file relative to the spec), `embedding_model`, `top_k` (default 3) and the tool names `remember_tool` (default `remember`) and `recall_tool` (default `recall`).
- The agent decides itself what to remember: `remember` (argument `fact`) embeds the fact and stores it; `recall` (argument `query`) returns the `top_k` most similar facts. Both are external tools (ADR-0050) – interpreter and generated code stay unchanged and equivalent.
- The tools live in the domain (`grauwolf-rag`) on top of the ports `Embedder` and `VectorStore`; the facade provides a file-based `VectorStore` that loads on opening and writes atomically after every entry (temporary file + `rename`). Identical facts are stored only once.

## Consequences
- Memories survive restarts and apply to all conversations of the agent; under `spec serve` all callers share one memory (separation by user would be a follow-up step).
- Automatic recall before every request is deliberately not part of this step; it would need a hook in the interpreter and in the generated code.
- Large memories will later need a real vector store; the port allows swapping it.

## Addendum (2026-11-09): follow-up steps implemented
Separation by user is implemented with ADR-0059, automatic recall and storing with ADR-0069.
