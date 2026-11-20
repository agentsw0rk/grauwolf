//! Attachments in user messages (ADR-0081).

use grauwolf_llm::{LlmClient, LlmError};
use grauwolf_openai::OpenAiClient;
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

#[tokio::test]
async fn sends_images_and_pdfs_as_content_parts() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({ "messages": [ { "role": "user", "content": [
            { "type": "text", "text": "What is shown here?" },
            { "type": "image_url", "image_url": { "url": "https://example.com/wolf.png" } },
            { "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw0KGgo=" } },
            { "type": "file", "file": { "filename": "anhang.pdf", "file_data": "data:application/pdf;base64,JVBERi0=" } }
        ] } ] })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [ { "message": { "role": "assistant", "content": "A wolf." } } ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let prompt = Prompt {
        id: "p".into(),
        messages: vec![Message::user_with(
            "What is shown here?",
            vec![
                attachment(
                    "image/png",
                    AttachmentData::Url("https://example.com/wolf.png".into()),
                ),
                attachment("image/png", AttachmentData::Base64("iVBORw0KGgo=".into())),
                attachment("application/pdf", AttachmentData::Base64("JVBERi0=".into())),
            ],
        )],
        params: Default::default(),
    };

    let answer = OpenAiClient::new("sk-test")
        .with_base_url(server.uri())
        .execute(&prompt, &LlModel::new("openai", "gpt-4o"), &[])
        .await
        .unwrap();

    assert_eq!(answer, Message::assistant("A wolf."));
}

#[tokio::test]
async fn refuses_attachments_it_cannot_send_before_any_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let prompt = Prompt {
        id: "p".into(),
        messages: vec![Message::user_with(
            "What happens in the video?",
            vec![attachment(
                "video/mp4",
                AttachmentData::Base64("AAAA".into()),
            )],
        )],
        params: Default::default(),
    };

    let error = OpenAiClient::new("sk-test")
        .with_base_url(server.uri())
        .execute(&prompt, &LlModel::new("openai", "gpt-4o"), &[])
        .await
        .unwrap_err();

    assert!(matches!(error, LlmError::Client(_)), "{error:?}");
    let text = error.to_string();
    assert!(
        text.contains("openai") && text.contains("video/mp4"),
        "{text}"
    );
}
