//! OpenAI `/moderations` (F01.15, ADR-0066) against recorded responses.

use grauwolf_llm::{LlmClient, Moderation};
use grauwolf_openai::OpenAiClient;
use grauwolf_prompt::LlModel;
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn moderates_natively_with_a_moderation_model() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/moderations"))
        .and(header("authorization", "Bearer sk-test"))
        .and(body_json(json!({
            "model": "omni-moderation-latest",
            "input": "I want to hurt someone."
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "modr-1",
            "model": "omni-moderation-latest",
            "results": [ {
                "flagged": true,
                "categories": { "violence": true, "hate": false, "self-harm": false },
                "category_scores": { "violence": 0.93, "hate": 0.01, "self-harm": 0.0 }
            } ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = OpenAiClient::new("sk-test").with_base_url(server.uri());

    let moderation = client
        .moderate(
            "I want to hurt someone.",
            &LlModel::new("openai", "omni-moderation-latest"),
        )
        .await
        .unwrap();

    assert_eq!(
        moderation,
        Moderation {
            harmful: true,
            categories: vec!["violence".into()],
            usage: None,
        }
    );
}

/// Compatible gateways (ADR-0066): with a chat model, classification goes through
/// `/chat/completions`, and `/moderations` is not called.
#[tokio::test]
async fn classifies_with_the_chat_model_without_a_moderation_model() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/moderations"))
        .respond_with(ResponseTemplate::new(404))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "model": "qwen3.5:9b",
            "choices": [ { "index": 0, "finish_reason": "stop", "message": {
                "role": "assistant",
                "content": "{ \"harmful\": false, \"categories\": [] }"
            } } ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = OpenAiClient::new("sk-test").with_base_url(server.uri());

    let moderation = client
        .moderate("Wie backe ich Brot?", &LlModel::new("openai", "qwen3.5:9b"))
        .await
        .unwrap();

    assert!(!moderation.harmful);
}
