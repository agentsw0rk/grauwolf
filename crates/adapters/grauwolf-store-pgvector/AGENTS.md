# grauwolf-store-pgvector

**Layer:** `adapters` · **Status:** ADR-0106, F14.19: `postgres::PgVector::connect(variable, url)` (shared pool per URL, creates the extension and `grauwolf_collections` under a schema lock; implements `ChunkDb` and `MemoryDb` with sqlx; `describe`/`describe_database` give error texts that name the variable and never the URL or password); `sql` (SQL texts, `vector_literal`, `check_dimension`); `chunks` (ports `ChunkDb`/`ChunkTx`, `sync_collection` = incremental indexing under the collection lock: hash per chunk, only new or changed chunks embedded, removed sources and surplus chunks deleted, rebuild on model or dimension change, one transaction; `PgChunkStore` as `VectorStore`; URL sources, ADR-0113: `sync_collection_with(.., keep, validators)` keeps the sources named in `keep` (a `304` or failed fetch) and replaces the validators, `ChunkDb::url_validators(collection)` returns those of URLs that still have chunks, table `grauwolf_url_validators(collection, url, etag, last_modified)`; a rebuild with `keep` fails so the next start fetches anew); `memory` (port `MemoryDb`, `PgMemoryStore` as `VectorStore` per agent and user: one row per distinct fact, expiry from `MemoryPolicy.ttl_days` at write time, expired facts never returned and deleted on the next write of the area, `min_similarity` filter, `access_count` on recall; `user_key`: shared area = empty, user = `user:<id>`); `InMemoryPgVector` stand-in without a database. **Goal:** `VectorStore` for the knowledge and the long-term memory in PostgreSQL with pgvector

## Purpose
PostgreSQL with the pgvector extension as the store for knowledge (RAG) and long-term memory: large collections, several replicas sharing one index, facts that expire.

## Features / ADRs
E14 F14.19, ADR-0038, ADR-0057, ADR-0058, ADR-0059, ADR-0106

## Allowed dependencies
- grauwolf crates: grauwolf-rag
- External: futures-util, sha2, sqlx (postgres, runtime-tokio, tls-rustls), thiserror, tokio (sync); dev: tokio

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- Vectors travel as pgvector text literals (`[0.1,0.2]`, bound as text and cast with `::text::vector`); no extra crate.
- Connection URLs and passwords never appear in an error text.

## Tests
- Logic runs without PostgreSQL against the in-memory stand-in; tests against a real database run only with `PGVECTOR_TEST_URL` (they return early otherwise).

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-store-pgvector`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
