//! OpenAI `/embeddings` as `Embedder` (F08.2, ADR-0038) against recorded responses.

use grauwolf_openai::OpenAiEmbedder;
use grauwolf_rag::{Embedder, Embedding};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn embeds_all_texts_in_one_request_in_input_order() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .and(header("authorization", "Bearer sk-test"))
        .and(body_json(
            json!({ "model": "text-embedding-3-small", "input": ["cat", "dog"] }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "object": "list",
            "data": [
                { "object": "embedding", "index": 1, "embedding": [0.3, 0.4] },
                { "object": "embedding", "index": 0, "embedding": [0.1, 0.2] }
            ],
            "model": "text-embedding-3-small"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let embeddings = OpenAiEmbedder::new("sk-test", "text-embedding-3-small")
        .with_base_url(server.uri())
        .embed(&["cat".to_owned(), "dog".to_owned()])
        .await
        .unwrap();

    assert_eq!(
        embeddings,
        [Embedding(vec![0.1, 0.2]), Embedding(vec![0.3, 0.4])]
    );
}
