# ADR-0048: SQL store for conversation histories

- **Status:** Accepted (User, 2026-11-08: "SQL store for histories")
- **Date:** 2026-11-08

## Context
Chat memory (ADR-0037) has so far stored histories as files. Servers with many users, multiple processes or shared databases need an SQL store with schema migration (F07.3).

## Decision
- New adapter crate `grauwolf-store-sql` based on `sqlx` with the `Any` driver: one implementation for **SQLite and Postgres**, selected by the URL (`sqlite://…`, `postgres://…`). MySQL follows when needed (same code, another driver).
- `SqlChatHistoryStore::connect(url)` creates the table (`CREATE TABLE IF NOT EXISTS grauwolf_conversations (conversation_id TEXT PRIMARY KEY, messages TEXT NOT NULL, updated_at TEXT NOT NULL)`); `save` replaces the history with an upsert (`ON CONFLICT … DO UPDATE`), messages as JSON as in the file store. SQLite files are created when needed.
- The facade selects the store: `memory::chat_history_store(target)` – a database URL yields the SQL store, otherwise a directory for files. The CLI gets `--conversations-db <url>` next to `--conversations-dir`.
- Checkpoints in SQL (F07.15) follow with the same crate.

## Consequences
- Conversations survive processes and can be shared between instances; Postgres needs a server, automated tests run against SQLite.
- `sqlx` enlarges the build; the crate is an optional feature (`sql`) in the facade.

## Addendum (2026-11-09): checkpoints in SQL implemented
Checkpoints in SQL (F07.15) are implemented: `SqlCheckpointStore` in `grauwolf-store-sql`, in the CLI via `--runs-db` next to `--runs-dir`. MySQL is still not connected.
