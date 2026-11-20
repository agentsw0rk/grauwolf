//! Index a spec's knowledge sources and offer them as a search tool (ADR-0038).

use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_rag::{Embedder, Embedding, RagError};
use grauwolf_spec::{KnowledgeSpec, ModelSpec};
use serde_json::json;

/// Counts the words of a fixed vocabulary – enough to test similarity.
struct WordCounter;

impl Embedder for WordCounter {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Embedding>, RagError>> {
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

fn knowledge(sources: &[&str]) -> KnowledgeSpec {
    KnowledgeSpec {
        sources: sources.iter().map(|source| source.to_string()).collect(),
        urls: Vec::new(),
        min_similarity: None,
        inject: false,
        tool_enabled: true,
        embedding_model: ModelSpec {
            provider: "ollama".into(),
            id: "nomic-embed-text".into(),
            fallbacks: Vec::new(),
            temperature: None,
            think: None,
        },
        tool: "search_notes".into(),
        top_k: 1,
        chunk_size: 200,
        index: None,
        store: None,
    }
}

#[tokio::test]
async fn indexes_files_and_directories_relative_to_the_spec() {
    // In the temporary directory of the container: freshly written files on the mounted
    // workspace are sometimes not visible right away.
    let base = grauwolf_test::TestDir::new("knowledge-tests");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("notes/pets")).unwrap();
    std::fs::write(base.join("README.md"), "Rust is a language.").unwrap();
    std::fs::write(base.join("notes/pets/cat.md"), "The cat is named Minka.").unwrap();
    std::fs::write(base.join("notes/trip.txt"), "The holiday goes to Cornwall.").unwrap();

    let tool = crate::knowledge::search_tool(
        &knowledge(&["README.md", "notes"]),
        &base,
        Arc::new(WordCounter),
    )
    .await
    .unwrap();
    let answer = tool
        .call_json(json!({ "query": "What is the cat called?" }))
        .await
        .unwrap();

    assert_eq!(tool.name(), "search_notes");
    assert_eq!(answer, "[notes/pets/cat.md] The cat is named Minka.");
}

/// Like `WordCounter`, but counts the embedded texts.
#[derive(Default)]
struct Counting(std::sync::atomic::AtomicUsize);

impl Embedder for Counting {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Embedding>, RagError>> {
        self.0
            .fetch_add(texts.len(), std::sync::atomic::Ordering::Relaxed);
        WordCounter.embed(texts)
    }
}

#[tokio::test]
async fn embeds_only_new_or_changed_files_when_an_index_is_kept() {
    use std::sync::atomic::Ordering;

    let base = grauwolf_test::TestDir::new("knowledge-index");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("notes")).unwrap();
    std::fs::write(base.join("notes/cat.md"), "The cat is named Minka.").unwrap();
    std::fs::write(base.join("notes/rust.md"), "Rust is a language.").unwrap();
    std::fs::write(base.join("notes/trip.md"), "The holiday goes to Cornwall.").unwrap();
    let mut spec = knowledge(&["notes"]);
    spec.index = Some("cache/knowledge.json".into());
    let build = || async {
        let embedder = Arc::new(Counting::default());
        let tool = crate::knowledge::search_tool(&spec, &base, embedder.clone())
            .await
            .unwrap();
        (tool, embedder.0.load(Ordering::Relaxed))
    };

    let (_, first) = build().await;
    let (_, unchanged) = build().await;
    std::fs::write(base.join("notes/cat.md"), "The cat is now named Mimi.").unwrap();
    std::fs::remove_file(base.join("notes/trip.md")).unwrap();
    let (tool, changed) = build().await;
    let answer = tool
        .call_json(json!({ "query": "What is the cat called?" }))
        .await
        .unwrap();
    let holiday = tool.call_json(json!({ "query": "holiday" })).await.unwrap();

    assert_eq!(first, 3);
    assert_eq!(unchanged, 0);
    assert_eq!(changed, 1);
    assert_eq!(answer, "[notes/cat.md] The cat is now named Mimi.");
    assert!(!holiday.contains("Cornwall"), "{holiday}");
    assert!(base.join("cache/knowledge.json").exists());
}

#[tokio::test]
async fn drops_hits_below_min_similarity_and_says_nothing_was_found() {
    let base = grauwolf_test::TestDir::new("knowledge-min");
    std::fs::write(base.join("pets.md"), "The cat likes the holiday.").unwrap();
    let mut spec = knowledge(&["pets.md"]);
    spec.min_similarity = Some(0.9);

    let tool = crate::knowledge::search_tool(&spec, &base, Arc::new(WordCounter))
        .await
        .unwrap();
    let weak = tool.call_json(json!({ "query": "cat" })).await.unwrap();
    let strong = tool
        .call_json(json!({ "query": "cat holiday" }))
        .await
        .unwrap();

    assert_eq!(weak, "No matches.");
    assert_eq!(strong, "[pets.md] The cat likes the holiday.");
}

mod urls {
    use super::*;
    use std::sync::atomic::Ordering;
    use wiremock::matchers::{header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const PAGE: &str = "<nav>Menu</nav><h1>Pets</h1><p>The cat is named Minka.</p>\
                        <p>Rust is a language.</p><footer>Imprint</footer>";

    /// The page with an ETag; `304` for a conditional request.
    async fn serve(body: &'static str, etag: &'static str) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/pets"))
            .and(header_exists("if-none-match"))
            .respond_with(ResponseTemplate::new(304))
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/pets"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(body, "text/html; charset=utf-8")
                    .insert_header("etag", etag),
            )
            .with_priority(5)
            .mount(&server)
            .await;
        server
    }

    fn url_spec(server: &MockServer) -> KnowledgeSpec {
        let mut spec = knowledge(&[]);
        spec.urls = vec![format!("{}/pets", server.uri())];
        spec.chunk_size = 30;
        spec.index = Some("cache/knowledge.json".into());
        spec
    }

    fn base(name: &str) -> grauwolf_test::TestDir {
        grauwolf_test::TestDir::new(&format!("url-{name}"))
    }

    async fn build(
        spec: &KnowledgeSpec,
        base: &std::path::Path,
    ) -> Result<(Arc<dyn grauwolf_tools::DynTool>, usize), String> {
        let embedder = Arc::new(Counting::default());
        let tool = crate::knowledge::search_tool(spec, base, embedder.clone()).await?;
        Ok((tool, embedder.0.load(Ordering::Relaxed)))
    }

    #[tokio::test]
    async fn indexes_a_page_names_the_url_and_asks_conditionally_next_time() {
        let server = serve(PAGE, "\"v1\"").await;
        let spec = url_spec(&server);
        let base = base("index");

        let (tool, first) = build(&spec, &base).await.unwrap();
        let (_, second) = build(&spec, &base).await.unwrap();
        let answer = tool.call_json(json!({ "query": "cat" })).await.unwrap();

        assert_eq!(first, 3);
        assert_eq!(second, 0, "304: nothing is embedded again");
        assert_eq!(
            answer,
            format!("[{}/pets] The cat is named Minka.", server.uri())
        );
        assert!(!answer.contains("Menu"));
    }

    #[tokio::test]
    async fn embeds_a_changed_page_again_and_drops_a_removed_url() {
        let server = serve(PAGE, "\"v1\"").await;
        let spec = url_spec(&server);
        let base = base("changed");
        build(&spec, &base).await.unwrap();

        let changed = serve(
            "<p>The cat is now named Mimi.</p><p>Rust is a language.</p>",
            "\"v2\"",
        )
        .await;
        let mut changed_spec = url_spec(&changed);
        changed_spec.urls = vec![format!("{}/pets", changed.uri())];
        let (tool, count) = build(&changed_spec, &base).await.unwrap();

        // The first server's URL is gone from the spec: only the new URL is searched.
        let answer = tool.call_json(json!({ "query": "cat" })).await.unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            answer,
            format!("[{}/pets] The cat is now named Mimi.", changed.uri())
        );
    }

    #[tokio::test]
    async fn keeps_the_indexed_page_when_the_url_fails_later() {
        let server = serve(PAGE, "\"v1\"").await;
        let spec = url_spec(&server);
        let base = base("failing");
        build(&spec, &base).await.unwrap();
        drop(server);

        let (tool, embedded) = build(&spec, &base).await.unwrap();
        let answer = tool.call_json(json!({ "query": "cat" })).await.unwrap();

        assert_eq!(embedded, 0);
        assert!(answer.contains("The cat is named Minka."), "{answer}");
    }

    #[tokio::test]
    async fn fails_naming_the_url_when_nothing_is_indexed_yet() {
        let server = MockServer::start().await;
        let spec = url_spec(&server);
        let base = base("missing");

        let error = build(&spec, &base).await.err().unwrap();

        assert_eq!(
            error,
            format!("knowledge URL {}/pets: HTTP 404 Not Found", server.uri())
        );
    }
}
