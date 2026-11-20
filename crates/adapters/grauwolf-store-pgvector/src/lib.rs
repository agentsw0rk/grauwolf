//! PostgreSQL with pgvector as the store for knowledge and long-term memory (ADR-0106).

pub mod chunks;
pub mod in_memory;
pub mod memory;
pub mod postgres;
pub mod sql;

pub use chunks::{
    ChunkDb, ChunkTx, CollectionInfo, PgChunkStore, SourceChunks, StoredChunk, SyncReport,
    UrlValidatorMap, UrlValidators, chunk_hash, sync_collection, sync_collection_with,
};
pub use in_memory::InMemoryPgVector;
pub use memory::{
    MemoryArea, MemoryDb, MemoryPolicy, NewFact, Now, PgMemoryStore, RecalledFact, system_now,
    user_key,
};
pub use postgres::PgVector;

/// Error of the pgvector store; texts never contain a connection URL or password.
#[derive(Debug, thiserror::Error)]
pub enum PgVectorError {
    /// The connection could not be opened.
    #[error("{0}")]
    Connect(String),
    /// A statement failed or the schema does not fit.
    #[error("{0}")]
    Database(String),
    /// An embedding contains a value PostgreSQL cannot store (NaN or infinity).
    #[error("the embedding contains a value that is not a finite number")]
    NonFinite,
}
