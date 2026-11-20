//! A stand-in for PostgreSQL that keeps everything in the process: for tests of the logic on top
//! of [`ChunkDb`] without a database. It mirrors what matters: a lock per collection until the
//! transaction ends, rollback by dropping, one embedding dimension per table.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use grauwolf_rag::{Chunk, Embedding, ScoredChunk};
use tokio::sync::OwnedMutexGuard;

use crate::PgVectorError;
use crate::chunks::{ChunkDb, ChunkTx, CollectionInfo, StoredChunk, StoredHashes, UrlValidatorMap};
use crate::memory::{MemoryArea, MemoryDb, NewFact, RecalledFact};

#[derive(Debug, Clone, Default)]
struct State {
    collections: BTreeMap<String, CollectionInfo>,
    /// Dimension of the chunk table, once created.
    table_dimension: Option<usize>,
    /// (collection, source, chunk) → row.
    chunks: BTreeMap<(String, String, usize), StoredChunk>,
    /// collection → validators of its URL sources.
    validators: BTreeMap<String, UrlValidatorMap>,
    /// agent → model of the memory.
    memory_models: BTreeMap<String, String>,
    /// (agent, user key, id) → row.
    memories: BTreeMap<(String, String, String), MemoryRow>,
}

#[derive(Debug, Clone)]
struct MemoryRow {
    fact: NewFact,
    access_count: u64,
}

/// In-memory stand-in for the database.
#[derive(Default)]
pub struct InMemoryPgVector {
    state: Arc<Mutex<State>>,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    events: Arc<Mutex<Vec<String>>>,
}

impl InMemoryPgVector {
    /// Log of what happened: `lock <collection>` when a transaction took the lock, `commit`.
    pub fn events(&self) -> Vec<String> {
        self.events.lock().expect("events").clone()
    }

    /// Stored chunks of `collection` as `(source, chunk number, content)`.
    pub fn chunks_of(&self, collection: &str) -> Vec<(String, usize, String)> {
        let state = self.state.lock().expect("state");
        state
            .chunks
            .iter()
            .filter(|((name, _, _), _)| name == collection)
            .map(|((_, source, number), row)| (source.clone(), *number, row.content.clone()))
            .collect()
    }

    /// Facts of an area as `(content, access count)`, expired ones included until a write
    /// deletes them.
    pub fn memories_of(&self, agent: &str, user_key: &str) -> Vec<(String, u64)> {
        let state = self.state.lock().expect("state");
        state
            .memories
            .iter()
            .filter(|((a, u, _), _)| a == agent && u == user_key)
            .map(|(_, row)| (row.fact.content.clone(), row.access_count))
            .collect()
    }

    /// Model and dimension recorded for `collection`.
    pub fn info_of(&self, collection: &str) -> Option<CollectionInfo> {
        self.state
            .lock()
            .expect("state")
            .collections
            .get(collection)
            .cloned()
    }
}

struct Tx {
    collection: String,
    work: State,
    shared: Arc<Mutex<State>>,
    events: Arc<Mutex<Vec<String>>>,
    _lock: OwnedMutexGuard<()>,
}

impl ChunkDb for InMemoryPgVector {
    fn begin<'a>(
        &'a self,
        collection: &'a str,
    ) -> BoxFuture<'a, Result<Box<dyn ChunkTx + 'a>, PgVectorError>> {
        Box::pin(async move {
            let lock = Arc::clone(
                self.locks
                    .lock()
                    .expect("locks")
                    .entry(collection.to_owned())
                    .or_default(),
            );
            let guard = lock.lock_owned().await;
            self.events
                .lock()
                .expect("events")
                .push(format!("lock {collection}"));
            let work = self.state.lock().expect("state").clone();
            Ok(Box::new(Tx {
                collection: collection.to_owned(),
                work,
                shared: Arc::clone(&self.state),
                events: Arc::clone(&self.events),
                _lock: guard,
            }) as Box<dyn ChunkTx + 'a>)
        })
    }

    fn url_validators<'a>(
        &'a self,
        collection: &'a str,
    ) -> BoxFuture<'a, Result<UrlValidatorMap, PgVectorError>> {
        Box::pin(async move {
            let state = self.state.lock().expect("state");
            let mut found = state
                .validators
                .get(collection)
                .cloned()
                .unwrap_or_default();
            found.retain(|url, _| {
                state
                    .chunks
                    .keys()
                    .any(|(name, source, _)| name == collection && source == url)
            });
            Ok(found)
        })
    }

    fn search<'a>(
        &'a self,
        collection: &'a str,
        query: &'a Embedding,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<ScoredChunk>, PgVectorError>> {
        Box::pin(async move {
            let state = self.state.lock().expect("state");
            let mut hits: Vec<ScoredChunk> = state
                .chunks
                .iter()
                .filter(|((name, _, _), _)| name == collection)
                .map(|((_, source, number), row)| ScoredChunk {
                    chunk: Chunk {
                        id: format!("{source}#{number}"),
                        source: source.clone(),
                        text: row.content.clone(),
                    },
                    score: query.cosine(&row.embedding),
                })
                .collect();
            hits.sort_by(|a, b| b.score.total_cmp(&a.score));
            hits.truncate(limit);
            Ok(hits)
        })
    }
}

impl ChunkTx for Tx {
    fn info(&mut self) -> BoxFuture<'_, Result<Option<CollectionInfo>, PgVectorError>> {
        let info = self.work.collections.get(&self.collection).cloned();
        Box::pin(async move { Ok(info) })
    }

    fn reset<'a>(
        &'a mut self,
        model: &'a str,
        dimension: usize,
    ) -> BoxFuture<'a, Result<(), PgVectorError>> {
        let collection = self.collection.clone();
        self.work
            .chunks
            .retain(|(name, _, _), _| *name != collection);
        self.work.validators.remove(&collection);
        let result = match self.work.table_dimension {
            Some(existing) if existing != dimension && !self.work.chunks.is_empty() => {
                Err(PgVectorError::Database(format!(
                    "the table grauwolf_chunks holds vectors of {existing} values for other \
                     collections, this model returns {dimension}"
                )))
            }
            _ => {
                self.work.table_dimension = Some(dimension);
                self.work.collections.insert(
                    collection,
                    CollectionInfo {
                        model: model.to_owned(),
                        dimension,
                    },
                );
                Ok(())
            }
        };
        Box::pin(async move { result })
    }

    fn hashes(&mut self) -> BoxFuture<'_, Result<StoredHashes, PgVectorError>> {
        let mut hashes = StoredHashes::new();
        for ((name, source, number), row) in &self.work.chunks {
            if *name == self.collection {
                hashes
                    .entry(source.clone())
                    .or_default()
                    .insert(*number, row.hash.clone());
            }
        }
        Box::pin(async move { Ok(hashes) })
    }

    fn put(&mut self, chunks: Vec<StoredChunk>) -> BoxFuture<'_, Result<(), PgVectorError>> {
        let dimension = self.work.table_dimension;
        let result = chunks.into_iter().try_for_each(|row| {
            if dimension.is_some_and(|dimension| dimension != row.embedding.0.len()) {
                return Err(PgVectorError::Database(
                    "the embedding has the wrong number of values for the table".into(),
                ));
            }
            self.work.chunks.insert(
                (self.collection.clone(), row.source.clone(), row.chunk),
                row,
            );
            Ok(())
        });
        Box::pin(async move { result })
    }

    fn delete_tail<'a>(
        &'a mut self,
        source: &'a str,
        from: usize,
    ) -> BoxFuture<'a, Result<(), PgVectorError>> {
        let collection = self.collection.clone();
        self.work.chunks.retain(|(name, row_source, number), _| {
            !(*name == collection && row_source == source && *number >= from)
        });
        Box::pin(async { Ok(()) })
    }

    fn delete_sources(&mut self, sources: Vec<String>) -> BoxFuture<'_, Result<(), PgVectorError>> {
        let collection = self.collection.clone();
        self.work
            .chunks
            .retain(|(name, source, _), _| !(*name == collection && sources.contains(source)));
        Box::pin(async { Ok(()) })
    }

    fn replace_validators(
        &mut self,
        validators: UrlValidatorMap,
    ) -> BoxFuture<'_, Result<(), PgVectorError>> {
        self.work
            .validators
            .insert(self.collection.clone(), validators);
        Box::pin(async { Ok(()) })
    }

    fn commit(self: Box<Self>) -> BoxFuture<'static, Result<(), PgVectorError>> {
        {
            // Only this collection's rows: transactions on other collections run in parallel.
            let mut shared = self.shared.lock().expect("state");
            let collection = &self.collection;
            shared.chunks.retain(|(name, _, _), _| name != collection);
            shared.chunks.extend(
                self.work
                    .chunks
                    .iter()
                    .filter(|((name, _, _), _)| name == collection)
                    .map(|(key, row)| (key.clone(), row.clone())),
            );
            if let Some(info) = self.work.collections.get(collection) {
                shared.collections.insert(collection.clone(), info.clone());
            }
            match self.work.validators.get(collection) {
                Some(validators) => {
                    shared
                        .validators
                        .insert(collection.clone(), validators.clone());
                }
                None => {
                    shared.validators.remove(collection);
                }
            }
            shared.table_dimension = self.work.table_dimension;
        }
        self.events.lock().expect("events").push("commit".into());
        Box::pin(async { Ok(()) })
    }
}

impl MemoryDb for InMemoryPgVector {
    fn prepare<'a>(
        &'a self,
        agent: &'a str,
        model: &'a str,
        _dimension: usize,
    ) -> BoxFuture<'a, Result<(), PgVectorError>> {
        let mut state = self.state.lock().expect("state");
        let result = match state.memory_models.get(agent) {
            Some(known) if known != model => Err(PgVectorError::Database(format!(
                "the memory of {agent} comes from the embedding model {known}, the spec names {model}"
            ))),
            _ => {
                state
                    .memory_models
                    .insert(agent.to_owned(), model.to_owned());
                Ok(())
            }
        };
        Box::pin(async move { result })
    }

    fn put<'a>(
        &'a self,
        area: &'a MemoryArea,
        fact: NewFact,
        now: i64,
    ) -> BoxFuture<'a, Result<(), PgVectorError>> {
        let mut state = self.state.lock().expect("state");
        state.memories.retain(|(agent, user, _), row| {
            !(agent == &area.agent
                && user == &area.user_key
                && row.fact.expires_at.is_some_and(|at| at <= now))
        });
        let key = (area.agent.clone(), area.user_key.clone(), fact.id.clone());
        match state.memories.get_mut(&key) {
            Some(row) => row.fact.expires_at = fact.expires_at,
            None => {
                state.memories.insert(
                    key,
                    MemoryRow {
                        fact,
                        access_count: 0,
                    },
                );
            }
        }
        Box::pin(async { Ok(()) })
    }

    fn nearest<'a>(
        &'a self,
        area: &'a MemoryArea,
        query: &'a Embedding,
        limit: usize,
        now: i64,
    ) -> BoxFuture<'a, Result<Vec<RecalledFact>, PgVectorError>> {
        let state = self.state.lock().expect("state");
        let mut facts: Vec<RecalledFact> = state
            .memories
            .iter()
            .filter(|((agent, user, _), row)| {
                agent == &area.agent
                    && user == &area.user_key
                    && row.fact.expires_at.is_none_or(|at| at > now)
            })
            .map(|((_, _, id), row)| RecalledFact {
                id: id.clone(),
                content: row.fact.content.clone(),
                similarity: query.cosine(&row.fact.embedding),
            })
            .collect();
        facts.sort_by(|a, b| b.similarity.total_cmp(&a.similarity));
        facts.truncate(limit);
        Box::pin(async move { Ok(facts) })
    }

    fn touch<'a>(
        &'a self,
        area: &'a MemoryArea,
        ids: &'a [String],
    ) -> BoxFuture<'a, Result<(), PgVectorError>> {
        let mut state = self.state.lock().expect("state");
        for id in ids {
            if let Some(row) =
                state
                    .memories
                    .get_mut(&(area.agent.clone(), area.user_key.clone(), id.clone()))
            {
                row.access_count += 1;
            }
        }
        Box::pin(async { Ok(()) })
    }
}
