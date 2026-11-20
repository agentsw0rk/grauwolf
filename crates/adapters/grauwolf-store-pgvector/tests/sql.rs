//! The vector literal and the SQL texts (no database needed).

use grauwolf_store_pgvector::sql::{self, vector_literal};

#[test]
fn writes_a_vector_as_a_pgvector_text_literal() {
    assert_eq!(vector_literal(&[0.1, -2.0, 3.5]).unwrap(), "[0.1,-2,3.5]");
    assert_eq!(vector_literal(&[]).unwrap(), "[]");
}

#[test]
fn refuses_values_that_postgres_cannot_store() {
    assert!(vector_literal(&[1.0, f32::NAN]).is_err());
    assert!(vector_literal(&[f32::INFINITY]).is_err());
}

#[test]
fn the_literal_reads_back_to_the_same_values() {
    let values = [0.123_456_79_f32, 1e-7, -3.4e20];
    let literal = vector_literal(&values).unwrap();
    let parsed: Vec<f32> = literal
        .trim_matches(['[', ']'])
        .split(',')
        .map(|part| part.parse().unwrap())
        .collect();
    assert_eq!(parsed, values);
}

#[test]
fn the_chunk_table_has_the_documented_columns_and_the_dimension() {
    let table = sql::create_chunks(768);
    for part in [
        "grauwolf_chunks",
        "collection text",
        "source text",
        "chunk integer",
        "hash text",
        "content text",
        "embedding vector(768)",
        "PRIMARY KEY (collection, source, chunk)",
    ] {
        assert!(table.contains(part), "{part} missing in {table}");
    }
    assert!(sql::CREATE_CHUNKS_INDEX.contains("hnsw (embedding vector_cosine_ops)"));
}

#[test]
fn the_memory_table_has_the_documented_columns_and_the_dimension() {
    let table = sql::create_memories(4);
    for part in [
        "grauwolf_memories",
        "agent text",
        "user_id text",
        "id text",
        "embedding vector(4)",
        "created_at timestamptz",
        "expires_at timestamptz",
        "access_count bigint",
        "PRIMARY KEY (agent, user_id, id)",
    ] {
        assert!(table.contains(part), "{part} missing in {table}");
    }
    assert!(sql::CREATE_MEMORIES_INDEX.contains("hnsw (embedding vector_cosine_ops)"));
}

#[test]
fn collections_are_serialized_with_an_advisory_transaction_lock() {
    assert!(sql::LOCK_COLLECTION.contains("pg_advisory_xact_lock"));
    assert!(sql::LOCK_SCHEMA.contains("pg_advisory_xact_lock"));
}

#[test]
fn searches_are_scoped_and_cosine_ordered() {
    assert!(sql::SEARCH_CHUNKS.contains("WHERE collection = $1"));
    assert!(sql::SEARCH_CHUNKS.contains("ORDER BY embedding <=> $2::text::vector LIMIT $3"));
    assert!(sql::NEAREST_MEMORIES.contains("agent = $1 AND user_id = $2"));
    assert!(sql::NEAREST_MEMORIES.contains("expires_at IS NULL OR expires_at > to_timestamp($4)"));
    assert!(sql::NEAREST_MEMORIES.contains("ORDER BY embedding <=> $3::text::vector LIMIT $5"));
}

#[test]
fn rejects_dimensions_pgvector_cannot_index() {
    assert!(sql::check_dimension(1536).is_ok());
    assert!(sql::check_dimension(0).is_err());
    assert!(sql::check_dimension(3072).is_err());
}
