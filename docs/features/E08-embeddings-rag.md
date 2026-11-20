# E08 – Embeddings & RAG


| ID | Feature | Rust approach | Prio |
|----|---------|-------------|------|
| F08.1 | Vector type + distance measures (cosine, Euclidean, dot) | `struct Embedding(Vec<f32>)` + functions | M3 |
| F08.2 | Embedder trait | `trait Embedder { async fn embed(&self, text) -> Result<Embedding> }` | M3 |
| F08.3 | LLM-based embedders (OpenAI, Ollama, …) | uses `EmbeddingClient` from E02 | M3 |
| F08.4 | Document abstraction & file system provider (JVM FS, read-only, filter) | `trait FileSystemProvider` (also for the E03 file tools) – `tokio::fs` | M2 (because of E03) |
| F08.5 | Document storage / ranked document storage | Traits | M3 |
| F08.6 | Vector storage in-memory + file-based | In-memory + file (bincode/JSON) | M3 |
| F08.7 | External vector stores | optional: `qdrant-client`, `pgvector` via sqlx (feature flags) | M3 |
| F08.8 | RAG retrieval as tool / node | `rag::search_tool(storage)` + node | M3 |
