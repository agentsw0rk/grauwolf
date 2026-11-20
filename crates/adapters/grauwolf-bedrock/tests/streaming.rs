//! `converse-stream` (ADR-0033, ADR-0105): event stream frames served as bytes.

use futures_util::StreamExt;
use grauwolf_bedrock::BedrockClient;
use grauwolf_bedrock::eventstream::crc32;
use grauwolf_llm::{LlmClient, LlmError, StreamFrame, aggregate};
use grauwolf_prompt::{LlModel, Message, Prompt, Reasoning, ToolCall, Usage};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn string_header(name: &str, value: &str) -> Vec<u8> {
    let mut header = vec![name.len() as u8];
    header.extend(name.as_bytes());
    header.push(7);
    header.extend((value.len() as u16).to_be_bytes());
    header.extend(value.as_bytes());
    header
}

fn frame(message_type: &str, kind_header: &str, kind: &str, payload: &Value) -> Vec<u8> {
    let headers = [
        string_header(kind_header, kind),
        string_header(":content-type", "application/json"),
        string_header(":message-type", message_type),
    ]
    .concat();
    let payload = payload.to_string();
    let total = 12 + headers.len() + payload.len() + 4;
    let mut frame = Vec::new();
    frame.extend((total as u32).to_be_bytes());
    frame.extend((headers.len() as u32).to_be_bytes());
    frame.extend(crc32(&frame[..8]).to_be_bytes());
    frame.extend(&headers);
    frame.extend(payload.as_bytes());
    frame.extend(crc32(&frame).to_be_bytes());
    frame
}

fn event(kind: &str, payload: Value) -> Vec<u8> {
    frame("event", ":event-type", kind, &payload)
}

fn stream(events: Vec<Vec<u8>>) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "application/vnd.amazon.eventstream")
        .set_body_bytes(events.concat())
}

fn model() -> LlModel {
    LlModel::new("bedrock", "eu.anthropic.claude-3-5-sonnet-20240620-v1:0")
}

fn client(server: &MockServer) -> BedrockClient {
    BedrockClient::with_api_key("key", "us-west-2").with_base_url(server.uri())
}

fn prompt() -> Prompt {
    Prompt::builder("p").user("Hello").build()
}

async fn collect(client: &BedrockClient) -> Vec<Result<StreamFrame, LlmError>> {
    client
        .execute_streaming(&prompt(), &model(), &[])
        .collect()
        .await
}

#[tokio::test]
async fn streams_text_tool_calls_reasoning_usage_and_the_finish_reason() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/model/eu.anthropic.claude-3-5-sonnet-20240620-v1%3A0/converse-stream"))
        .respond_with(stream(vec![
            event("messageStart", json!({ "role": "assistant" })),
            event("contentBlockDelta", json!({ "contentBlockIndex": 0, "delta": { "reasoningContent": { "text": "Let me " } } })),
            event("contentBlockDelta", json!({ "contentBlockIndex": 0, "delta": { "reasoningContent": { "text": "think." } } })),
            event("contentBlockDelta", json!({ "contentBlockIndex": 0, "delta": { "reasoningContent": { "signature": "sig" } } })),
            event("contentBlockStop", json!({ "contentBlockIndex": 0 })),
            event("contentBlockDelta", json!({ "contentBlockIndex": 1, "delta": { "text": "Hel" } })),
            event("contentBlockDelta", json!({ "contentBlockIndex": 1, "delta": { "text": "lo" } })),
            event("contentBlockStop", json!({ "contentBlockIndex": 1 })),
            event("contentBlockStart", json!({ "contentBlockIndex": 2, "start": { "toolUse": { "toolUseId": "t1", "name": "add" } } })),
            event("contentBlockDelta", json!({ "contentBlockIndex": 2, "delta": { "toolUse": { "input": "{\"a\": 1," } } })),
            event("contentBlockDelta", json!({ "contentBlockIndex": 2, "delta": { "toolUse": { "input": " \"b\": 2}" } } })),
            event("contentBlockStop", json!({ "contentBlockIndex": 2 })),
            event("messageStop", json!({ "stopReason": "tool_use" })),
            event("metadata", json!({ "usage": { "inputTokens": 10, "outputTokens": 4, "totalTokens": 14, "cacheReadInputTokens": 5 }, "metrics": { "latencyMs": 100 } })),
        ]))
        .expect(1)
        .mount(&server)
        .await;

    let frames: Vec<StreamFrame> = collect(&client(&server))
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();

    assert_eq!(
        frames,
        vec![
            StreamFrame::Reasoning(Reasoning {
                provider: "bedrock".into(),
                data: json!({ "reasoningContent": { "reasoningText": { "text": "Let me think.", "signature": "sig" } } }),
            }),
            StreamFrame::TextDelta { text: "Hel".into() },
            StreamFrame::TextDelta { text: "lo".into() },
            StreamFrame::ToolCall(ToolCall {
                id: "t1".into(),
                tool: "add".into(),
                args: json!({ "a": 1, "b": 2 }),
            }),
            StreamFrame::Usage(Usage {
                input_tokens: 15,
                output_tokens: 4,
                cache_read_tokens: Some(5),
                cache_write_tokens: None,
            }),
            StreamFrame::End {
                finish_reason: Some("tool_use".into())
            },
        ]
    );
    assert!(matches!(aggregate(frames), Message::Assistant(_)));
}

#[tokio::test]
async fn streams_redacted_reasoning_and_tool_calls_without_arguments() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(stream(vec![
            event("contentBlockDelta", json!({ "contentBlockIndex": 0, "delta": { "reasoningContent": { "redactedContent": "b3BhcXVl" } } })),
            event("contentBlockStop", json!({ "contentBlockIndex": 0 })),
            event("contentBlockStart", json!({ "contentBlockIndex": 1, "start": { "toolUse": { "toolUseId": "t1", "name": "now" } } })),
            event("contentBlockStop", json!({ "contentBlockIndex": 1 })),
            event("messageStop", json!({ "stopReason": "tool_use" })),
        ]))
        .mount(&server)
        .await;

    let frames: Vec<StreamFrame> = collect(&client(&server))
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();

    assert_eq!(
        frames[0],
        StreamFrame::Reasoning(Reasoning {
            provider: "bedrock".into(),
            data: json!({ "reasoningContent": { "redactedContent": "b3BhcXVl" } }),
        })
    );
    assert_eq!(
        frames[1],
        StreamFrame::ToolCall(ToolCall {
            id: "t1".into(),
            tool: "now".into(),
            args: json!({}),
        })
    );
}

#[tokio::test]
async fn reports_an_exception_in_the_stream_after_the_frames_before_it() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(stream(vec![
            event(
                "contentBlockDelta",
                json!({ "contentBlockIndex": 0, "delta": { "text": "Hel" } }),
            ),
            frame(
                "exception",
                ":exception-type",
                "throttlingException",
                &json!({ "message": "Too many requests" }),
            ),
            event(
                "contentBlockDelta",
                json!({ "contentBlockIndex": 0, "delta": { "text": "never" } }),
            ),
        ]))
        .mount(&server)
        .await;

    let frames = collect(&client(&server)).await;

    assert_eq!(frames.len(), 2, "{frames:?}");
    assert!(matches!(&frames[0], Ok(StreamFrame::TextDelta { text }) if text == "Hel"));
    assert!(
        matches!(&frames[1], Err(LlmError::Transient { source, .. }) if source.to_string().contains("throttlingException")),
        "{frames:?}"
    );
}

#[tokio::test]
async fn classifies_validation_exceptions_in_the_stream_as_client_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(stream(vec![frame(
            "exception",
            ":exception-type",
            "validationException",
            &json!({ "message": "bad" }),
        )]))
        .mount(&server)
        .await;

    let frames = collect(&client(&server)).await;

    assert!(
        matches!(&frames[..], [Err(LlmError::Client(_))]),
        "{frames:?}"
    );
}

#[tokio::test]
async fn reports_a_damaged_frame_as_an_error_and_stops() {
    let mut damaged = event(
        "contentBlockDelta",
        json!({ "contentBlockIndex": 0, "delta": { "text": "bad" } }),
    );
    let at = damaged.len() - 6;
    damaged[at] ^= 0x01;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(stream(vec![
            event(
                "contentBlockDelta",
                json!({ "contentBlockIndex": 0, "delta": { "text": "ok" } }),
            ),
            damaged,
            event("messageStop", json!({ "stopReason": "end_turn" })),
        ]))
        .mount(&server)
        .await;

    let frames = collect(&client(&server)).await;

    assert_eq!(frames.len(), 2, "{frames:?}");
    assert!(matches!(&frames[0], Ok(StreamFrame::TextDelta { .. })));
    assert!(
        matches!(&frames[1], Err(LlmError::Client(source)) if source.to_string().contains("checksum")),
        "{frames:?}"
    );
}

#[tokio::test]
async fn reports_a_stream_that_ends_before_the_message_stop() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(stream(vec![event(
            "contentBlockDelta",
            json!({ "contentBlockIndex": 0, "delta": { "text": "Hel" } }),
        )]))
        .mount(&server)
        .await;

    let frames = collect(&client(&server)).await;

    assert!(
        matches!(frames.last(), Some(Err(LlmError::Client(_)))),
        "{frames:?}"
    );
}

#[tokio::test]
async fn reports_an_error_status_before_the_first_frame() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({ "message": "denied" })))
        .mount(&server)
        .await;

    let frames = collect(&client(&server)).await;

    assert!(
        matches!(&frames[..], [Err(LlmError::Client(source))] if source.to_string().contains("denied access")),
        "{frames:?}"
    );
}

#[tokio::test]
async fn reports_a_withheld_answer_as_an_error_after_the_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(stream(vec![
            event(
                "contentBlockDelta",
                json!({ "contentBlockIndex": 0, "delta": { "text": "Sorry" } }),
            ),
            event(
                "messageStop",
                json!({ "stopReason": "guardrail_intervened" }),
            ),
        ]))
        .mount(&server)
        .await;

    let frames = collect(&client(&server)).await;

    assert!(
        matches!(frames.last(), Some(Err(LlmError::Client(_)))),
        "{frames:?}"
    );
}
