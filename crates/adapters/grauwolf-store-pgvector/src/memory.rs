//! Long-term memory in PostgreSQL (ADR-0058, ADR-0059, ADR-0106): facts per agent and user, with
//! optional expiry and a minimum similarity.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::future::BoxFuture;
use grauwolf_rag::{Chunk, Embedding, RagError, ScoredChunk, VectorStore};

use crate::PgVectorError;
use crate::chunks::chunk_hash;

/// Source of the facts, as in the file store.
const MEMORY_SOURCE: &str = "memory";

const SECONDS_PER_DAY: i64 = 86_400;

/// Whose facts: the agent (spec name) and the user key ([`user_key`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MemoryArea {
    /// Spec name.
    pub agent: String,
    /// Result of [`user_key`].
    pub user_key: String,
}

/// Column value for a user: the shared area (no user) is the empty string, a user is
/// `user:<id>`, so no user id can ever reach the shared area.
pub fn user_key(user: Option<&str>) -> String {
    user.map(|user| format!("user:{user}")).unwrap_or_default()
}

/// A fact to store.
#[derive(Debug, Clone, PartialEq)]
pub struct NewFact {
    /// Hash of the content: the same text is one row per area.
    pub id: String,
    /// The fact.
    pub content: String,
    /// Its embedding.
    pub embedding: Embedding,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds, `None` = never.
    pub expires_at: Option<i64>,
}

/// A fact found by the database.
#[derive(Debug, Clone, PartialEq)]
pub struct RecalledFact {
    /// Hash of the content.
    pub id: String,
    /// The fact.
    pub content: String,
    /// Cosine similarity to the query.
    pub similarity: f32,
}

/// The database side of the memory.
pub trait MemoryDb: Send + Sync {
    /// Creates the table for `dimension` if needed and records the embedding `model` of the
    /// agent; another model than the recorded one is an error (the vectors would not compare).
    fn prepare<'a>(
        &'a self,
        agent: &'a str,
        model: &'a str,
        dimension: usize,
    ) -> BoxFuture<'a, Result<(), PgVectorError>>;

    /// Deletes the area's facts that expired at `now`, then stores `fact`; the same fact again
    /// only renews its expiry.
    fn put<'a>(
        &'a self,
        area: &'a MemoryArea,
        fact: NewFact,
        now: i64,
    ) -> BoxFuture<'a, Result<(), PgVectorError>>;

    /// The `limit` unexpired facts (at `now`) of the area nearest to `query`, most similar first.
    fn nearest<'a>(
        &'a self,
        area: &'a MemoryArea,
        query: &'a Embedding,
        limit: usize,
        now: i64,
    ) -> BoxFuture<'a, Result<Vec<RecalledFact>, PgVectorError>>;

    /// Counts a recall for the facts `ids`.
    fn touch<'a>(
        &'a self,
        area: &'a MemoryArea,
        ids: &'a [String],
    ) -> BoxFuture<'a, Result<(), PgVectorError>>;
}

/// Expiry and relevance of a memory.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct MemoryPolicy {
    /// Facts expire this many days after storing; `None` = never.
    pub ttl_days: Option<u32>,
    /// Recall drops facts less similar than this; `None` = keep the `limit` best.
    pub min_similarity: Option<f32>,
}

impl MemoryPolicy {
    /// Expiry (Unix seconds) of a fact stored at `now`.
    pub fn expires_at(&self, now: i64) -> Option<i64> {
        self.ttl_days
            .map(|days| now + i64::from(days) * SECONDS_PER_DAY)
    }

    /// Keeps the facts that reach `min_similarity`.
    pub fn filter(&self, mut facts: Vec<RecalledFact>) -> Vec<RecalledFact> {
        if let Some(minimum) = self.min_similarity {
            facts.retain(|fact| fact.similarity >= minimum);
        }
        facts
    }
}

/// Current time in Unix seconds.
pub type Now = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The system clock as [`Now`].
pub fn system_now() -> Now {
    Arc::new(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs() as i64)
    })
}

/// The memory of one area as a [`VectorStore`]: `add` stores a fact once per area (with the
/// policy's expiry), `search` returns unexpired facts of the area that reach the minimum
/// similarity and counts the recall.
pub struct PgMemoryStore {
    db: Arc<dyn MemoryDb>,
    area: MemoryArea,
    policy: MemoryPolicy,
    now: Now,
}

impl PgMemoryStore {
    /// Memory of `agent` for `user` (`None` = shared area).
    pub fn new(
        db: Arc<dyn MemoryDb>,
        agent: &str,
        user: Option<&str>,
        policy: MemoryPolicy,
        now: Now,
    ) -> Self {
        Self {
            db,
            area: MemoryArea {
                agent: agent.to_owned(),
                user_key: user_key(user),
            },
            policy,
            now,
        }
    }
}

fn store_error(error: PgVectorError) -> RagError {
    RagError::Store(Box::new(error))
}

impl VectorStore for PgMemoryStore {
    fn add<'a>(
        &'a self,
        chunk: Chunk,
        embedding: Embedding,
    ) -> BoxFuture<'a, Result<(), RagError>> {
        Box::pin(async move {
            let now = (self.now)();
            let fact = NewFact {
                id: chunk_hash(&chunk.text),
                content: chunk.text,
                embedding,
                created_at: now,
                expires_at: self.policy.expires_at(now),
            };
            self.db
                .put(&self.area, fact, now)
                .await
                .map_err(store_error)
        })
    }

    fn search<'a>(
        &'a self,
        query: &'a Embedding,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<ScoredChunk>, RagError>> {
        Box::pin(async move {
            let found = self
                .db
                .nearest(&self.area, query, limit, (self.now)())
                .await
                .map_err(store_error)?;
            let facts = self.policy.filter(found);
            let ids: Vec<String> = facts.iter().map(|fact| fact.id.clone()).collect();
            if !ids.is_empty() {
                self.db.touch(&self.area, &ids).await.map_err(store_error)?;
            }
            Ok(facts
                .into_iter()
                .map(|fact| ScoredChunk {
                    chunk: Chunk {
                        id: fact.id,
                        source: MEMORY_SOURCE.into(),
                        text: fact.content,
                    },
                    score: fact.similarity,
                })
                .collect())
        })
    }
}
