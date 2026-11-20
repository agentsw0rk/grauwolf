//! SQL storage (F07.3, F07.15, ADR-0048): conversations and checkpoints in SQLite or Postgres
//! via `sqlx::Any`.

use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::future::BoxFuture;
use grauwolf_agents::checkpoints::{CheckpointStore, ExecutionState};
use grauwolf_agents::memory::ChatHistoryStore;
use grauwolf_graph::NodeError;
use grauwolf_prompt::Message;
use sqlx::any::AnyPoolOptions;
use sqlx::{AnyPool, Row};

/// Error while connecting to or setting up the database.
#[derive(Debug, thiserror::Error)]
pub enum SqlStoreError {
    /// Connection or query failed.
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
}

/// Schema of the conversation table; created when connecting.
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS grauwolf_conversations (\
     conversation_id TEXT PRIMARY KEY, \
     messages TEXT NOT NULL, \
     updated_at TEXT NOT NULL)";

/// Connects to `url` and creates the table according to `schema`; SQLite files are created
/// when needed.
async fn connect(url: &str, schema: &str) -> Result<AnyPool, SqlStoreError> {
    sqlx::any::install_default_drivers();
    let url = if url.starts_with("sqlite:") && !url.contains('?') {
        format!("{url}?mode=rwc")
    } else {
        url.to_owned()
    };
    let pool = AnyPoolOptions::new().connect(&url).await?;
    sqlx::query(schema).execute(&pool).await?;
    Ok(pool)
}

/// Point in time in Unix seconds for `updated_at`.
fn now() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs().to_string())
        .unwrap_or_default()
}

/// [`ChatHistoryStore`] in an SQL database (SQLite or Postgres).
#[derive(Debug, Clone)]
pub struct SqlChatHistoryStore {
    pool: AnyPool,
}

impl SqlChatHistoryStore {
    /// Connects to `url` (`sqlite://file.db`, `postgres://…`) and creates the table.
    /// SQLite files are created when needed.
    pub async fn connect(url: &str) -> Result<Self, SqlStoreError> {
        Ok(Self {
            pool: connect(url, SCHEMA).await?,
        })
    }
}

impl ChatHistoryStore for SqlChatHistoryStore {
    fn load<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<Message>, NodeError>> {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT messages FROM grauwolf_conversations WHERE conversation_id = $1",
            )
            .bind(conversation_id)
            .fetch_optional(&self.pool)
            .await?;
            match row {
                Some(row) => Ok(serde_json::from_str(&row.try_get::<String, _>(0)?)?),
                None => Ok(Vec::new()),
            }
        })
    }

    fn save<'a>(
        &'a self,
        conversation_id: &'a str,
        messages: &'a [Message],
    ) -> BoxFuture<'a, Result<(), NodeError>> {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO grauwolf_conversations (conversation_id, messages, updated_at) \
                 VALUES ($1, $2, $3) \
                 ON CONFLICT (conversation_id) DO UPDATE \
                 SET messages = excluded.messages, updated_at = excluded.updated_at",
            )
            .bind(conversation_id)
            .bind(serde_json::to_string(messages)?)
            .bind(now())
            .execute(&self.pool)
            .await?;
            Ok(())
        })
    }
}

/// Schema of the checkpoint table: the latest state per run.
const CHECKPOINT_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS grauwolf_checkpoints (\
     run_id TEXT PRIMARY KEY, \
     state TEXT NOT NULL, \
     updated_at TEXT NOT NULL)";

/// [`CheckpointStore`] in an SQL database (SQLite or Postgres, F07.15).
#[derive(Debug, Clone)]
pub struct SqlCheckpointStore {
    pool: AnyPool,
}

impl SqlCheckpointStore {
    /// Connects to `url` (`sqlite://file.db`, `postgres://…`) and creates the table.
    pub async fn connect(url: &str) -> Result<Self, SqlStoreError> {
        Ok(Self {
            pool: connect(url, CHECKPOINT_SCHEMA).await?,
        })
    }
}

impl CheckpointStore for SqlCheckpointStore {
    fn save<'a>(
        &'a self,
        run_id: &'a str,
        state: &'a ExecutionState,
    ) -> BoxFuture<'a, Result<(), NodeError>> {
        Box::pin(async move {
            // A single upsert is atomic: an abort never leaves half-written states.
            sqlx::query(
                "INSERT INTO grauwolf_checkpoints (run_id, state, updated_at) \
                 VALUES ($1, $2, $3) \
                 ON CONFLICT (run_id) DO UPDATE \
                 SET state = excluded.state, updated_at = excluded.updated_at",
            )
            .bind(run_id)
            .bind(serde_json::to_string(state)?)
            .bind(now())
            .execute(&self.pool)
            .await?;
            Ok(())
        })
    }

    fn load<'a>(
        &'a self,
        run_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<ExecutionState>, NodeError>> {
        Box::pin(async move {
            let row = sqlx::query("SELECT state FROM grauwolf_checkpoints WHERE run_id = $1")
                .bind(run_id)
                .fetch_optional(&self.pool)
                .await?;
            match row {
                Some(row) => Ok(Some(serde_json::from_str(&row.try_get::<String, _>(0)?)?)),
                None => Ok(None),
            }
        })
    }
}
