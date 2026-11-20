//! Digest of an installed model via `/api/tags` (ADR-0094).

use grauwolf_ollama::OllamaClient;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn server_with_tags() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "models": [
                { "name": "llama3.2:latest", "model": "llama3.2:latest", "digest": "aaaaaaaaaaaa1111" },
                { "name": "qwen3.5:9b", "model": "qwen3.5:9b", "digest": "bbbbbbbbbbbb2222" }
            ]
        })))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn finds_the_digest_of_a_model_by_its_exact_name() {
    let server = server_with_tags().await;

    let digest = OllamaClient::new(server.uri())
        .model_digest("qwen3.5:9b")
        .await;

    assert_eq!(digest.as_deref(), Some("bbbbbbbbbbbb2222"));
}

#[tokio::test]
async fn a_name_without_tag_means_latest_and_case_does_not_matter() {
    let server = server_with_tags().await;

    let digest = OllamaClient::new(server.uri())
        .model_digest("Llama3.2")
        .await;

    assert_eq!(digest.as_deref(), Some("aaaaaaaaaaaa1111"));
}

#[tokio::test]
async fn an_unknown_model_or_an_unreachable_server_has_no_digest() {
    let server = server_with_tags().await;
    let client = OllamaClient::new(server.uri());
    assert_eq!(client.model_digest("mistral").await, None);

    // A port nobody listens on.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let uri = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    assert_eq!(
        OllamaClient::new(uri).model_digest("qwen3.5:9b").await,
        None
    );
}
