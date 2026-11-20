//! Embeddings via `batchEmbedContents` (ADR-0038, ADR-0083).

use grauwolf_gemini::GeminiEmbedder;
use grauwolf_rag::{Embedder, Embedding};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn embeds_all_texts_in_one_batch_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-embedding-001:batchEmbedContents"))
        .and(header("x-goog-api-key", "g-test"))
        .and(body_json(json!({
            "requests": [
                { "model": "models/gemini-embedding-001", "content": { "parts": [ { "text": "Wolf" } ] } },
                { "model": "models/gemini-embedding-001", "content": { "parts": [ { "text": "Fox" } ] } }
            ]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "embeddings": [ { "values": [0.5, 0.25] }, { "values": [0.0, 1.0] } ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let embedder =
        GeminiEmbedder::new("g-test", "models/gemini-embedding-001").with_base_url(server.uri());

    let embeddings = embedder
        .embed(&["Wolf".to_owned(), "Fox".to_owned()])
        .await
        .unwrap();

    assert_eq!(
        embeddings,
        [Embedding(vec![0.5, 0.25]), Embedding(vec![0.0, 1.0])]
    );
}
