//! Attachments (ADR-0081) as `image` and `document` blocks of the Converse API.

use grauwolf_bedrock::BedrockClient;
use grauwolf_llm::{LlmClient, LlmError};
use grauwolf_prompt::{Attachment, AttachmentData, LlModel, Message, Prompt};
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn attachment(media_type: &str, data: AttachmentData) -> Attachment {
    Attachment {
        media_type: media_type.into(),
        data,
    }
}

fn base64(media_type: &str) -> Attachment {
    attachment(media_type, AttachmentData::Base64("QUJD".into()))
}

fn prompt_with(attachments: Vec<Attachment>) -> Prompt {
    let mut prompt = Prompt::builder("p").build();
    prompt.messages.push(Message::User {
        content: "What is this?".into(),
        attachments,
    });
    prompt
}

async fn server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "output": { "message": { "role": "assistant", "content": [ { "text": "ok" } ] } },
            "stopReason": "end_turn"
        })))
        .mount(&server)
        .await;
    server
}

fn client(server: &MockServer) -> BedrockClient {
    BedrockClient::with_api_key("key", "us-west-2").with_base_url(server.uri())
}

fn model() -> LlModel {
    LlModel::new("bedrock", "amazon.nova-pro-v1:0")
}

#[tokio::test]
async fn sends_images_and_documents_before_the_text() {
    let server = server().await;
    let prompt = prompt_with(vec![
        base64("image/png"),
        base64("image/jpg"),
        base64("application/pdf"),
        attachment(
            "image/webp",
            AttachmentData::Url("s3://bucket/key.webp".into()),
        ),
        base64("text/markdown"),
    ]);

    client(&server)
        .execute(&prompt, &model(), &[])
        .await
        .unwrap();

    let body: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(
        body["messages"][0]["content"],
        json!([
            { "image": { "format": "png", "source": { "bytes": "QUJD" } } },
            { "image": { "format": "jpeg", "source": { "bytes": "QUJD" } } },
            { "document": { "format": "pdf", "name": "document-3", "source": { "bytes": "QUJD" } } },
            { "image": { "format": "webp", "source": { "s3Location": { "uri": "s3://bucket/key.webp" } } } },
            { "document": { "format": "md", "name": "document-5", "source": { "bytes": "QUJD" } } },
            { "text": "What is this?" }
        ])
    );
}

#[tokio::test]
async fn rejects_attachments_bedrock_cannot_take_before_sending() {
    let server = server().await;
    let cases = [
        base64("audio/wav"),
        base64("video/mp4"),
        attachment(
            "image/png",
            AttachmentData::Url("https://example.com/a.png".into()),
        ),
    ];
    for attachment in cases {
        let error = client(&server)
            .execute(&prompt_with(vec![attachment]), &model(), &[])
            .await
            .unwrap_err();

        assert!(
            matches!(&error, LlmError::Client(source) if source.to_string().starts_with("bedrock: attachments")),
            "{error:?}"
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}
