//! Ollama `/api/embed` as `Embedder` (F08.2) against recorded responses.

use grauwolf_ollama::OllamaEmbedder;
use grauwolf_rag::{Embedder, Embedding};
use serde_json::json;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn embeds_all_texts_in_one_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/embed"))
        .and(body_json(json!({
            "model": "nomic-embed-text",
            "input": ["cat", "dog"]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "nomic-embed-text",
            "embeddings": [[0.1, 0.2], [0.3, 0.4]]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let embedder = OllamaEmbedder::new(server.uri(), "nomic-embed-text");

    let embeddings = embedder
        .embed(&["cat".to_owned(), "dog".to_owned()])
        .await
        .unwrap();

    assert_eq!(
        embeddings,
        vec![Embedding(vec![0.1, 0.2]), Embedding(vec![0.3, 0.4])]
    );
}

#[tokio::test]
async fn reports_a_server_error_as_embedding_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/embed"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({ "error": "model does not support embeddings" })),
        )
        .mount(&server)
        .await;
    let embedder = OllamaEmbedder::new(server.uri(), "qwen3.5:9b");

    let error = embedder.embed(&["x".to_owned()]).await.unwrap_err();

    assert!(
        error.to_string().contains("does not support embeddings"),
        "{error}"
    );
}
