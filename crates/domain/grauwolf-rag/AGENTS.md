# grauwolf-rag

**Layer:** `domain` · **Status:** `Embedding` with `cosine`, `Chunk`/`ScoredChunk`, port `VectorStore` (add, search by similarity) with `InMemoryVectorStore`. Port `Embedder`, `chunk_text(source, text, max_chars)` (paragraphs; long paragraphs split at spaces; short ones combined; IDs `<source>#<n>`), `Retriever` (`index` in batches of 16, `search(query, k)`; `with_min_similarity(Some(x))` drops weaker hits after the store search, ADR-0113). `search_tool(retriever, name, k)` offers the search as a `DynTool` (argument `query`, result `[<source>] <text>` per hit, otherwise "No matches."; `DynTool::knowledge_context()` returns the same search as a `KnowledgeContext` for `knowledge.inject`, ADR-0113). Long-term memory (ADR-0058): `remember_tool(retriever, name)` (argument `fact`, stores the fact with source `memory`, reply "Remembered.") and `recall_tool(retriever, name, k)` (argument `query`, the most relevant facts, one per line). **Goal:** spec `knowledge` and `long_term_memory`

## Purpose
Retrieval-Augmented Generation (E08): split documents into sections, embed them, find similar sections, and offer them to agents as a tool; remember facts across conversations and recall them.

## Features / ADRs
E08 (F08.1–F08.8), E07 (long-term memory), ADR-0038, ADR-0058

## Allowed dependencies
- grauwolf crates: grauwolf-tools, grauwolf-prompt (for the search tool)
- External: futures-util, serde, serde_json, thiserror; dev: futures-executor

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- No I/O: the facade reads files, and adapters supply embeddings via the port `Embedder`.
- Hits are sorted by similarity, descending.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-rag`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
