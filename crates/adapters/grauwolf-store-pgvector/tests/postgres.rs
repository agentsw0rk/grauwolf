//! Against a real PostgreSQL with pgvector. Runs only with `PGVECTOR_TEST_URL` (for example
//! `postgres://postgres:postgres@localhost:5432/postgres` from `pgvector/pgvector`); without it
//! every test returns early. The tests use vectors of 3 values and unique names, and need a
//! database whose pgvector tables, if present, are empty or of dimension 3.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::future::BoxFuture;
use grauwolf_rag::{Chunk, Embedder, Embedding, RagError, VectorStore};
use grauwolf_store_pgvector::{
    MemoryDb, MemoryPolicy, PgChunkStore, PgMemoryStore, PgVector, SourceChunks, sync_collection,
    system_now,
};

struct Letters;

impl Embedder for Letters {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Embedding>, RagError>> {
        let embeddings = texts
            .iter()
            .map(|text| {
                Embedding(
                    ['a', 'b', 'c']
                        .map(|letter| text.matches(letter).count() as f32 + 0.01)
                        .to_vec(),
                )
            })
            .collect();
        Box::pin(async move { Ok(embeddings) })
    }
}

async fn database() -> Option<Arc<PgVector>> {
    let url = std::env::var("PGVECTOR_TEST_URL").ok()?;
    Some(PgVector::connect("PGVECTOR_TEST_URL", &url).await.unwrap())
}

fn unique(name: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("test-{name}-{nanos}")
}

fn source(name: &str, texts: &[&str]) -> SourceChunks {
    SourceChunks {
        source: name.into(),
        chunks: texts
            .iter()
            .enumerate()
            .map(|(number, text)| Chunk {
                id: format!("{name}#{number}"),
                source: name.into(),
                text: (*text).into(),
            })
            .collect(),
    }
}

#[tokio::test]
async fn indexes_incrementally_and_searches_by_similarity() {
    let Some(db) = database().await else { return };
    let collection = unique("knowledge");

    let first = sync_collection(
        &*db,
        &Letters,
        &collection,
        "test:m",
        &[source("a.md", &["aaa", "bbb"])],
    )
    .await
    .unwrap();
    let second = sync_collection(
        &*db,
        &Letters,
        &collection,
        "test:m",
        &[source("a.md", &["aaa", "ccc"])],
    )
    .await
    .unwrap();

    assert_eq!((first.embedded, second.embedded, second.kept), (2, 1, 1));
    let store = PgChunkStore::new(db.clone(), collection);
    let hits = store
        .search(&Embedding(vec![1.0, 0.0, 0.0]), 2)
        .await
        .unwrap();
    assert_eq!(hits[0].chunk.text, "aaa");
    assert_eq!(hits[0].chunk.source, "a.md");
    assert!(hits.iter().all(|hit| hit.chunk.text != "bbb"));
}

#[tokio::test]
async fn memory_separates_users_and_expires_facts() {
    let Some(db) = database().await else { return };
    let agent = unique("agent");
    db.prepare(&agent, "test:m", 3).await.unwrap();
    let policy = MemoryPolicy {
        ttl_days: Some(1),
        min_similarity: Some(0.5),
    };
    let remember =
        |user: Option<&str>| PgMemoryStore::new(db.clone(), &agent, user, policy, system_now());
    let fact = |text: &str| Chunk {
        id: "memory".into(),
        source: "memory".into(),
        text: text.into(),
    };
    let query = Embedding(vec![1.0, 0.0, 0.0]);

    remember(Some("anna"))
        .add(fact("anna likes tea"), query.clone())
        .await
        .unwrap();
    remember(None)
        .add(fact("shared"), query.clone())
        .await
        .unwrap();
    remember(Some("anna"))
        .add(fact("anna likes tea"), query.clone())
        .await
        .unwrap();

    let anna = remember(Some("anna")).search(&query, 5).await.unwrap();
    assert_eq!(anna.len(), 1);
    assert_eq!(anna[0].chunk.text, "anna likes tea");
    assert!(
        remember(Some("ben"))
            .search(&query, 5)
            .await
            .unwrap()
            .is_empty()
    );
    let orthogonal = Embedding(vec![0.0, 1.0, 0.0]);
    assert!(
        remember(Some("anna"))
            .search(&orthogonal, 5)
            .await
            .unwrap()
            .is_empty()
    );

    let later = PgMemoryStore::new(
        db.clone(),
        &agent,
        Some("anna"),
        policy,
        Arc::new(|| system_now()() + 3 * 86_400),
    );
    assert!(later.search(&query, 5).await.unwrap().is_empty());
}
