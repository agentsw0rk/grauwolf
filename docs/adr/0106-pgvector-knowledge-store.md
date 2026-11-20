# ADR-0106: PostgreSQL with pgvector for knowledge and long-term memory

- **Status:** Accepted (ADR-0092, F14.19; User, 2026-11-13: knowledge and long-term memory; live check against the owner's pgvector container)
- **Date:** 2026-11-10

## Context
grauwolf's knowledge (RAG) keeps vectors in memory and persists the index in a file; only changed
content is embedded again (ADR-0038, ADR-0057). That fits small and medium document sets per
container. AgentWeaver offers PostgreSQL with the pgvector extension as storage
(`PGVECTOR_HOST`, `PGVECTOR_PORT`, `PGVECTOR_DB`, `PGVECTOR_USER`, `PGVECTOR_PASSWORD`, plus a
docker-compose file with `pgvector/pgvector:pg17`) – for large collections and for several replicas
sharing one index.

## Decision
- **Port** `VectorStore` (existing, ADR-0038) gets a second implementation in a new adapter
  `grauwolf-store-pgvector` (sqlx with PostgreSQL, already used for chat history and checkpoints,
  ADR-0048).
- **Schema** (created on start if missing, `CREATE EXTENSION IF NOT EXISTS vector`): table
  `grauwolf_chunks(collection text, source text, chunk int, hash text, content text, embedding
  vector(<dim>), primary key (collection, source, chunk))` with an HNSW index on `embedding`
  (cosine). `collection` = spec name plus knowledge name, so several agents share one database.
- **Incremental indexing** as ADR-0057: chunks are identified by source and hash; only new or changed
  chunks are embedded; chunks of removed sources are deleted. A changed embedding model or dimension
  rebuilds the collection.
- **Spec:** `knowledge.store: { "kind": "postgres", "url_env": "PGVECTOR_URL" }` (default kind
  `file`). The URL comes only from the environment. The AgentWeaver export maps its `PGVECTOR_*`
  variables to a URL in the deployment and keeps its docker-compose file for local runs.
- Search: top-k by cosine distance with the similarity threshold of the spec, as today.

## Consequences
- Large collections and multiple replicas work with one shared index; the container stays stateless.
- Concurrent indexing by several replicas is serialized with a PostgreSQL advisory lock per
  collection.
- Tests need PostgreSQL with pgvector; they are skipped without `PGVECTOR_TEST_URL`.

## Implementation notes (2026-11-13)
- **Long-term memory too** (extension decided by the user): `long_term_memory.vector_store: { "kind":
  "postgres", "url_env": "PGVECTOR_URL", "ttl_days"?: n (>= 1), "min_similarity"?: 0..1 }`, mutually
  exclusive with `store` (validation error `InvalidStore` for none or both). Table
  `grauwolf_memories(agent, user_id, id, content, embedding vector(dim), created_at, expires_at,
  access_count, primary key (agent, user_id, id))` with an HNSW index. `agent` is the spec name,
  `user_id` is empty for the shared area and `user:<id>` for a user (so no user id can reach the
  shared area), `id` is the SHA-256 of the text: identical facts are one row per area; storing one
  again renews its expiry. Recall returns at most `top_k` unexpired facts with similarity >=
  `min_similarity`, best first, and counts `access_count`; expired facts are deleted lazily by the
  next write of the same area (no background job). `min_similarity` applies to the PostgreSQL store
  only; the file store is unchanged. The memory records the embedding model per agent in
  `grauwolf_collections` (`memory:<agent>`) and refuses another one, like the file.
- **Spec shape:** `knowledge.store: { kind: file|postgres, url_env? }` (a struct with a `kind`
  enum, no tagged union, so `deny_unknown_fields` holds and nothing is `untagged`/`flatten`);
  `knowledge.index` together with `postgres` is an error. The URL is only ever read from the
  variable named by `url_env`, through the host (`grauwolf_host::pgvector::connect`); `PGVECTOR_URL`
  and `PGVECTOR_TEST_URL` are in `config::VARIABLES`.
- **Schema:** `grauwolf_collections(collection, model, dimension)` records model and dimension per
  collection (knowledge `<spec name>/<tool name>`). A table has one dimension: a collection with
  another dimension recreates the table if it is empty and is an error otherwise (use a separate
  database or schema). pgvector indexes up to 2000 values with HNSW; larger models are refused with
  a clear message. The dimension is learned by one probe embedding when a collection is new or
  rebuilt, and a changed dimension is also detected when new chunks are embedded.
- **Vector literal:** vectors are sent as pgvector text literals (`[0.1,0.2]`) bound as text and cast
  with `::text::vector`; the `pgvector` crate was not needed, so there is no new dependency beyond
  `sha2` (already in the workspace).
- **Concurrency:** schema changes take a transaction-level advisory lock `grauwolf-schema`, indexing
  a collection takes one per collection (`hashtextextended(collection)`); both are released with the
  transaction, so a failed indexing run changes nothing.
- **Pooling:** knowledge and memory with the same URL share one sqlx pool (registry keyed by URL).
- **Errors:** a missing variable, an invalid URL, a refused connection, rejected credentials, a
  missing extension and a missing permission to create it each have their own message naming the
  variable; URL, user and password never appear.
- **Features:** facade and host feature `pgvector` (in `default`); generated crates enable it only
  when the spec uses a postgres store.
- **Tested without a database:** the SQL texts, the vector literal, incremental indexing (hash per
  chunk, partial re-embedding, removed sources, surplus chunks, rebuild on model and dimension
  change, rollback on failure, concurrent replicas, lock use), the memory logic (TTL, lazy delete,
  `min_similarity`, user separation, `access_count`, model check) and the host wiring against the
  in-memory stand-in `InMemoryPgVector`; error texts through a refused connection. The real SQL
  (`tests/postgres.rs`) runs only with `PGVECTOR_TEST_URL`.
- **Live check (2026-11-13)** against `pgvector/pgvector:pg17` (owner's container, reached at
  `host.docker.internal:5432`):
  - `tests/postgres.rs` with `PGVECTOR_TEST_URL`: both tests pass (incremental indexing and
    similarity search; memory separated per user and expiring) – so the typing of
    `to_timestamp($n)` with `bigint` works.
  - `docs/user-guide/examples/07-knowledge-postgres.json` with Ollama (`qwen3.5:9b`,
    `nomic-embed-text`, 768 values) in a fresh database: "Ab wann ist Nachtruhe?" answered from the
    house rules with the source; the second run took 6 s instead of 22 s (nothing re-embedded).
  - `07-memory-postgres.json`: `--user anna` stored "my favourite tea is Earl Grey", a new run as
    `anna` recalled it, a run as `bob` did not know it.
  - **Limitation seen live:** one table holds vectors of one dimension. The test database
    already held 3-value vectors from the tests, so the 768-value model was refused with a clear
    error ("use a separate database or schema for this model"); agents with embedding models of
    different dimensions need separate databases or schemas.
  - Still unconfirmed: the HNSW search with the `collection` filter when many collections share
    the table (the index may return fewer than `top_k` rows; pgvector 0.8 iterative scans would
    fix that).
