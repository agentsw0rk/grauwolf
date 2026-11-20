//! Long-term memory in PostgreSQL (ADR-0106): users, expiry, minimum similarity, against the
//! in-memory stand-in.

use std::sync::{Arc, Mutex};

use grauwolf_rag::{Chunk, Embedding, VectorStore};
use grauwolf_store_pgvector::{
    InMemoryPgVector, MemoryDb, MemoryPolicy, Now, PgMemoryStore, user_key,
};

fn clock() -> (Now, Arc<Mutex<i64>>) {
    let time = Arc::new(Mutex::new(1_000_000));
    let shared = Arc::clone(&time);
    (Arc::new(move || *shared.lock().unwrap()), time)
}

fn store(
    db: &Arc<InMemoryPgVector>,
    user: Option<&str>,
    policy: MemoryPolicy,
    now: &Now,
) -> PgMemoryStore {
    PgMemoryStore::new(db.clone(), "agent", user, policy, Arc::clone(now))
}

fn fact(text: &str) -> Chunk {
    Chunk {
        id: "memory".into(),
        source: "memory".into(),
        text: text.into(),
    }
}

fn east() -> Embedding {
    Embedding(vec![1.0, 0.0])
}

fn north() -> Embedding {
    Embedding(vec![0.0, 1.0])
}

async fn texts(store: &PgMemoryStore, query: &Embedding, limit: usize) -> Vec<String> {
    store
        .search(query, limit)
        .await
        .unwrap()
        .into_iter()
        .map(|hit| hit.chunk.text)
        .collect()
}

#[tokio::test]
async fn recalls_the_most_similar_facts_first_and_at_most_the_limit() {
    let db = Arc::new(InMemoryPgVector::default());
    let (now, _) = clock();
    let memory = store(&db, None, MemoryPolicy::default(), &now);
    memory.add(fact("north"), north()).await.unwrap();
    memory.add(fact("east"), east()).await.unwrap();
    memory
        .add(fact("mostly east"), Embedding(vec![1.0, 0.2]))
        .await
        .unwrap();

    assert_eq!(texts(&memory, &east(), 2).await, ["east", "mostly east"]);
}

#[tokio::test]
async fn the_same_fact_is_stored_once_per_area() {
    let db = Arc::new(InMemoryPgVector::default());
    let (now, _) = clock();
    let memory = store(&db, Some("anna"), MemoryPolicy::default(), &now);
    memory.add(fact("likes tea"), east()).await.unwrap();
    memory.add(fact("likes tea"), east()).await.unwrap();

    assert_eq!(db.memories_of("agent", &user_key(Some("anna"))).len(), 1);
}

#[tokio::test]
async fn users_and_the_shared_area_are_separate() {
    let db = Arc::new(InMemoryPgVector::default());
    let (now, _) = clock();
    let policy = MemoryPolicy::default();
    store(&db, Some("anna"), policy, &now)
        .add(fact("anna's fact"), east())
        .await
        .unwrap();
    store(&db, None, policy, &now)
        .add(fact("shared fact"), east())
        .await
        .unwrap();

    assert_eq!(
        texts(&store(&db, Some("anna"), policy, &now), &east(), 5).await,
        ["anna's fact"]
    );
    assert_eq!(
        texts(&store(&db, Some("ben"), policy, &now), &east(), 5).await,
        Vec::<String>::new()
    );
    assert_eq!(
        texts(&store(&db, None, policy, &now), &east(), 5).await,
        ["shared fact"]
    );
}

#[tokio::test]
async fn a_user_can_never_reach_the_shared_area_by_name() {
    assert_ne!(user_key(Some("")), user_key(None));
    assert_eq!(user_key(None), "");
}

#[tokio::test]
async fn expired_facts_are_never_returned() {
    let db = Arc::new(InMemoryPgVector::default());
    let (now, time) = clock();
    let memory = store(
        &db,
        None,
        MemoryPolicy {
            ttl_days: Some(2),
            min_similarity: None,
        },
        &now,
    );
    memory.add(fact("short lived"), east()).await.unwrap();

    *time.lock().unwrap() += 2 * 86_400 - 1;
    assert_eq!(texts(&memory, &east(), 5).await, ["short lived"]);
    *time.lock().unwrap() += 1;
    assert_eq!(texts(&memory, &east(), 5).await, Vec::<String>::new());
}

#[tokio::test]
async fn facts_without_a_ttl_never_expire() {
    let db = Arc::new(InMemoryPgVector::default());
    let (now, time) = clock();
    let memory = store(&db, None, MemoryPolicy::default(), &now);
    memory.add(fact("forever"), east()).await.unwrap();

    *time.lock().unwrap() += 10_000 * 86_400;

    assert_eq!(texts(&memory, &east(), 5).await, ["forever"]);
}

#[tokio::test]
async fn expired_facts_are_deleted_on_the_next_write_of_the_area() {
    let db = Arc::new(InMemoryPgVector::default());
    let (now, time) = clock();
    let memory = store(
        &db,
        None,
        MemoryPolicy {
            ttl_days: Some(1),
            min_similarity: None,
        },
        &now,
    );
    memory.add(fact("old"), east()).await.unwrap();
    *time.lock().unwrap() += 2 * 86_400;
    // Reading does not delete.
    texts(&memory, &east(), 5).await;
    assert_eq!(db.memories_of("agent", "").len(), 1);

    memory.add(fact("new"), north()).await.unwrap();

    let left: Vec<_> = db
        .memories_of("agent", "")
        .into_iter()
        .map(|(text, _)| text)
        .collect();
    assert_eq!(left, ["new"]);
}

#[tokio::test]
async fn storing_a_fact_again_renews_its_expiry() {
    let db = Arc::new(InMemoryPgVector::default());
    let (now, time) = clock();
    let memory = store(
        &db,
        None,
        MemoryPolicy {
            ttl_days: Some(2),
            min_similarity: None,
        },
        &now,
    );
    memory.add(fact("renewed"), east()).await.unwrap();
    *time.lock().unwrap() += 86_400;
    memory.add(fact("renewed"), east()).await.unwrap();
    *time.lock().unwrap() += 86_400 + 10;

    assert_eq!(texts(&memory, &east(), 5).await, ["renewed"]);
}

#[tokio::test]
async fn weak_matches_below_the_minimum_similarity_are_dropped() {
    let db = Arc::new(InMemoryPgVector::default());
    let (now, _) = clock();
    let policy = MemoryPolicy {
        ttl_days: None,
        min_similarity: Some(0.5),
    };
    let memory = store(&db, None, policy, &now);
    memory.add(fact("east"), east()).await.unwrap();
    memory.add(fact("north"), north()).await.unwrap();

    assert_eq!(texts(&memory, &east(), 5).await, ["east"]);
}

#[tokio::test]
async fn recall_counts_only_the_returned_facts() {
    let db = Arc::new(InMemoryPgVector::default());
    let (now, _) = clock();
    let policy = MemoryPolicy {
        ttl_days: None,
        min_similarity: Some(0.5),
    };
    let memory = store(&db, None, policy, &now);
    memory.add(fact("east"), east()).await.unwrap();
    memory.add(fact("north"), north()).await.unwrap();

    texts(&memory, &east(), 5).await;
    texts(&memory, &east(), 5).await;

    assert_eq!(
        db.memories_of("agent", ""),
        [("east".to_owned(), 2), ("north".to_owned(), 0)]
    );
}

#[tokio::test]
async fn another_embedding_model_than_the_recorded_one_is_an_error() {
    let db = InMemoryPgVector::default();
    db.prepare("agent", "ollama:m", 4).await.unwrap();

    db.prepare("agent", "ollama:m", 4).await.unwrap();
    let error = db.prepare("agent", "openai:other", 4).await.unwrap_err();

    assert!(error.to_string().contains("ollama:m"));
}
