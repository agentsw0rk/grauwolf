//! Incremental indexing of knowledge chunks (ADR-0057, ADR-0106), against the in-memory stand-in.

use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use grauwolf_rag::{Chunk, Embedder, Embedding, RagError, VectorStore};
use grauwolf_store_pgvector::{
    InMemoryPgVector, PgChunkStore, SourceChunks, SyncReport, chunk_hash, sync_collection,
};

/// Embeds a text as counts of `a`, `b`, `c` (plus padding up to `dimension`) and remembers
/// every text it was asked for.
struct Letters {
    dimension: usize,
    seen: Mutex<Vec<String>>,
}

impl Letters {
    fn new(dimension: usize) -> Arc<Self> {
        Arc::new(Self {
            dimension,
            seen: Mutex::new(Vec::new()),
        })
    }

    fn asked(&self) -> Vec<String> {
        std::mem::take(&mut *self.seen.lock().unwrap())
    }
}

impl Embedder for Letters {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Embedding>, RagError>> {
        self.seen.lock().unwrap().extend(texts.iter().cloned());
        let embeddings = texts
            .iter()
            .map(|text| {
                let mut vector = vec![0.0; self.dimension];
                for (slot, letter) in ['a', 'b', 'c'].into_iter().enumerate() {
                    vector[slot] = text.matches(letter).count() as f32;
                }
                Embedding(vector)
            })
            .collect();
        Box::pin(async move { Ok(embeddings) })
    }
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

const COLLECTION: &str = "agent/search_notes";

async fn sync(
    db: &InMemoryPgVector,
    embedder: &Letters,
    model: &str,
    sources: &[SourceChunks],
) -> SyncReport {
    sync_collection(db, embedder, COLLECTION, model, sources)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_first_run_embeds_everything_and_records_model_and_dimension() {
    let db = InMemoryPgVector::default();
    let embedder = Letters::new(4);

    let report = sync(&db, &embedder, "ollama:m", &[source("a.md", &["aa", "bb"])]).await;

    assert_eq!(
        report,
        SyncReport {
            embedded: 2,
            kept: 0,
            removed_sources: 0,
            rebuilt: false
        }
    );
    assert_eq!(
        db.chunks_of(COLLECTION),
        [
            ("a.md".into(), 0, "aa".into()),
            ("a.md".into(), 1, "bb".into())
        ]
    );
    let info = db.info_of(COLLECTION).unwrap();
    assert_eq!((info.model.as_str(), info.dimension), ("ollama:m", 4));
}

#[tokio::test]
async fn an_unchanged_second_run_embeds_nothing() {
    let db = InMemoryPgVector::default();
    let embedder = Letters::new(4);
    let sources = [source("a.md", &["aa", "bb"])];
    sync(&db, &embedder, "ollama:m", &sources).await;
    embedder.asked();

    let report = sync(&db, &embedder, "ollama:m", &sources).await;

    assert_eq!(
        report,
        SyncReport {
            embedded: 0,
            kept: 2,
            removed_sources: 0,
            rebuilt: false
        }
    );
    assert_eq!(embedder.asked(), Vec::<String>::new());
}

#[tokio::test]
async fn only_changed_and_new_chunks_are_embedded_and_surplus_chunks_are_deleted() {
    let db = InMemoryPgVector::default();
    let embedder = Letters::new(4);
    sync(
        &db,
        &embedder,
        "ollama:m",
        &[source("a.md", &["aa", "bb", "cc"])],
    )
    .await;
    embedder.asked();

    let report = sync(
        &db,
        &embedder,
        "ollama:m",
        &[source("a.md", &["aa", "bbb"]), source("b.md", &["c"])],
    )
    .await;

    assert_eq!(
        report,
        SyncReport {
            embedded: 2,
            kept: 1,
            removed_sources: 0,
            rebuilt: false
        }
    );
    assert_eq!(embedder.asked(), ["bbb", "c"]);
    assert_eq!(
        db.chunks_of(COLLECTION),
        [
            ("a.md".into(), 0, "aa".into()),
            ("a.md".into(), 1, "bbb".into()),
            ("b.md".into(), 0, "c".into()),
        ]
    );
}

#[tokio::test]
async fn chunks_of_removed_sources_are_deleted() {
    let db = InMemoryPgVector::default();
    let embedder = Letters::new(4);
    sync(
        &db,
        &embedder,
        "ollama:m",
        &[source("a.md", &["aa"]), source("b.md", &["bb"])],
    )
    .await;

    let report = sync(&db, &embedder, "ollama:m", &[source("b.md", &["bb"])]).await;

    assert_eq!(report.removed_sources, 1);
    assert_eq!(db.chunks_of(COLLECTION), [("b.md".into(), 0, "bb".into())]);
}

#[tokio::test]
async fn a_changed_model_rebuilds_the_collection() {
    let db = InMemoryPgVector::default();
    let embedder = Letters::new(4);
    let sources = [source("a.md", &["aa", "bb"])];
    sync(&db, &embedder, "ollama:m", &sources).await;
    embedder.asked();

    let report = sync(&db, &embedder, "ollama:other", &sources).await;

    assert!(report.rebuilt);
    assert_eq!((report.embedded, report.kept), (2, 0));
    assert_eq!(db.info_of(COLLECTION).unwrap().model, "ollama:other");
}

#[tokio::test]
async fn a_changed_dimension_with_the_same_model_name_rebuilds_the_collection() {
    let db = InMemoryPgVector::default();
    sync(
        &db,
        &Letters::new(4),
        "ollama:m",
        &[source("a.md", &["aa"])],
    )
    .await;

    // A new chunk is the first thing that shows the new dimension.
    let report = sync(
        &db,
        &Letters::new(6),
        "ollama:m",
        &[source("a.md", &["aa", "bb"])],
    )
    .await;

    assert!(report.rebuilt);
    assert_eq!((report.embedded, report.kept), (2, 0));
    assert_eq!(db.info_of(COLLECTION).unwrap().dimension, 6);
}

#[tokio::test]
async fn every_run_takes_the_collection_lock_and_commits() {
    let db = InMemoryPgVector::default();
    sync(
        &db,
        &Letters::new(3),
        "ollama:m",
        &[source("a.md", &["aa"])],
    )
    .await;

    assert_eq!(
        db.events(),
        [format!("lock {COLLECTION}"), "commit".to_owned()]
    );
}

#[tokio::test]
async fn replicas_indexing_at_the_same_time_embed_each_chunk_once() {
    let db = Arc::new(InMemoryPgVector::default());
    let embedder = Letters::new(3);
    let sources = Arc::new([source("a.md", &["aa", "bb"])]);

    let runs: Vec<_> = (0..4)
        .map(|_| {
            let (db, embedder, sources) =
                (Arc::clone(&db), Arc::clone(&embedder), Arc::clone(&sources));
            tokio::spawn(async move {
                sync_collection(&*db, &*embedder, COLLECTION, "ollama:m", &*sources)
                    .await
                    .unwrap()
            })
        })
        .collect();
    let mut embedded = 0;
    for run in runs {
        embedded += run.await.unwrap().embedded;
    }

    assert_eq!(embedded, 2);
}

#[tokio::test]
async fn a_failing_embedding_leaves_the_collection_unchanged() {
    struct Failing;
    impl Embedder for Failing {
        fn embed<'a>(&'a self, _: &'a [String]) -> BoxFuture<'a, Result<Vec<Embedding>, RagError>> {
            Box::pin(async { Err(RagError::Embedding("down".into())) })
        }
    }
    let db = InMemoryPgVector::default();
    let embedder = Letters::new(3);
    sync(&db, &embedder, "ollama:m", &[source("a.md", &["aa"])]).await;

    let error = sync_collection(
        &db,
        &Failing,
        COLLECTION,
        "ollama:m",
        &[source("a.md", &["zz"])],
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("down"));
    assert_eq!(db.chunks_of(COLLECTION), [("a.md".into(), 0, "aa".into())]);
}

#[tokio::test]
async fn the_store_returns_the_most_similar_chunks_of_its_collection() {
    let db = Arc::new(InMemoryPgVector::default());
    let embedder = Letters::new(3);
    sync(
        &db,
        &embedder,
        "ollama:m",
        &[source("a.md", &["aaa", "bbb"])],
    )
    .await;
    sync_collection(
        &*db,
        &*embedder,
        "other/tool",
        "ollama:m",
        &[source("x.md", &["aaaa"])],
    )
    .await
    .unwrap();
    let store = PgChunkStore::new(db, COLLECTION);

    let hits = store
        .search(&Embedding(vec![1.0, 0.0, 0.0]), 5)
        .await
        .unwrap();

    let texts: Vec<_> = hits.iter().map(|hit| hit.chunk.text.as_str()).collect();
    assert_eq!(texts, ["aaa", "bbb"]);
    assert_eq!(hits[0].chunk.source, "a.md");
}

#[test]
fn the_hash_is_stable_and_depends_on_the_text() {
    assert_eq!(chunk_hash("a"), chunk_hash("a"));
    assert_ne!(chunk_hash("a"), chunk_hash("b"));
    assert_eq!(chunk_hash("").len(), 64);
}
