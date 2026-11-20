//! JSON requests to provider APIs (ADR-0008).

use grauwolf_llm_http::{HttpClient, HttpError};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn posts_json_and_decodes_the_json_answer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(body_json(json!({ "model": "m" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
        .expect(1)
        .mount(&server)
        .await;
    let client = HttpClient::new(server.uri());

    let answer: Value = client
        .post_json("/api/chat", &json!({ "model": "m" }))
        .await
        .unwrap();

    assert_eq!(answer, json!({ "ok": true }));
}

#[tokio::test]
async fn reports_error_status_with_the_response_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(404).set_body_string(r#"{"error":"model not found"}"#))
        .mount(&server)
        .await;
    let client = HttpClient::new(server.uri());

    let error = client
        .post_json::<_, Value>("/api/chat", &json!({}))
        .await
        .unwrap_err();

    assert!(
        matches!(&error, HttpError::Status { status: 404, body, .. } if body.contains("model not found")),
        "{error:?}"
    );
}

#[tokio::test]
async fn sends_bearer_token_and_extra_headers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::header("authorization", "Bearer secret"))
        .and(wiremock::matchers::header(
            "anthropic-version",
            "2023-06-01",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    let client = HttpClient::new(server.uri())
        .with_bearer_token("secret")
        .with_header("anthropic-version", "2023-06-01");

    let answer: Value = client.post_json("/v1/messages", &json!({})).await.unwrap();

    assert_eq!(answer, json!({}));
}

#[tokio::test]
async fn classifies_rate_limits_as_transient_with_the_requested_delay() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/limit"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "3"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/bad"))
        .respond_with(ResponseTemplate::new(400).set_body_string("broken"))
        .mount(&server)
        .await;
    let client = HttpClient::new(server.uri());

    let limited = client
        .post_json::<_, Value>("/limit", &json!({}))
        .await
        .unwrap_err();
    let bad = client
        .post_json::<_, Value>("/bad", &json!({}))
        .await
        .unwrap_err();

    assert!(matches!(
        grauwolf_llm::LlmError::from(limited),
        grauwolf_llm::LlmError::Transient { retry_after: Some(delay), .. }
            if delay == std::time::Duration::from_secs(3)
    ));
    assert!(matches!(
        grauwolf_llm::LlmError::from(bad),
        grauwolf_llm::LlmError::Client(_)
    ));
}

#[tokio::test]
async fn classifies_unreachable_servers_as_transient() {
    // Port 9 (discard) is not in use locally: the connection is refused.
    let client = HttpClient::new("http://127.0.0.1:9");

    let error = client
        .post_json::<_, Value>("/x", &json!({}))
        .await
        .unwrap_err();

    assert!(matches!(
        grauwolf_llm::LlmError::from(error),
        grauwolf_llm::LlmError::Transient {
            retry_after: None,
            ..
        }
    ));
}

/// Timeouts (ADR-0065): a server that sends nothing for too long leads to a temporary error
/// instead of endless waiting.
#[tokio::test]
async fn gives_up_on_a_server_that_sends_nothing_within_the_read_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "ok": true }))
                .set_delay(std::time::Duration::from_secs(5)),
        )
        .mount(&server)
        .await;
    let client =
        HttpClient::new(server.uri()).with_read_timeout(std::time::Duration::from_millis(200));
    let started = std::time::Instant::now();

    let error = client
        .post_json::<_, Value>("/api/chat", &json!({}))
        .await
        .unwrap_err();

    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert!(
        matches!(
            grauwolf_llm::LlmError::from(error),
            grauwolf_llm::LlmError::Transient { .. }
        ),
        "timeout must be temporary"
    );
}

#[tokio::test]
async fn gets_json_and_decodes_the_json_answer() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "models": [] })))
        .expect(1)
        .mount(&server)
        .await;
    let client = HttpClient::new(server.uri());

    let answer: Value = client.get_json("/api/tags").await.unwrap();

    assert_eq!(answer, json!({ "models": [] }));
}

#[tokio::test]
async fn a_signer_adds_headers_computed_from_the_serialized_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/signed"))
        .and(wiremock::matchers::header("x-body-length", "11"))
        .and(wiremock::matchers::header("x-method", "POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
        .expect(1)
        .mount(&server)
        .await;
    let client = HttpClient::new(server.uri()).with_signer(std::sync::Arc::new(|request| {
        assert!(request.url.ends_with("/signed"));
        vec![
            ("x-body-length".into(), request.body.len().to_string()),
            ("x-method".into(), request.method.into()),
        ]
    }));

    let answer: Value = client
        .post_json("/signed", &json!({ "a": "bcd" }))
        .await
        .unwrap();

    assert_eq!(answer, json!({ "ok": true }));
}

#[tokio::test]
async fn post_chunks_returns_the_raw_body_in_pieces() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/binary"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8, 159, 146, 150, 255]))
        .mount(&server)
        .await;
    let client = HttpClient::new(server.uri());

    let chunks: Vec<Vec<u8>> = futures_util::StreamExt::collect::<Vec<_>>(
        client.post_chunks("/binary", &json!({})).await.unwrap(),
    )
    .await
    .into_iter()
    .map(Result::unwrap)
    .collect();

    assert_eq!(chunks.concat(), vec![0u8, 159, 146, 150, 255]);
}

#[tokio::test]
async fn post_chunks_reports_an_error_status_before_the_first_chunk() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(403).set_body_string("denied"))
        .mount(&server)
        .await;
    let client = HttpClient::new(server.uri());

    let error = client
        .post_chunks("/binary", &json!({}))
        .await
        .err()
        .unwrap();

    assert!(matches!(error, HttpError::Status { status: 403, .. }));
}
