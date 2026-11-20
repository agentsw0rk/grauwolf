# grauwolf-store-sql

**Layer:** `adapters` · **Status:** `SqlChatHistoryStore::connect(url)` (sqlx `Any`: `sqlite://…` – the file is created when needed – or `postgres://…`) implements `ChatHistoryStore`; the table `grauwolf_conversations (conversation_id PRIMARY KEY, messages JSON-Text, updated_at Unix-Sekunden)` is created on connect; `save` via upsert. `SqlCheckpointStore::connect(url)` implements `CheckpointStore` (table `grauwolf_checkpoints (run_id PRIMARY KEY, state JSON-Text, updated_at)`, the latest state per run via upsert; F07.15). **Goal:** MySQL

## Purpose
SQL storage for conversation histories and, later, checkpoints (E07 F07.3, F07.15).

## Features / ADRs
E07 F07.3, ADR-0048

## Allowed dependencies
- grauwolf crates: grauwolf-agents, grauwolf-graph, grauwolf-prompt
- External: futures-util, serde_json, sqlx (any, sqlite, postgres, runtime-tokio, tls-rustls), thiserror; dev: tokio

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- SQL only in forms that SQLite and Postgres both understand (`$n` placeholders, `ON CONFLICT … DO UPDATE`).
- Messages as JSON, in the same format as the file store.

## Tests
- Runs against SQLite files under `target/store-sql-tests`; Postgres is not tested automatically.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-store-sql`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
