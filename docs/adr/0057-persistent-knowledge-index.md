# ADR-0057: Persistent knowledge index – re-embed only what changed

- **Status:** Accepted (User, 2026-11-08: "then go on" on the proposal)
- **Date:** 2026-11-08

## Context
`knowledge` (ADR-0038) splits and embeds all sources anew at every startup. For larger collections this costs noticeable time and – with paid embedding services – money (F08.6).

## Decision
- `knowledge.index` (optional, relative to the spec) names a JSON file with the embedding model, chunk size and, per source file, a checksum of the content (FNV-1a, 64 bits) together with chunks and embeddings.
- At startup: if model and chunk size match, unchanged files are taken from the index; only new or changed files are split and embedded; removed files are dropped. Afterwards the index is rewritten atomically (temporary file + `rename`).
- Search stays in memory (`InMemoryVectorStore`); the index is only a cache. Without `index` everything behaves as before.
- The logic lives in the facade (`grauwolf::knowledge`), the domain stays free of IO.

## Consequences
- Second startup without changes: no embedding requests for documents.
- Very large collections will later need a real vector store (e.g. SQL with vector search); the port `VectorStore` allows that.
