//! Attachments in requests to `/run` and `/run/stream` (ADR-0081).

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_agents::memory::MemoryChatHistoryStore;
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{AttachmentData, LlModel, Message, Prompt, ToolDescriptor};
use grauwolf_spec::AgentSpec;
use serde_json::{Value, json};

/// Replies with the attachments of the last user message, e.g. `image/png:base64`.
struct DescribeAttachments;

impl LlmClient for DescribeAttachments {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        let described = prompt
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::User { attachments, .. } => Some(
                    attachments
                        .iter()
                        .map(|attachment| match &attachment.data {
                            AttachmentData::Base64(_) => {
                                format!("{}:base64", attachment.media_type)
                            }
                            AttachmentData::Url(url) => format!("{}:{url}", attachment.media_type),
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                _ => None,
            })
            .unwrap_or_default();
        Box::pin(async move { Ok(Message::assistant(described)) })
    }
}

async fn serve() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/specs/echo.json");
    let spec = AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
    let executor = PromptExecutor::builder()
        .client("ollama", Arc::new(DescribeAttachments))
        .build();
    let agent = Interpreter::new(Arc::new(executor)).load(&spec).unwrap();
    let router =
        grauwolf_server::router(Arc::new(agent), Arc::new(MemoryChatHistoryStore::default()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}")
}

#[tokio::test]
async fn sends_the_attachments_of_a_request_with_its_input() {
    let base = serve().await;
    let client = reqwest::Client::new();
    let body = json!({
        "input": "What is this?",
        "attachments": [
            { "media_type": "image/png", "data": "iVBORw==" },
            { "media_type": "application/pdf", "url": "https://example.org/a.pdf" }
        ]
    });

    let run: Value = client
        .post(format!("{base}/run"))
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let stream = client
        .post(format!("{base}/run/stream"))
        .json(&body)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    let expected = "image/png:base64 application/pdf:https://example.org/a.pdf";
    assert_eq!(run, json!({ "output": expected }));
    assert!(
        stream
            .trim_end()
            .ends_with(&format!("event: output\ndata: \"{expected}\"")),
        "{stream}"
    );
}

#[tokio::test]
async fn refuses_attachments_without_exactly_one_of_data_and_url() {
    let base = serve().await;
    let client = reqwest::Client::new();

    for attachment in [
        json!({ "media_type": "image/png" }),
        json!({ "media_type": "image/png", "data": "iVBORw==", "url": "https://example.org/a.png" }),
    ] {
        for endpoint in ["run", "run/stream"] {
            let response = client
                .post(format!("{base}/{endpoint}"))
                .json(&json!({ "input": "What is this?", "attachments": [attachment] }))
                .send()
                .await
                .unwrap();
            let status = response.status();
            let error: Value = response.json().await.unwrap();
            assert_eq!(status, 400, "{endpoint}: {attachment}");
            assert_eq!(
                error,
                json!({ "error": "attachments/0: give exactly one of data and url" }),
                "{endpoint}"
            );
        }
    }
}
