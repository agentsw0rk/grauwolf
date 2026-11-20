//! Attachments in user messages (ADR-0081).

use grauwolf_anthropic::AnthropicClient;
use grauwolf_llm::{LlmClient, LlmError};
use grauwolf_prompt::{Attachment, AttachmentData, LlModel, Message, Prompt};
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn attachment(media_type: &str, data: AttachmentData) -> Attachment {
    Attachment {
        media_type: media_type.into(),
        data,
    }
}

fn prompt(attachments: Vec<Attachment>) -> Prompt {
    Prompt {
        id: "p".into(),
        messages: vec![Message::user_with("What is shown here?", attachments)],
        params: Default::default(),
    }
}

#[tokio::test]
async fn sends_images_and_pdfs_as_blocks_before_the_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(body_partial_json(json!({ "messages": [ { "role": "user", "content": [
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo=" } },
            { "type": "document", "source": { "type": "url", "url": "https://example.com/plan.pdf" } },
            { "type": "text", "text": "What is shown here?" }
        ] } ] })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [ { "type": "text", "text": "A plan." } ],
            "stop_reason": "end_turn"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let answer = AnthropicClient::new("sk-ant-test")
        .with_base_url(server.uri())
        .execute(
            &prompt(vec![
                attachment("image/png", AttachmentData::Base64("iVBORw0KGgo=".into())),
                attachment(
                    "application/pdf",
                    AttachmentData::Url("https://example.com/plan.pdf".into()),
                ),
            ]),
            &LlModel::new("anthropic", "claude-haiku-5-5"),
            &[],
        )
        .await
        .unwrap();

    assert_eq!(answer, Message::assistant("A plan."));
}

#[tokio::test]
async fn refuses_audio_before_any_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let error = AnthropicClient::new("sk-ant-test")
        .with_base_url(server.uri())
        .execute(
            &prompt(vec![attachment(
                "audio/wav",
                AttachmentData::Base64("UklGRg==".into()),
            )]),
            &LlModel::new("anthropic", "claude-haiku-5-5"),
            &[],
        )
        .await
        .unwrap_err();

    assert!(matches!(error, LlmError::Client(_)), "{error:?}");
    let text = error.to_string();
    assert!(
        text.contains("anthropic") && text.contains("audio/wav"),
        "{text}"
    );
}
