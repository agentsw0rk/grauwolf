//! Knowledge chunks in PostgreSQL: incremental indexing and search (ADR-0057, ADR-0106).
//!
//! The database sits behind the small traits [`ChunkDb`] and [`ChunkTx`]; the indexing logic
//! ([`sync_collection`]) is plain Rust on top of them, so it runs against the in-memory stand-in
//! in tests and against PostgreSQL in production.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_rag::{Chunk, Embedder, Embedding, RagError, ScoredChunk, VectorStore};
use sha2::{Digest, Sha256};

use crate::PgVectorError;

/// Chunks per embedding request.
const BATCH: usize = 16;

/// Text embedded to learn the dimension of a model.
const PROBE: &str = "dimension probe";

/// What the database knows about a collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionInfo {
    /// `<provider>:<model>` of the stored embeddings.
    pub model: String,
    /// Number of values per embedding.
    pub dimension: usize,
}

/// A chunk row.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredChunk {
    /// Source (path relative to the spec).
    pub source: String,
    /// Position of the chunk within its source.
    pub chunk: usize,
    /// Hash of the content ([`chunk_hash`]).
    pub hash: String,
    /// Text of the chunk.
    pub content: String,
    /// Embedding of the text.
    pub embedding: Embedding,
}

/// Cache validators of a URL source (ADR-0113).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UrlValidators {
    /// `ETag` of the last response.
    pub etag: Option<String>,
    /// `Last-Modified` of the last response.
    pub last_modified: Option<String>,
}

/// Validators per URL (the source name of its chunks).
pub type UrlValidatorMap = BTreeMap<String, UrlValidators>;

/// Hashes of the stored chunks: source → chunk number → hash.
pub type StoredHashes = BTreeMap<String, BTreeMap<usize, String>>;

/// The database side of the knowledge store.
pub trait ChunkDb: Send + Sync {
    /// Starts a transaction for `collection` and takes the collection's lock until it ends, so
    /// replicas index one after the other.
    fn begin<'a>(
        &'a self,
        collection: &'a str,
    ) -> BoxFuture<'a, Result<Box<dyn ChunkTx + 'a>, PgVectorError>>;

    /// Validators of the URL sources of `collection` that still have chunks stored.
    fn url_validators<'a>(
        &'a self,
        collection: &'a str,
    ) -> BoxFuture<'a, Result<UrlValidatorMap, PgVectorError>>;

    /// The `limit` chunks of `collection` most similar to `query`, most similar first.
    fn search<'a>(
        &'a self,
        collection: &'a str,
        query: &'a Embedding,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<ScoredChunk>, PgVectorError>>;
}

/// One locked transaction on a collection. Dropping it without [`commit`](Self::commit) discards
/// the changes.
pub trait ChunkTx: Send {
    /// Model and dimension recorded for the collection, if it exists.
    fn info(&mut self) -> BoxFuture<'_, Result<Option<CollectionInfo>, PgVectorError>>;

    /// Empties the collection and records `model` and `dimension`; creates the chunk table for
    /// `dimension` if needed.
    fn reset<'a>(
        &'a mut self,
        model: &'a str,
        dimension: usize,
    ) -> BoxFuture<'a, Result<(), PgVectorError>>;

    /// Hashes of the stored chunks.
    fn hashes(&mut self) -> BoxFuture<'_, Result<StoredHashes, PgVectorError>>;

    /// Inserts or replaces chunks.
    fn put(&mut self, chunks: Vec<StoredChunk>) -> BoxFuture<'_, Result<(), PgVectorError>>;

    /// Deletes the chunks of `source` from number `from` on.
    fn delete_tail<'a>(
        &'a mut self,
        source: &'a str,
        from: usize,
    ) -> BoxFuture<'a, Result<(), PgVectorError>>;

    /// Deletes all chunks of `sources`.
    fn delete_sources(&mut self, sources: Vec<String>) -> BoxFuture<'_, Result<(), PgVectorError>>;

    /// Replaces the stored validators of the collection's URL sources.
    fn replace_validators(
        &mut self,
        validators: UrlValidatorMap,
    ) -> BoxFuture<'_, Result<(), PgVectorError>>;

    /// Makes the changes permanent and releases the lock.
    fn commit(self: Box<Self>) -> BoxFuture<'static, Result<(), PgVectorError>>;
}

/// The chunks of one source, in order.
#[derive(Debug, Clone)]
pub struct SourceChunks {
    /// Source (path relative to the spec).
    pub source: String,
    /// Its chunks; the position in the list is the chunk number.
    pub chunks: Vec<Chunk>,
}

/// What an indexing run did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// Chunks embedded and written.
    pub embedded: usize,
    /// Chunks that were already stored unchanged.
    pub kept: usize,
    /// Sources removed because they are gone.
    pub removed_sources: usize,
    /// The collection was emptied first because model or dimension changed.
    pub rebuilt: bool,
}

/// SHA-256 of a chunk's text as hex.
pub fn chunk_hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Brings `collection` in line with `sources` (ADR-0057): only new or changed chunks are embedded,
/// chunks of removed sources and surplus chunks are deleted, and a changed `model` or embedding
/// dimension rebuilds the collection. Everything happens in one transaction under the
/// collection's lock; on failure nothing changes.
pub async fn sync_collection(
    db: &dyn ChunkDb,
    embedder: &dyn Embedder,
    collection: &str,
    model: &str,
    sources: &[SourceChunks],
) -> Result<SyncReport, PgVectorError> {
    sync_collection_with(
        db,
        embedder,
        collection,
        model,
        sources,
        &[],
        &UrlValidatorMap::new(),
    )
    .await
}

/// Like [`sync_collection`] for collections with URL sources (ADR-0113): the sources named in
/// `keep` stay as stored (a `304` or a failed fetch) although `sources` does not carry them, and
/// `validators` replace the stored ones. A rebuild (changed model or dimension) cannot keep
/// anything and fails, so that the next start fetches everything again.
pub async fn sync_collection_with(
    db: &dyn ChunkDb,
    embedder: &dyn Embedder,
    collection: &str,
    model: &str,
    sources: &[SourceChunks],
    keep: &[String],
    validators: &UrlValidatorMap,
) -> Result<SyncReport, PgVectorError> {
    let mut force_rebuild = false;
    let plan = Plan {
        sources,
        keep,
        validators,
    };
    loop {
        match attempt(db, embedder, collection, model, &plan, force_rebuild).await? {
            Attempt::Done(report) => return Ok(report),
            Attempt::DimensionChanged if force_rebuild => {
                return Err(PgVectorError::Database(
                    "the embedding model returned vectors of differing lengths".into(),
                ));
            }
            Attempt::DimensionChanged => force_rebuild = true,
        }
    }
}

struct Plan<'a> {
    sources: &'a [SourceChunks],
    keep: &'a [String],
    validators: &'a UrlValidatorMap,
}

enum Attempt {
    Done(SyncReport),
    DimensionChanged,
}

fn embedding_error(error: RagError) -> PgVectorError {
    PgVectorError::Database(error.to_string())
}

async fn attempt(
    db: &dyn ChunkDb,
    embedder: &dyn Embedder,
    collection: &str,
    model: &str,
    plan: &Plan<'_>,
    force_rebuild: bool,
) -> Result<Attempt, PgVectorError> {
    let sources = plan.sources;
    let mut tx = db.begin(collection).await?;
    let info = tx.info().await?;
    let mut report = SyncReport::default();
    let dimension = match info {
        Some(info) if info.model == model && !force_rebuild => info.dimension,
        previous => {
            let probe = embedder
                .embed(&[PROBE.to_owned()])
                .await
                .map_err(embedding_error)?;
            let dimension = probe.first().map_or(0, |embedding| embedding.0.len());
            crate::sql::check_dimension(dimension)?;
            if !plan.keep.is_empty() {
                return Err(PgVectorError::Database(
                    "the collection had to be rebuilt (embedding model or dimension changed); \
                     start again to fetch the URLs anew"
                        .into(),
                ));
            }
            tx.reset(model, dimension).await?;
            report.rebuilt = previous.is_some();
            dimension
        }
    };

    let stored = tx.hashes().await?;
    let current: BTreeSet<&str> = sources
        .iter()
        .map(|part| part.source.as_str())
        .chain(plan.keep.iter().map(String::as_str))
        .collect();
    let gone: Vec<String> = stored
        .keys()
        .filter(|source| !current.contains(source.as_str()))
        .cloned()
        .collect();
    report.removed_sources = gone.len();
    if !gone.is_empty() {
        tx.delete_sources(gone).await?;
    }

    let mut pending: Vec<StoredChunk> = Vec::new();
    for part in sources {
        let known = stored.get(&part.source);
        for (number, chunk) in part.chunks.iter().enumerate() {
            let hash = chunk_hash(&chunk.text);
            if known.and_then(|hashes| hashes.get(&number)) == Some(&hash) {
                report.kept += 1;
            } else {
                pending.push(StoredChunk {
                    source: part.source.clone(),
                    chunk: number,
                    hash,
                    content: chunk.text.clone(),
                    embedding: Embedding(Vec::new()),
                });
            }
        }
        if known.is_some_and(|hashes| hashes.keys().any(|number| *number >= part.chunks.len())) {
            tx.delete_tail(&part.source, part.chunks.len()).await?;
        }
    }

    for batch in pending.chunks_mut(BATCH) {
        let texts: Vec<String> = batch.iter().map(|chunk| chunk.content.clone()).collect();
        let embeddings = embedder.embed(&texts).await.map_err(embedding_error)?;
        if embeddings.len() != batch.len() {
            return Err(PgVectorError::Database(format!(
                "the embedder returned {} embeddings for {} texts",
                embeddings.len(),
                batch.len()
            )));
        }
        if embeddings
            .iter()
            .any(|embedding| embedding.0.len() != dimension)
        {
            return Ok(Attempt::DimensionChanged);
        }
        for (chunk, embedding) in batch.iter_mut().zip(embeddings) {
            chunk.embedding = embedding;
        }
        report.embedded += batch.len();
        tx.put(batch.to_vec()).await?;
    }
    tx.replace_validators(plan.validators.clone()).await?;
    tx.commit().await?;
    Ok(Attempt::Done(report))
}

/// The knowledge collection `collection` as a [`VectorStore`]: searches the database. Indexing
/// goes through [`sync_collection`]; `add` stores a single chunk (number taken from the id
/// `<source>#<n>`).
pub struct PgChunkStore {
    db: Arc<dyn ChunkDb>,
    collection: String,
}

impl PgChunkStore {
    /// Store for `collection` on `db`.
    pub fn new(db: Arc<dyn ChunkDb>, collection: impl Into<String>) -> Self {
        Self {
            db,
            collection: collection.into(),
        }
    }
}

fn store_error(error: PgVectorError) -> RagError {
    RagError::Store(Box::new(error))
}

impl VectorStore for PgChunkStore {
    fn add<'a>(
        &'a self,
        chunk: Chunk,
        embedding: Embedding,
    ) -> BoxFuture<'a, Result<(), RagError>> {
        Box::pin(async move {
            let number = chunk
                .id
                .rsplit_once('#')
                .and_then(|(_, number)| number.parse().ok())
                .unwrap_or(0);
            let mut tx = self.db.begin(&self.collection).await.map_err(store_error)?;
            tx.put(vec![StoredChunk {
                source: chunk.source,
                chunk: number,
                hash: chunk_hash(&chunk.text),
                content: chunk.text,
                embedding,
            }])
            .await
            .map_err(store_error)?;
            tx.commit().await.map_err(store_error)
        })
    }

    fn search<'a>(
        &'a self,
        query: &'a Embedding,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<ScoredChunk>, RagError>> {
        Box::pin(async move {
            self.db
                .search(&self.collection, query, limit)
                .await
                .map_err(store_error)
        })
    }
}
