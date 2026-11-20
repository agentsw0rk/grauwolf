//! Attachments in user messages (ADR-0081).

use grauwolf_llm::{LlmClient, LlmError};
use grauwolf_ollama::OllamaClient;
use grauwolf_prompt::{Attachment, AttachmentData, LlModel, Message, Prompt};
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn prompt(attachments: Vec<Attachment>) -> Prompt {
    Prompt {
        id: "p".into(),
        messages: vec![Message::user_with("What is in the picture?", attachments)],
        params: Default::default(),
    }
}

fn attachment(media_type: &str, data: AttachmentData) -> Attachment {
    Attachment {
        media_type: media_type.into(),
        data,
    }
}

#[tokio::test]
async fn sends_images_as_base64_with_the_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(body_partial_json(json!({ "messages": [
            { "role": "user", "content": "What is in the picture?", "images": ["iVBORw0KGgo="] }
        ] })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "message": { "role": "assistant", "content": "A wolf." }, "done": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let answer = OllamaClient::new(server.uri())
        .execute(
            &prompt(vec![attachment(
                "image/png",
                AttachmentData::Base64("iVBORw0KGgo=".into()),
            )]),
            &LlModel::new("ollama", "qwen3.5:9b"),
            &[],
        )
        .await
        .unwrap();

    assert_eq!(answer, Message::assistant("A wolf."));
}

#[tokio::test]
async fn refuses_documents_and_image_urls_before_any_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let client = OllamaClient::new(server.uri());
    let model = LlModel::new("ollama", "qwen3.5:9b");

    let pdf = client
        .execute(
            &prompt(vec![attachment(
                "application/pdf",
                AttachmentData::Base64("JVBERi0=".into()),
            )]),
            &model,
            &[],
        )
        .await
        .unwrap_err();
    let url = client
        .execute(
            &prompt(vec![attachment(
                "image/png",
                AttachmentData::Url("https://example.com/wolf.png".into()),
            )]),
            &model,
            &[],
        )
        .await
        .unwrap_err();

    assert!(
        matches!(pdf, LlmError::Client(_)) && pdf.to_string().contains("application/pdf"),
        "{pdf}"
    );
    assert!(
        matches!(url, LlmError::Client(_)) && url.to_string().contains("URL"),
        "{url}"
    );
}
