//! Attachments on user messages for Gemini (ADR-0081, ADR-0083).

use grauwolf_gemini::GeminiClient;
use grauwolf_llm::LlmClient;
use grauwolf_prompt::{Attachment, AttachmentData, LlModel, Message, Prompt};
use serde_json::json;
use wiremock::matchers::{body_json, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn sends_inline_data_and_file_uris_before_the_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_json(json!({
            "contents": [ { "role": "user", "parts": [
                { "inlineData": { "mimeType": "audio/mpeg", "data": "SUQz" } },
                { "fileData": { "mimeType": "application/pdf", "fileUri": "https://example.org/a.pdf" } },
                { "text": "What is this?" }
            ] } ]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "candidates": [ { "content": { "role": "model", "parts": [ { "text": "A song and a letter." } ] }, "finishReason": "STOP" } ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = GeminiClient::new("g-test").with_base_url(server.uri());
    let mut prompt = Prompt::builder("p").build();
    prompt.messages.push(Message::user_with(
        "What is this?",
        vec![
            Attachment {
                media_type: "audio/mpeg".into(),
                data: AttachmentData::Base64("SUQz".into()),
            },
            Attachment {
                media_type: "application/pdf".into(),
                data: AttachmentData::Url("https://example.org/a.pdf".into()),
            },
        ],
    ));

    let answer = client
        .execute(&prompt, &LlModel::new("gemini", "gemini-3-flash"), &[])
        .await
        .unwrap();

    assert_eq!(answer, Message::assistant("A song and a letter."));
}
