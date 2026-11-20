//! The SQL the adapter sends, as texts and a few builders (pure, tested without a database).

use crate::PgVectorError;

/// Highest dimension pgvector can index with HNSW for the type `vector`.
pub const MAX_DIMENSION: usize = 2000;

/// Enables pgvector; needs the extension installed on the server.
pub const CREATE_EXTENSION: &str = "CREATE EXTENSION IF NOT EXISTS vector";

/// Model and dimension per collection (knowledge `<spec>/<tool>`, memory `memory:<agent>`).
pub const CREATE_COLLECTIONS: &str = "CREATE TABLE IF NOT EXISTS grauwolf_collections \
     (collection text PRIMARY KEY, model text NOT NULL, dimension integer NOT NULL)";

/// Cache validators of URL sources (ADR-0113).
pub const CREATE_URL_VALIDATORS: &str = "CREATE TABLE IF NOT EXISTS grauwolf_url_validators \
     (collection text NOT NULL, url text NOT NULL, etag text, last_modified text, \
     PRIMARY KEY (collection, url))";

/// Validators of collection `$1` whose URL still has chunks.
pub const SELECT_URL_VALIDATORS: &str = "SELECT v.url, v.etag, v.last_modified \
     FROM grauwolf_url_validators v WHERE v.collection = $1 AND EXISTS \
     (SELECT 1 FROM grauwolf_chunks c WHERE c.collection = v.collection AND c.source = v.url)";

/// Deletes the validators of collection `$1`.
pub const DELETE_URL_VALIDATORS: &str = "DELETE FROM grauwolf_url_validators WHERE collection = $1";

/// Stores a validator: collection `$1`, url `$2`, etag `$3`, last modified `$4`.
pub const INSERT_URL_VALIDATOR: &str = "INSERT INTO grauwolf_url_validators \
     (collection, url, etag, last_modified) VALUES ($1, $2, $3, $4)";

/// Serializes the work of several replicas on one collection until the transaction ends.
pub const LOCK_COLLECTION: &str = "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))";

/// Serializes schema changes between replicas until the transaction ends.
pub const LOCK_SCHEMA: &str =
    "SELECT pg_advisory_xact_lock(hashtextextended('grauwolf-schema', 0))";

/// Table of the knowledge chunks for vectors of `dimension` values.
pub fn create_chunks(dimension: usize) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS grauwolf_chunks (collection text NOT NULL, source text NOT NULL, \
         chunk integer NOT NULL, hash text NOT NULL, content text NOT NULL, \
         embedding vector({dimension}) NOT NULL, PRIMARY KEY (collection, source, chunk))"
    )
}

/// HNSW index (cosine) of the chunk embeddings.
pub const CREATE_CHUNKS_INDEX: &str = "CREATE INDEX IF NOT EXISTS grauwolf_chunks_embedding \
     ON grauwolf_chunks USING hnsw (embedding vector_cosine_ops)";

/// Table of the long-term memory for vectors of `dimension` values.
pub fn create_memories(dimension: usize) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS grauwolf_memories (agent text NOT NULL, user_id text NOT NULL, \
         id text NOT NULL, content text NOT NULL, embedding vector({dimension}) NOT NULL, \
         created_at timestamptz NOT NULL, expires_at timestamptz, \
         access_count bigint NOT NULL DEFAULT 0, PRIMARY KEY (agent, user_id, id))"
    )
}

/// HNSW index (cosine) of the memory embeddings.
pub const CREATE_MEMORIES_INDEX: &str = "CREATE INDEX IF NOT EXISTS grauwolf_memories_embedding \
     ON grauwolf_memories USING hnsw (embedding vector_cosine_ops)";

/// Hashes of all stored chunks of a collection (`$1`).
pub const SELECT_HASHES: &str =
    "SELECT source, chunk, hash FROM grauwolf_chunks WHERE collection = $1";

/// Inserts or replaces a chunk: collection `$1`, source `$2`, chunk `$3`, hash `$4`, content `$5`,
/// embedding `$6`.
pub const UPSERT_CHUNK: &str = "INSERT INTO grauwolf_chunks (collection, source, chunk, hash, content, embedding) \
     VALUES ($1, $2, $3, $4, $5, $6::text::vector) \
     ON CONFLICT (collection, source, chunk) DO UPDATE SET hash = EXCLUDED.hash, \
     content = EXCLUDED.content, embedding = EXCLUDED.embedding";

/// Deletes the chunks of source `$2` from number `$3` on (collection `$1`).
pub const DELETE_CHUNK_TAIL: &str =
    "DELETE FROM grauwolf_chunks WHERE collection = $1 AND source = $2 AND chunk >= $3";

/// Deletes the chunks of all sources in `$2` (collection `$1`).
pub const DELETE_SOURCES: &str =
    "DELETE FROM grauwolf_chunks WHERE collection = $1 AND source = ANY($2)";

/// Deletes all chunks of a collection (`$1`).
pub const DELETE_COLLECTION_CHUNKS: &str = "DELETE FROM grauwolf_chunks WHERE collection = $1";

/// The `limit` (`$3`) chunks of collection `$1` nearest to the query vector `$2` by cosine distance,
/// with their similarity (1 minus the distance).
pub const SEARCH_CHUNKS: &str = "SELECT source, chunk, content, \
     (1 - (embedding <=> $2::text::vector))::float4 AS similarity \
     FROM grauwolf_chunks WHERE collection = $1 \
     ORDER BY embedding <=> $2::text::vector LIMIT $3";

/// Model and dimension of a collection (`$1`).
pub const SELECT_COLLECTION: &str =
    "SELECT model, dimension FROM grauwolf_collections WHERE collection = $1";

/// Records model and dimension of a collection: collection `$1`, model `$2`, dimension `$3`.
pub const UPSERT_COLLECTION: &str = "INSERT INTO grauwolf_collections (collection, model, dimension) \
     VALUES ($1, $2, $3) ON CONFLICT (collection) DO UPDATE SET model = EXCLUDED.model, \
     dimension = EXCLUDED.dimension";

/// Dimension of the `embedding` column of the table named in `$1` (none if the table is missing).
pub const SELECT_TABLE_DIMENSION: &str = "SELECT a.atttypmod FROM pg_attribute a \
     WHERE a.attrelid = to_regclass($1) AND a.attname = 'embedding' AND NOT a.attisdropped";

/// Deletes the expired facts of an area: agent `$1`, user key `$2`, now (Unix seconds) `$3`.
pub const DELETE_EXPIRED_MEMORIES: &str = "DELETE FROM grauwolf_memories \
     WHERE agent = $1 AND user_id = $2 AND expires_at IS NOT NULL AND expires_at <= to_timestamp($3)";

/// Stores a fact: agent `$1`, user key `$2`, id `$3`, content `$4`, embedding `$5`, now `$6`,
/// expiry `$7` (Unix seconds or NULL). The same fact again only renews its expiry.
pub const UPSERT_MEMORY: &str = "INSERT INTO grauwolf_memories \
     (agent, user_id, id, content, embedding, created_at, expires_at) \
     VALUES ($1, $2, $3, $4, $5::text::vector, to_timestamp($6), to_timestamp($7)) \
     ON CONFLICT (agent, user_id, id) DO UPDATE SET expires_at = EXCLUDED.expires_at";

/// The `limit` (`$5`) unexpired facts of an area (agent `$1`, user key `$2`) nearest to the query
/// vector `$3` by cosine distance, with their similarity; `$4` is now (Unix seconds).
pub const NEAREST_MEMORIES: &str = "SELECT id, content, \
     (1 - (embedding <=> $3::text::vector))::float4 AS similarity \
     FROM grauwolf_memories WHERE agent = $1 AND user_id = $2 \
     AND (expires_at IS NULL OR expires_at > to_timestamp($4)) \
     ORDER BY embedding <=> $3::text::vector LIMIT $5";

/// Counts a recall for the facts `$3` of an area (agent `$1`, user key `$2`).
pub const TOUCH_MEMORIES: &str = "UPDATE grauwolf_memories SET access_count = access_count + 1 \
     WHERE agent = $1 AND user_id = $2 AND id = ANY($3)";

/// pgvector text literal of `values`: `[0.1,0.2]`. Bind it as text and cast it with
/// `::text::vector`.
pub fn vector_literal(values: &[f32]) -> Result<String, PgVectorError> {
    if values.iter().any(|value| !value.is_finite()) {
        return Err(PgVectorError::NonFinite);
    }
    let parts: Vec<String> = values.iter().map(f32::to_string).collect();
    Ok(format!("[{}]", parts.join(",")))
}

/// Checks that an index over `dimension` values is possible.
pub fn check_dimension(dimension: usize) -> Result<(), PgVectorError> {
    if dimension == 0 || dimension > MAX_DIMENSION {
        return Err(PgVectorError::Database(format!(
            "the embedding model returns vectors of {dimension} values; pgvector can index 1 to \
             {MAX_DIMENSION}"
        )));
    }
    Ok(())
}
