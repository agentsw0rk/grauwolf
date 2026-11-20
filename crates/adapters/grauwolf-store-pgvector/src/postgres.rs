//! The PostgreSQL implementation of [`ChunkDb`] and [`MemoryDb`] (ADR-0106).
//!
//! Error texts name the environment variable that holds the URL, never the URL or the password.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures_util::future::BoxFuture;
use grauwolf_rag::{Chunk, Embedding, ScoredChunk};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgConnection, PgPool, Postgres, Row, Transaction};

use crate::PgVectorError;
use crate::chunks::{ChunkDb, ChunkTx, CollectionInfo, StoredChunk, StoredHashes};
use crate::memory::{MemoryArea, MemoryDb, NewFact, RecalledFact};
use crate::sql::{self, vector_literal};

/// Prefix of the collection that records the model of an agent's memory.
const MEMORY_COLLECTION: &str = "memory:";

/// Text for a database error that names `variable` and never the URL or password.
pub fn describe(error: &sqlx::Error, variable: &str) -> String {
    match error {
        sqlx::Error::Configuration(_) => {
            format!("the URL in the environment variable {variable} is not a valid PostgreSQL URL")
        }
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) | sqlx::Error::PoolTimedOut => format!(
            "cannot connect to the PostgreSQL server named by {variable} (connection refused, \
             unreachable or timed out)"
        ),
        sqlx::Error::Database(database) => {
            describe_database(database.code().as_deref(), database.message(), variable)
        }
        _ => format!("PostgreSQL request failed (connection from {variable})"),
    }
}

/// Text for a server error with SQLSTATE `code` and `message`.
pub fn describe_database(code: Option<&str>, message: &str, variable: &str) -> String {
    match code {
        Some("28P01" | "28000") => {
            format!("PostgreSQL rejected the credentials in the URL from {variable}")
        }
        Some("3D000") => format!("the database named in the URL from {variable} does not exist"),
        Some("42501") => format!(
            "no permission for this operation with the URL from {variable}; for the extension ask \
             an administrator to run `CREATE EXTENSION vector` once"
        ),
        Some("0A000") if message.contains("vector") => format!(
            "the pgvector extension is not available on the server named by {variable}; install \
             pgvector (e.g. the image pgvector/pgvector) and run `CREATE EXTENSION vector`"
        ),
        _ => format!("PostgreSQL error: {message}"),
    }
}

/// A shared connection pool with the schema ready.
#[derive(Clone)]
pub struct PgVector {
    pool: PgPool,
    variable: String,
}

impl PgVector {
    /// Connects with `url`, which came from the environment variable `variable`, and creates
    /// the extension and the collections table if missing. Knowledge and memory with the same URL
    /// share one pool.
    pub async fn connect(variable: &str, url: &str) -> Result<Arc<Self>, PgVectorError> {
        static OPEN: Mutex<Option<HashMap<String, Weak<PgVector>>>> = Mutex::new(None);
        let find = || {
            let open = OPEN.lock().expect("pool registry");
            open.as_ref()?.get(url)?.upgrade()
        };
        if let Some(shared) = find() {
            return Ok(shared);
        }
        let options: PgConnectOptions = url.parse().map_err(|error| {
            PgVectorError::Connect(describe(
                &sqlx::Error::Configuration(Box::new(error)),
                variable,
            ))
        })?;
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(options)
            .await
            .map_err(|error| PgVectorError::Connect(describe(&error, variable)))?;
        let created = Self {
            pool,
            variable: variable.to_owned(),
        };
        created.init().await?;
        let created = Arc::new(created);
        let mut open = OPEN.lock().expect("pool registry");
        let open = open.get_or_insert_with(HashMap::new);
        open.retain(|_, pool| pool.strong_count() > 0);
        match open.get(url).and_then(Weak::upgrade) {
            Some(shared) => Ok(shared),
            None => {
                open.insert(url.to_owned(), Arc::downgrade(&created));
                Ok(created)
            }
        }
    }

    fn fail(&self, error: sqlx::Error) -> PgVectorError {
        PgVectorError::Database(describe(&error, &self.variable))
    }

    async fn init(&self) -> Result<(), PgVectorError> {
        let mut tx = self.pool.begin().await.map_err(|e| self.fail(e))?;
        sqlx::query(sql::LOCK_SCHEMA)
            .execute(&mut *tx)
            .await
            .map_err(|e| self.fail(e))?;
        sqlx::query(sql::CREATE_EXTENSION)
            .execute(&mut *tx)
            .await
            .map_err(|e| self.fail(e))?;
        sqlx::query(sql::CREATE_COLLECTIONS)
            .execute(&mut *tx)
            .await
            .map_err(|e| self.fail(e))?;
        sqlx::query(sql::CREATE_URL_VALIDATORS)
            .execute(&mut *tx)
            .await
            .map_err(|e| self.fail(e))?;
        tx.commit().await.map_err(|e| self.fail(e))
    }

    /// Creates `table` (and its index) for `dimension`. A table of another dimension is
    /// recreated if it is empty, otherwise it is an error.
    async fn ensure_table(
        &self,
        connection: &mut PgConnection,
        table: &str,
        create: String,
        index: &str,
        dimension: usize,
    ) -> Result<(), PgVectorError> {
        sql::check_dimension(dimension)?;
        sqlx::query(sql::LOCK_SCHEMA)
            .execute(&mut *connection)
            .await
            .map_err(|e| self.fail(e))?;
        let current: Option<i32> = sqlx::query_scalar(sql::SELECT_TABLE_DIMENSION)
            .bind(table)
            .fetch_optional(&mut *connection)
            .await
            .map_err(|e| self.fail(e))?;
        if let Some(current) = current.filter(|current| *current as usize != dimension) {
            let rows: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&mut *connection)
                .await
                .map_err(|e| self.fail(e))?;
            if rows > 0 {
                return Err(PgVectorError::Database(format!(
                    "the table {table} holds vectors of {current} values, the embedding model returns \
                     {dimension}; use a separate database or schema for this model"
                )));
            }
            sqlx::query(&format!("DROP TABLE {table}"))
                .execute(&mut *connection)
                .await
                .map_err(|e| self.fail(e))?;
        }
        sqlx::query(&create)
            .execute(&mut *connection)
            .await
            .map_err(|e| self.fail(e))?;
        sqlx::query(index)
            .execute(&mut *connection)
            .await
            .map_err(|e| self.fail(e))?;
        Ok(())
    }
}

struct PgTx {
    tx: Transaction<'static, Postgres>,
    collection: String,
    db: PgVector,
}

impl ChunkDb for PgVector {
    fn begin<'a>(
        &'a self,
        collection: &'a str,
    ) -> BoxFuture<'a, Result<Box<dyn ChunkTx + 'a>, PgVectorError>> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(|e| self.fail(e))?;
            sqlx::query(sql::LOCK_COLLECTION)
                .bind(collection)
                .execute(&mut *tx)
                .await
                .map_err(|e| self.fail(e))?;
            Ok(Box::new(PgTx {
                tx,
                collection: collection.to_owned(),
                db: self.clone(),
            }) as Box<dyn ChunkTx + 'a>)
        })
    }

    fn url_validators<'a>(
        &'a self,
        collection: &'a str,
    ) -> BoxFuture<'a, Result<crate::chunks::UrlValidatorMap, PgVectorError>> {
        Box::pin(async move {
            let exists: Option<i32> = sqlx::query_scalar(sql::SELECT_TABLE_DIMENSION)
                .bind("grauwolf_chunks")
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| self.fail(e))?;
            if exists.is_none() {
                return Ok(Default::default());
            }
            let rows = sqlx::query(sql::SELECT_URL_VALIDATORS)
                .bind(collection)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| self.fail(e))?;
            rows.iter()
                .map(|row| {
                    Ok((
                        row.try_get::<String, _>("url")?,
                        crate::chunks::UrlValidators {
                            etag: row.try_get("etag")?,
                            last_modified: row.try_get("last_modified")?,
                        },
                    ))
                })
                .collect::<Result<_, sqlx::Error>>()
                .map_err(|e| self.fail(e))
        })
    }

    fn search<'a>(
        &'a self,
        collection: &'a str,
        query: &'a Embedding,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<ScoredChunk>, PgVectorError>> {
        Box::pin(async move {
            let literal = vector_literal(&query.0)?;
            let rows = sqlx::query(sql::SEARCH_CHUNKS)
                .bind(collection)
                .bind(literal)
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| self.fail(e))?;
            rows.iter()
                .map(|row| {
                    let source: String = row.try_get("source")?;
                    let number: i32 = row.try_get("chunk")?;
                    Ok(ScoredChunk {
                        chunk: Chunk {
                            id: format!("{source}#{number}"),
                            source,
                            text: row.try_get("content")?,
                        },
                        score: row.try_get("similarity")?,
                    })
                })
                .collect::<Result<_, sqlx::Error>>()
                .map_err(|e| self.fail(e))
        })
    }
}

impl ChunkTx for PgTx {
    fn info(&mut self) -> BoxFuture<'_, Result<Option<CollectionInfo>, PgVectorError>> {
        Box::pin(async move {
            let row = sqlx::query(sql::SELECT_COLLECTION)
                .bind(&self.collection)
                .fetch_optional(&mut *self.tx)
                .await
                .map_err(|e| self.db.fail(e))?;
            row.map(|row| {
                Ok(CollectionInfo {
                    model: row.try_get("model")?,
                    dimension: row.try_get::<i32, _>("dimension")? as usize,
                })
            })
            .transpose()
            .map_err(|e: sqlx::Error| self.db.fail(e))
        })
    }

    fn reset<'a>(
        &'a mut self,
        model: &'a str,
        dimension: usize,
    ) -> BoxFuture<'a, Result<(), PgVectorError>> {
        Box::pin(async move {
            let db = self.db.clone();
            // Own rows first, so that a table of another dimension can be recreated when no
            // other collection uses it.
            let exists: Option<i32> = sqlx::query_scalar(sql::SELECT_TABLE_DIMENSION)
                .bind("grauwolf_chunks")
                .fetch_optional(&mut *self.tx)
                .await
                .map_err(|e| db.fail(e))?;
            if exists.is_some() {
                sqlx::query(sql::DELETE_COLLECTION_CHUNKS)
                    .bind(&self.collection)
                    .execute(&mut *self.tx)
                    .await
                    .map_err(|e| db.fail(e))?;
            }
            db.ensure_table(
                &mut self.tx,
                "grauwolf_chunks",
                sql::create_chunks(dimension),
                sql::CREATE_CHUNKS_INDEX,
                dimension,
            )
            .await?;
            sqlx::query(sql::UPSERT_COLLECTION)
                .bind(&self.collection)
                .bind(model)
                .bind(dimension as i32)
                .execute(&mut *self.tx)
                .await
                .map_err(|e| db.fail(e))?;
            Ok(())
        })
    }

    fn hashes(&mut self) -> BoxFuture<'_, Result<StoredHashes, PgVectorError>> {
        Box::pin(async move {
            let exists: Option<i32> = sqlx::query_scalar(sql::SELECT_TABLE_DIMENSION)
                .bind("grauwolf_chunks")
                .fetch_optional(&mut *self.tx)
                .await
                .map_err(|e| self.db.fail(e))?;
            let mut hashes = StoredHashes::new();
            if exists.is_none() {
                return Ok(hashes);
            }
            let rows = sqlx::query(sql::SELECT_HASHES)
                .bind(&self.collection)
                .fetch_all(&mut *self.tx)
                .await
                .map_err(|e| self.db.fail(e))?;
            for row in rows {
                let read = || -> Result<(String, i32, String), sqlx::Error> {
                    Ok((
                        row.try_get("source")?,
                        row.try_get("chunk")?,
                        row.try_get("hash")?,
                    ))
                };
                let (source, number, hash) = read().map_err(|e| self.db.fail(e))?;
                hashes
                    .entry(source)
                    .or_default()
                    .insert(number as usize, hash);
            }
            Ok(hashes)
        })
    }

    fn put(&mut self, chunks: Vec<StoredChunk>) -> BoxFuture<'_, Result<(), PgVectorError>> {
        Box::pin(async move {
            for chunk in chunks {
                sqlx::query(sql::UPSERT_CHUNK)
                    .bind(&self.collection)
                    .bind(chunk.source)
                    .bind(chunk.chunk as i32)
                    .bind(chunk.hash)
                    .bind(chunk.content)
                    .bind(vector_literal(&chunk.embedding.0)?)
                    .execute(&mut *self.tx)
                    .await
                    .map_err(|e| self.db.fail(e))?;
            }
            Ok(())
        })
    }

    fn delete_tail<'a>(
        &'a mut self,
        source: &'a str,
        from: usize,
    ) -> BoxFuture<'a, Result<(), PgVectorError>> {
        Box::pin(async move {
            sqlx::query(sql::DELETE_CHUNK_TAIL)
                .bind(&self.collection)
                .bind(source)
                .bind(from as i32)
                .execute(&mut *self.tx)
                .await
                .map_err(|e| self.db.fail(e))?;
            Ok(())
        })
    }

    fn delete_sources(&mut self, sources: Vec<String>) -> BoxFuture<'_, Result<(), PgVectorError>> {
        Box::pin(async move {
            sqlx::query(sql::DELETE_SOURCES)
                .bind(&self.collection)
                .bind(sources)
                .execute(&mut *self.tx)
                .await
                .map_err(|e| self.db.fail(e))?;
            Ok(())
        })
    }

    fn replace_validators(
        &mut self,
        validators: crate::chunks::UrlValidatorMap,
    ) -> BoxFuture<'_, Result<(), PgVectorError>> {
        Box::pin(async move {
            sqlx::query(sql::DELETE_URL_VALIDATORS)
                .bind(&self.collection)
                .execute(&mut *self.tx)
                .await
                .map_err(|e| self.db.fail(e))?;
            for (url, validator) in validators {
                sqlx::query(sql::INSERT_URL_VALIDATOR)
                    .bind(&self.collection)
                    .bind(url)
                    .bind(validator.etag)
                    .bind(validator.last_modified)
                    .execute(&mut *self.tx)
                    .await
                    .map_err(|e| self.db.fail(e))?;
            }
            Ok(())
        })
    }

    fn commit(self: Box<Self>) -> BoxFuture<'static, Result<(), PgVectorError>> {
        Box::pin(async move {
            let this = *self;
            this.tx.commit().await.map_err(|e| this.db.fail(e))
        })
    }
}

impl MemoryDb for PgVector {
    fn prepare<'a>(
        &'a self,
        agent: &'a str,
        model: &'a str,
        dimension: usize,
    ) -> BoxFuture<'a, Result<(), PgVectorError>> {
        Box::pin(async move {
            let collection = format!("{MEMORY_COLLECTION}{agent}");
            let mut tx = self.pool.begin().await.map_err(|e| self.fail(e))?;
            sqlx::query(sql::LOCK_COLLECTION)
                .bind(&collection)
                .execute(&mut *tx)
                .await
                .map_err(|e| self.fail(e))?;
            let known: Option<String> =
                sqlx::query_scalar("SELECT model FROM grauwolf_collections WHERE collection = $1")
                    .bind(&collection)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|e| self.fail(e))?;
            if let Some(known) = known.filter(|known| known != model) {
                return Err(PgVectorError::Database(format!(
                    "the memory of {agent} comes from the embedding model {known}, the spec names \
                     {model}; restore the model or delete its rows in grauwolf_memories"
                )));
            }
            self.ensure_table(
                &mut tx,
                "grauwolf_memories",
                sql::create_memories(dimension),
                sql::CREATE_MEMORIES_INDEX,
                dimension,
            )
            .await?;
            sqlx::query(sql::UPSERT_COLLECTION)
                .bind(&collection)
                .bind(model)
                .bind(dimension as i32)
                .execute(&mut *tx)
                .await
                .map_err(|e| self.fail(e))?;
            tx.commit().await.map_err(|e| self.fail(e))
        })
    }

    fn put<'a>(
        &'a self,
        area: &'a MemoryArea,
        fact: NewFact,
        now: i64,
    ) -> BoxFuture<'a, Result<(), PgVectorError>> {
        Box::pin(async move {
            let literal = vector_literal(&fact.embedding.0)?;
            let mut tx = self.pool.begin().await.map_err(|e| self.fail(e))?;
            sqlx::query(sql::DELETE_EXPIRED_MEMORIES)
                .bind(&area.agent)
                .bind(&area.user_key)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(|e| self.fail(e))?;
            sqlx::query(sql::UPSERT_MEMORY)
                .bind(&area.agent)
                .bind(&area.user_key)
                .bind(&fact.id)
                .bind(&fact.content)
                .bind(literal)
                .bind(fact.created_at)
                .bind(fact.expires_at)
                .execute(&mut *tx)
                .await
                .map_err(|e| self.fail(e))?;
            tx.commit().await.map_err(|e| self.fail(e))
        })
    }

    fn nearest<'a>(
        &'a self,
        area: &'a MemoryArea,
        query: &'a Embedding,
        limit: usize,
        now: i64,
    ) -> BoxFuture<'a, Result<Vec<RecalledFact>, PgVectorError>> {
        Box::pin(async move {
            let rows = sqlx::query(sql::NEAREST_MEMORIES)
                .bind(&area.agent)
                .bind(&area.user_key)
                .bind(vector_literal(&query.0)?)
                .bind(now)
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| self.fail(e))?;
            rows.iter()
                .map(|row| {
                    Ok(RecalledFact {
                        id: row.try_get("id")?,
                        content: row.try_get("content")?,
                        similarity: row.try_get("similarity")?,
                    })
                })
                .collect::<Result<_, sqlx::Error>>()
                .map_err(|e| self.fail(e))
        })
    }

    fn touch<'a>(
        &'a self,
        area: &'a MemoryArea,
        ids: &'a [String],
    ) -> BoxFuture<'a, Result<(), PgVectorError>> {
        Box::pin(async move {
            sqlx::query(sql::TOUCH_MEMORIES)
                .bind(&area.agent)
                .bind(&area.user_key)
                .bind(ids)
                .execute(&self.pool)
                .await
                .map_err(|e| self.fail(e))?;
            Ok(())
        })
    }
}
