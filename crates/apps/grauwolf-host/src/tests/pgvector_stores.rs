//! Knowledge and long-term memory in PostgreSQL with pgvector (ADR-0106), against the in-memory
//! stand-in of the adapter: no database needed.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::config::MapEnv;
use crate::knowledge::{search_tool_in, search_tool_on};
use crate::long_term_memory::LongTermMemory;
use futures_util::future::BoxFuture;
use grauwolf_rag::{Embedder, Embedding, RagError};
use grauwolf_spec::{
    KnowledgeSpec, KnowledgeStoreKind, KnowledgeStoreSpec, LongTermMemorySpec,
    MemoryVectorStoreSpec, ModelSpec, VectorStoreKind,
};
use grauwolf_store_pgvector::InMemoryPgVector;
use serde_json::json;

/// Counts the words of a fixed vocabulary and the embedded texts.
#[derive(Default)]
struct WordCounter(AtomicUsize);

impl Embedder for WordCounter {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Embedding>, RagError>> {
        self.0.fetch_add(texts.len(), Ordering::Relaxed);
        let embeddings = texts
            .iter()
            .map(|text| {
                let text = text.to_lowercase();
                Embedding(
                    ["cat", "rust", "holiday"]
                        .iter()
                        .map(|word| text.matches(word).count() as f32)
                        .collect(),
                )
            })
            .collect();
        Box::pin(async move { Ok(embeddings) })
    }
}

fn model() -> ModelSpec {
    ModelSpec {
        provider: "ollama".into(),
        id: "nomic-embed-text".into(),
        fallbacks: Vec::new(),
        temperature: None,
        think: None,
    }
}

fn knowledge() -> KnowledgeSpec {
    KnowledgeSpec {
        sources: vec!["notes".into()],
        urls: Vec::new(),
        min_similarity: None,
        inject: false,
        tool_enabled: true,
        embedding_model: model(),
        tool: "search_notes".into(),
        top_k: 1,
        chunk_size: 200,
        index: None,
        store: Some(KnowledgeStoreSpec {
            kind: KnowledgeStoreKind::Postgres,
            url_env: Some("PGVECTOR_URL".into()),
        }),
    }
}

fn notes(name: &str) -> grauwolf_test::TestDir {
    let base = grauwolf_test::TestDir::new(&format!("pgvector-{name}"));
    std::fs::create_dir_all(base.join("notes")).unwrap();
    std::fs::write(base.join("notes/cat.md"), "The cat is named Minka.").unwrap();
    std::fs::write(base.join("notes/rust.md"), "Rust is a language.").unwrap();
    base
}

#[tokio::test]
async fn the_knowledge_lands_in_the_database_and_is_embedded_once() {
    let base = notes("knowledge");
    let db = Arc::new(InMemoryPgVector::default());
    let embedder = Arc::new(WordCounter::default());

    let tool = search_tool_on("agent", &knowledge(), &base, embedder.clone(), db.clone())
        .await
        .unwrap();
    let after_first = embedder.0.load(Ordering::Relaxed);
    search_tool_on("agent", &knowledge(), &base, embedder.clone(), db.clone())
        .await
        .unwrap();
    // The second start embedded nothing: not the chunks, not even a dimension probe.
    assert_eq!(embedder.0.load(Ordering::Relaxed), after_first);

    let answer = tool.call_json(json!({ "query": "cat" })).await.unwrap();
    assert_eq!(answer, "[notes/cat.md] The cat is named Minka.");
    assert_eq!(
        db.chunks_of("agent/search_notes").len(),
        2,
        "collection is <spec name>/<tool>"
    );
}

#[tokio::test]
async fn a_missing_url_variable_is_named() {
    let base = notes("missing");

    let error = search_tool_in(
        "agent",
        &knowledge(),
        &base,
        Arc::new(WordCounter::default()),
        &MapEnv::default(),
    )
    .await
    .err()
    .unwrap();

    assert!(
        error.contains("PGVECTOR_URL") && error.contains("not set"),
        "{error}"
    );
}

#[tokio::test]
async fn a_database_that_cannot_be_reached_is_reported_without_the_password() {
    let base = notes("refused");
    let env = MapEnv::new([("PGVECTOR_URL", "postgres://agent:s3cret@127.0.0.1:1/db")]);

    let error = search_tool_in(
        "agent",
        &knowledge(),
        &base,
        Arc::new(WordCounter::default()),
        &env,
    )
    .await
    .err()
    .unwrap();

    assert!(error.contains("PGVECTOR_URL"), "{error}");
    assert!(!error.contains("s3cret"), "{error}");
}

fn memory(ttl_days: Option<u32>, min_similarity: Option<f32>) -> LongTermMemorySpec {
    LongTermMemorySpec {
        store: None,
        vector_store: Some(MemoryVectorStoreSpec {
            kind: VectorStoreKind::Postgres,
            url_env: "PGVECTOR_URL".into(),
            ttl_days,
            min_similarity,
        }),
        embedding_model: model(),
        remember_tool: "remember".into(),
        recall_tool: "recall".into(),
        top_k: 3,
        auto_recall: false,
        auto_remember: false,
        tools: true,
        remember_compressed: false,
    }
}

#[tokio::test]
async fn the_memory_remembers_per_user_and_applies_the_minimum_similarity() {
    let db = Arc::new(InMemoryPgVector::default());
    let memory = LongTermMemory::open_on(
        "agent",
        &memory(None, Some(0.5)),
        Arc::new(WordCounter::default()),
        db,
    )
    .await
    .unwrap();
    let [remember, _] = memory.tools(Some("anna"));
    remember
        .call_json(json!({ "fact": "The cat is named Mia." }))
        .await
        .unwrap();
    remember
        .call_json(json!({ "fact": "The cat is named Mia." }))
        .await
        .unwrap();
    let [remember, _] = memory.tools(Some("anna"));
    remember
        .call_json(json!({ "fact": "Rust is a language." }))
        .await
        .unwrap();

    let [_, recall] = memory.tools(Some("anna"));
    assert_eq!(
        recall.call_json(json!({ "query": "cat" })).await.unwrap(),
        "The cat is named Mia."
    );
    let [_, ben] = memory.tools(Some("ben"));
    assert_eq!(
        ben.call_json(json!({ "query": "cat" })).await.unwrap(),
        "No matches."
    );
    let [_, shared] = memory.tools(None);
    assert_eq!(
        shared.call_json(json!({ "query": "cat" })).await.unwrap(),
        "No matches."
    );
}

#[tokio::test]
async fn another_embedding_model_for_the_same_agent_is_refused() {
    let db = Arc::new(InMemoryPgVector::default());
    LongTermMemory::open_on(
        "agent",
        &memory(None, None),
        Arc::new(WordCounter::default()),
        db.clone(),
    )
    .await
    .unwrap();
    let mut other = memory(None, None);
    other.embedding_model.id = "other".into();

    let error = LongTermMemory::open_on("agent", &other, Arc::new(WordCounter::default()), db)
        .await
        .err()
        .unwrap();

    assert!(error.contains("ollama:nomic-embed-text"), "{error}");
}

#[tokio::test]
async fn the_postgres_memory_without_a_url_variable_names_it() {
    let error = LongTermMemory::open_in(
        "agent",
        &memory(Some(30), None),
        std::path::Path::new("."),
        Arc::new(WordCounter::default()),
        &MapEnv::default(),
    )
    .await
    .err()
    .unwrap();

    assert!(error.contains("PGVECTOR_URL"), "{error}");
}

#[tokio::test]
async fn min_similarity_applies_to_the_database_search_too() {
    let base = notes("threshold");
    let mut spec = knowledge();
    spec.min_similarity = Some(0.9);
    spec.top_k = 2;
    let db = Arc::new(InMemoryPgVector::default());

    let tool = search_tool_on("agent", &spec, &base, Arc::new(WordCounter::default()), db)
        .await
        .unwrap();

    assert_eq!(
        tool.call_json(json!({ "query": "cat" })).await.unwrap(),
        "[notes/cat.md] The cat is named Minka."
    );
    assert_eq!(
        tool.call_json(json!({ "query": "holiday" })).await.unwrap(),
        "No matches."
    );
}

mod urls {
    use super::*;
    use wiremock::matchers::{header_exists, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn serve(body: &'static str, etag: &'static str) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header_exists("if-none-match"))
            .respond_with(ResponseTemplate::new(304))
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(body, "text/html")
                    .insert_header("etag", etag),
            )
            .with_priority(5)
            .mount(&server)
            .await;
        server
    }

    fn url_knowledge(server: &MockServer) -> KnowledgeSpec {
        let mut spec = knowledge();
        spec.sources = Vec::new();
        spec.urls = vec![format!("{}/pets", server.uri())];
        spec.chunk_size = 30;
        spec
    }

    #[tokio::test]
    async fn embeds_a_url_once_and_only_the_changed_chunks_after_a_change() {
        let server = serve(
            "<p>The cat is named Minka.</p><p>Rust is a language.</p>",
            "\"v1\"",
        )
        .await;
        let spec = url_knowledge(&server);
        let db = Arc::new(InMemoryPgVector::default());
        let embedder = Arc::new(WordCounter::default());
        let base = notes("url-once");

        search_tool_on("agent", &spec, &base, embedder.clone(), db.clone())
            .await
            .unwrap();
        let first = embedder.0.load(Ordering::Relaxed);
        search_tool_on("agent", &spec, &base, embedder.clone(), db.clone())
            .await
            .unwrap();
        let unchanged = embedder.0.load(Ordering::Relaxed);

        let changed = serve(
            "<p>The cat is now named Mimi.</p><p>Rust is a language.</p>",
            "\"v2\"",
        )
        .await;
        let mut changed_spec = url_knowledge(&changed);
        changed_spec.urls = vec![format!("{}/pets", changed.uri())];
        search_tool_on("agent", &changed_spec, &base, embedder.clone(), db.clone())
            .await
            .unwrap();

        // first: probe + 2 chunks; 304: nothing; a new URL: its 2 chunks (the old URL is removed).
        assert_eq!(first, 3);
        assert_eq!(unchanged, first);
        assert_eq!(embedder.0.load(Ordering::Relaxed), first + 2);
        let stored = db.chunks_of("agent/search_notes");
        assert!(
            stored
                .iter()
                .all(|(source, _, _)| source.starts_with(&changed.uri())),
            "{stored:?}"
        );
    }

    #[tokio::test]
    async fn a_failing_url_keeps_its_stored_chunks() {
        let server = serve("<p>The cat is named Minka.</p>", "\"v1\"").await;
        let spec = url_knowledge(&server);
        let db = Arc::new(InMemoryPgVector::default());
        let base = notes("url-fail");
        let embedder = Arc::new(WordCounter::default());
        search_tool_on("agent", &spec, &base, embedder.clone(), db.clone())
            .await
            .unwrap();
        drop(server);

        let tool = search_tool_on("agent", &spec, &base, embedder, db.clone())
            .await
            .unwrap();

        let answer = tool.call_json(json!({ "query": "cat" })).await.unwrap();
        assert!(answer.contains("The cat is named Minka."), "{answer}");
    }
}
