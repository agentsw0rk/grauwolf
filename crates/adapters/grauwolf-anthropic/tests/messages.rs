//! Anthropic Messages API (F02.3) against recorded responses.

use grauwolf_anthropic::AnthropicClient;
use grauwolf_llm::LlmClient;
use grauwolf_prompt::{
    AssistantMessage, AssistantPart, LlModel, Message, Prompt, Reasoning, ToolCall,
};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn model() -> LlModel {
    LlModel::new("anthropic", "claude-opus-5-5")
}

/// Response in the Messages API format with the given content blocks.
fn response(content: serde_json::Value, stop_reason: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-5-5",
        "content": content,
        "stop_reason": stop_reason,
        "usage": { "input_tokens": 10, "output_tokens": 2 }
    }))
}

#[tokio::test]
async fn sends_system_prompt_and_messages_and_returns_the_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-ant-test"))
        .and(header("anthropic-version", "2026-10-21"))
        .and(body_json(json!({
            "model": "claude-opus-5-5",
            "max_tokens": 16000,
            "cache_control": { "type": "ephemeral" },
            "system": [
                { "type": "text", "text": "Be brief.", "cache_control": { "type": "ephemeral" } }
            ],
            "messages": [ { "role": "user", "content": "Hello" } ]
        })))
        .respond_with(response(
            json!([
                { "type": "thinking", "thinking": "", "signature": "sig" },
                { "type": "text", "text": "Hi" }
            ]),
            "end_turn",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let client = AnthropicClient::new("sk-ant-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p")
        .system("Be brief.")
        .user("Hello")
        .build();

    let answer = client.execute(&prompt, &model(), &[]).await.unwrap();

    let Message::Assistant(answer) = answer else {
        panic!("Expected a response: {answer:?}");
    };
    assert_eq!(answer.text(), "Hi");
}

fn add_tool() -> grauwolf_prompt::ToolDescriptor {
    grauwolf_prompt::ToolDescriptor {
        name: "add".into(),
        description: "Adds two numbers".into(),
        parameters: json!({ "type": "object", "properties": { "a": { "type": "number" }, "b": { "type": "number" } } }),
    }
}

#[tokio::test]
async fn offers_tools_and_returns_the_tool_use() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::body_partial_json(json!({
            "tools": [{
                "name": "add",
                "description": "Adds two numbers",
                "input_schema": { "type": "object", "properties": { "a": { "type": "number" }, "b": { "type": "number" } } }
            }]
        })))
        .respond_with(response(
            json!([
                { "type": "tool_use", "id": "toolu_1", "name": "add", "input": { "a": 2, "b": 3 } }
            ]),
            "tool_use",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let client = AnthropicClient::new("sk-ant-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("What is 2+3?").build();

    let answer = client
        .execute(&prompt, &model(), &[add_tool()])
        .await
        .unwrap();

    assert_eq!(
        answer,
        Message::tool_call("toolu_1", "add", json!({ "a": 2, "b": 3 }))
    );
}

#[tokio::test]
async fn sends_tool_use_and_tool_result_blocks_from_the_history() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::body_partial_json(json!({
            "messages": [
                { "role": "user", "content": "What is 2+3?" },
                { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "toolu_1", "name": "add", "input": { "a": 2, "b": 3 } }
                ] },
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_1", "content": "5" }
                ] }
            ]
        })))
        .respond_with(response(json!([{ "type": "text", "text": "The result is 5." }]), "end_turn"))
        .expect(1)
        .mount(&server)
        .await;
    let client = AnthropicClient::new("sk-ant-test").with_base_url(server.uri());
    let mut prompt = Prompt::builder("p").user("What is 2+3?").build();
    prompt.messages.push(Message::tool_call(
        "toolu_1",
        "add",
        json!({ "a": 2, "b": 3 }),
    ));
    prompt
        .messages
        .push(Message::tool_result("toolu_1", "add", "5"));

    let answer = client
        .execute(&prompt, &model(), &[add_tool()])
        .await
        .unwrap();

    assert_eq!(answer, Message::assistant("The result is 5."));
}

#[tokio::test]
async fn requests_structured_answers_natively_with_closed_object_schemas() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_json(json!({
            "model": "claude-opus-5-5",
            "max_tokens": 16000,
            "cache_control": { "type": "ephemeral" },
            "messages": [ { "role": "user", "content": "Login bug" } ],
            "output_config": { "format": { "type": "json_schema", "schema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "tags": { "type": "array", "items": {
                        "type": "object",
                        "properties": { "name": { "type": "string" } },
                        "additionalProperties": false
                    } }
                },
                "required": ["title"],
                "additionalProperties": false
            } } }
        })))
        .respond_with(response(
            json!([{ "type": "text", "text": "{\"title\":\"Login kaputt\"}" }]),
            "end_turn",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let client = AnthropicClient::new("sk-ant-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Login bug").build();
    let output = grauwolf_llm::StructuredOutput {
        name: "Ticket".into(),
        schema: json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "tags": { "type": "array", "items": { "type": "object", "properties": { "name": { "type": "string" } } } }
            },
            "required": ["title"]
        }),
    };

    let answer = client
        .execute_structured(&prompt, &model(), &output)
        .await
        .unwrap();

    assert_eq!(answer, Message::assistant(r#"{"title":"Login kaputt"}"#));
}

#[tokio::test]
async fn reports_refusals_as_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
            "content": [],
            "stop_reason": "refusal",
            "stop_details": { "type": "refusal", "category": "cyber", "explanation": "not allowed" }
        })))
        .mount(&server)
        .await;
    let client = AnthropicClient::new("sk-ant-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("…").build();

    let error = client.execute(&prompt, &model(), &[]).await.unwrap_err();

    let message = error.to_string();
    assert!(
        message.contains("refused") && message.contains("cyber"),
        "{message}"
    );
}

#[tokio::test]
async fn keeps_thinking_blocks_and_text_of_an_answer_in_order() {
    let thinking = json!({ "type": "thinking", "thinking": "", "signature": "sig-1" });
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(response(
            json!([
                thinking,
                { "type": "text", "text": "I am working that out." },
                { "type": "tool_use", "id": "toolu_1", "name": "add", "input": { "a": 2, "b": 3 } }
            ]),
            "tool_use",
        ))
        .mount(&server)
        .await;
    let client = AnthropicClient::new("sk-ant-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("What is 2+3?").build();

    let answer = client
        .execute(&prompt, &model(), &[add_tool()])
        .await
        .unwrap();

    assert_eq!(
        answer,
        Message::Assistant(AssistantMessage {
            parts: vec![
                AssistantPart::Reasoning(Reasoning {
                    provider: "anthropic".into(),
                    data: thinking.clone(),
                }),
                AssistantPart::Text {
                    text: "I am working that out.".into()
                },
                AssistantPart::ToolCall(ToolCall {
                    id: "toolu_1".into(),
                    tool: "add".into(),
                    args: json!({ "a": 2, "b": 3 }),
                }),
            ]
        })
    );
}

#[tokio::test]
async fn sends_own_thinking_blocks_back_unchanged_and_drops_foreign_reasoning() {
    let thinking = json!({ "type": "thinking", "thinking": "", "signature": "sig-1" });
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::body_partial_json(json!({
            "messages": [
                { "role": "user", "content": "What is 2+3?" },
                { "role": "assistant", "content": [
                    thinking,
                    { "type": "tool_use", "id": "toolu_1", "name": "add", "input": { "a": 2, "b": 3 } }
                ] },
                { "role": "user", "content": [ { "type": "tool_result", "tool_use_id": "toolu_1", "content": "5" } ] }
            ]
        })))
        .respond_with(response(json!([{ "type": "text", "text": "5" }]), "end_turn"))
        .expect(1)
        .mount(&server)
        .await;
    let client = AnthropicClient::new("sk-ant-test").with_base_url(server.uri());
    let mut prompt = Prompt::builder("p").user("What is 2+3?").build();
    prompt.messages.push(Message::Assistant(AssistantMessage {
        parts: vec![
            AssistantPart::Reasoning(Reasoning {
                provider: "openai".into(),
                data: json!({ "encrypted": "fremd" }),
            }),
            AssistantPart::Reasoning(Reasoning {
                provider: "anthropic".into(),
                data: thinking.clone(),
            }),
            AssistantPart::ToolCall(ToolCall {
                id: "toolu_1".into(),
                tool: "add".into(),
                args: json!({ "a": 2, "b": 3 }),
            }),
        ],
    }));
    prompt
        .messages
        .push(Message::tool_result("toolu_1", "add", "5"));

    let answer = client
        .execute(&prompt, &model(), &[add_tool()])
        .await
        .unwrap();

    assert_eq!(answer, Message::assistant("5"));
}

#[tokio::test]
async fn groups_results_of_parallel_tool_calls_into_one_user_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_json(json!({
            "model": "claude-opus-5-5",
            "max_tokens": 16000,
            "cache_control": { "type": "ephemeral" },
            "messages": [
                { "role": "user", "content": "What is 2+3 and 4+5?" },
                { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "toolu_1", "name": "add", "input": { "a": 2, "b": 3 } },
                    { "type": "tool_use", "id": "toolu_2", "name": "add", "input": { "a": 4, "b": 5 } }
                ] },
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_1", "content": "5" },
                    { "type": "tool_result", "tool_use_id": "toolu_2", "content": "9" }
                ] }
            ]
        })))
        .respond_with(response(json!([{ "type": "text", "text": "5 and 9" }]), "end_turn"))
        .expect(1)
        .mount(&server)
        .await;
    let client = AnthropicClient::new("sk-ant-test").with_base_url(server.uri());
    let mut prompt = Prompt::builder("p").user("What is 2+3 and 4+5?").build();
    let call = |id: &str, a: i64, b: i64| {
        AssistantPart::ToolCall(ToolCall {
            id: id.into(),
            tool: "add".into(),
            args: json!({ "a": a, "b": b }),
        })
    };
    prompt.messages.push(Message::Assistant(AssistantMessage {
        parts: vec![call("toolu_1", 2, 3), call("toolu_2", 4, 5)],
    }));
    prompt
        .messages
        .push(Message::tool_result("toolu_1", "add", "5"));
    prompt
        .messages
        .push(Message::tool_result("toolu_2", "add", "9"));

    let answer = client.execute(&prompt, &model(), &[]).await.unwrap();

    assert_eq!(answer, Message::assistant("5 and 9"));
}

fn sse(events: &[(&str, serde_json::Value)]) -> String {
    events
        .iter()
        .map(|(name, data)| format!("event: {name}\ndata: {data}\n\n"))
        .collect()
}

async fn stream_frames(
    body: String,
) -> Vec<Result<grauwolf_llm::StreamFrame, grauwolf_llm::LlmError>> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::body_partial_json(
            json!({ "stream": true }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .expect(1)
        .mount(&server)
        .await;
    let client = AnthropicClient::new("sk-ant-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("What is 2+3?").build();
    futures_util::StreamExt::collect(client.execute_streaming(&prompt, &model(), &[add_tool()]))
        .await
}

#[tokio::test]
async fn streams_text_and_assembles_thinking_and_tool_use_blocks() {
    let body = sse(&[
        (
            "message_start",
            json!({ "type": "message_start", "message": { "id": "msg_1", "content": [] } }),
        ),
        (
            "content_block_start",
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "" } }),
        ),
        (
            "content_block_delta",
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "Think" } }),
        ),
        (
            "content_block_delta",
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "ing" } }),
        ),
        (
            "content_block_delta",
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "signature_delta", "signature": "sig-1" } }),
        ),
        (
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": 0 }),
        ),
        (
            "content_block_start",
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "text", "text": "" } }),
        ),
        (
            "content_block_delta",
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "text_delta", "text": "I " } }),
        ),
        ("ping", json!({ "type": "ping" })),
        (
            "content_block_delta",
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "text_delta", "text": "am calculating." } }),
        ),
        (
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": 1 }),
        ),
        (
            "content_block_start",
            json!({ "type": "content_block_start", "index": 2, "content_block": { "type": "tool_use", "id": "toolu_1", "name": "add", "input": {} } }),
        ),
        (
            "content_block_delta",
            json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "input_json_delta", "partial_json": "{\"a\": 2, " } }),
        ),
        (
            "content_block_delta",
            json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "input_json_delta", "partial_json": "\"b\": 3}" } }),
        ),
        (
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": 2 }),
        ),
        (
            "message_delta",
            json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" } }),
        ),
        ("message_stop", json!({ "type": "message_stop" })),
    ]);

    let frames: Vec<grauwolf_llm::StreamFrame> = stream_frames(body)
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();

    assert_eq!(
        frames,
        [
            grauwolf_llm::StreamFrame::Reasoning(Reasoning {
                provider: "anthropic".into(),
                data: json!({ "type": "thinking", "thinking": "Thinking", "signature": "sig-1" }),
            }),
            grauwolf_llm::StreamFrame::TextDelta { text: "I ".into() },
            grauwolf_llm::StreamFrame::TextDelta {
                text: "am calculating.".into()
            },
            grauwolf_llm::StreamFrame::ToolCall(ToolCall {
                id: "toolu_1".into(),
                tool: "add".into(),
                args: json!({ "a": 2, "b": 3 }),
            }),
            grauwolf_llm::StreamFrame::End {
                finish_reason: Some("tool_use".into())
            },
        ]
    );
}

#[tokio::test]
async fn reports_overload_events_in_the_stream_as_transient_errors() {
    let body = sse(&[
        (
            "message_start",
            json!({ "type": "message_start", "message": { "id": "msg_1", "content": [] } }),
        ),
        (
            "error",
            json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } }),
        ),
    ]);

    let frames = stream_frames(body).await;

    assert!(
        matches!(
            frames.as_slice(),
            [Err(grauwolf_llm::LlmError::Transient { .. })]
        ),
        "{frames:?}"
    );
}

#[tokio::test]
async fn sends_the_tool_choice_of_the_prompt() {
    use grauwolf_prompt::ToolChoice;

    let server = MockServer::start().await;
    for (choice, expected) in [
        (ToolChoice::Required, json!({ "type": "any" })),
        (ToolChoice::None, json!({ "type": "none" })),
        (
            ToolChoice::Tool("add".into()),
            json!({ "type": "tool", "name": "add" }),
        ),
    ] {
        server.reset().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(wiremock::matchers::body_partial_json(
                json!({ "tool_choice": expected }),
            ))
            .respond_with(response(
                json!([{ "type": "text", "text": "ok" }]),
                "end_turn",
            ))
            .expect(1)
            .mount(&server)
            .await;
        let mut prompt = Prompt::builder("p").user("Rechne").build();
        prompt.params.tool_choice = choice;

        AnthropicClient::new("sk-ant-test")
            .with_base_url(server.uri())
            .execute(&prompt, &model(), &[add_tool()])
            .await
            .unwrap();
        server.verify().await;
    }
}

/// Prompt cache (ADR-0062): every request caches its prefix automatically.
#[tokio::test]
async fn caches_the_prompt_automatically() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(wiremock::matchers::body_partial_json(
            json!({ "cache_control": { "type": "ephemeral" } }),
        ))
        .respond_with(response(
            json!([{ "type": "text", "text": "Hi" }]),
            "end_turn",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let client = AnthropicClient::new("sk-ant-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Hello").build();

    client.execute(&prompt, &model(), &[]).await.unwrap();
}

/// Prompt cache (ADR-0062): tools and the system prompt get their own breakpoint, so that
/// different conversations with the same start share it.
#[tokio::test]
async fn marks_the_end_of_the_system_prompt_as_a_cache_breakpoint() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(wiremock::matchers::body_partial_json(json!({
            "system": [
                { "type": "text", "text": "Be brief.\n\nAnswer in English.",
                  "cache_control": { "type": "ephemeral" } }
            ]
        })))
        .respond_with(response(
            json!([{ "type": "text", "text": "Hi" }]),
            "end_turn",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let client = AnthropicClient::new("sk-ant-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p")
        .system("Be brief.")
        .system("Answer in English.")
        .user("Hello")
        .build();

    client.execute(&prompt, &model(), &[]).await.unwrap();
}

/// Forced tool choice (ADR-0063): these models reject `tool_choice` `any`/`tool`.
#[test]
fn knows_which_models_cannot_force_tool_calls() {
    let client = AnthropicClient::new("sk-ant-test");
    let forces = |id: &str| client.forces_tool_choice(&LlModel::new("anthropic", id));

    for id in [
        "claude-fable-5-1",
        "claude-mythos-5-1",
        "claude-opus-5-5",
        "claude-sonnet-5-5",
        "anthropic.claude-opus-5-5",
    ] {
        assert!(!forces(id), "{id}");
    }
    for id in [
        "claude-haiku-5-5",
        "claude-opus-5",
        "claude-fable-5",
        "claude-sonnet-4-6",
    ] {
        assert!(forces(id), "{id}");
    }
}

/// The model the provider reports (ADR-0094).
#[tokio::test]
async fn reports_the_model_that_answered() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_1", "type": "message", "role": "assistant",
            "model": "claude-opus-5-5-20261001",
            "content": [ { "type": "text", "text": "Hi" } ],
            "stop_reason": "end_turn"
        })))
        .mount(&server)
        .await;
    let client = AnthropicClient::new("sk-ant-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Hello").build();

    let completion = client
        .execute_with_usage(&prompt, &model(), &[])
        .await
        .unwrap();

    assert_eq!(
        completion.model.as_deref(),
        Some("claude-opus-5-5-20261001")
    );
}

/// Token usage (ADR-0067): `input_tokens` includes cached tokens.
#[tokio::test]
async fn reports_token_usage_including_cached_tokens() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-haiku-5-5",
            "content": [ { "type": "text", "text": "Hi" } ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 4,
                "cache_creation_input_tokens": 20,
                "cache_read_input_tokens": 5906,
                "output_tokens": 12
            }
        })))
        .mount(&server)
        .await;
    let client = AnthropicClient::new("sk-ant-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Hello").build();

    let completion = client
        .execute_with_usage(&prompt, &model(), &[])
        .await
        .unwrap();

    assert_eq!(
        completion.usage,
        Some(grauwolf_prompt::Usage {
            input_tokens: 5930,
            output_tokens: 12,
            cache_read_tokens: Some(5906),
            cache_write_tokens: Some(20),
        })
    );
}

/// Token usage of structured responses (ADR-0067).
#[tokio::test]
async fn reports_token_usage_of_structured_answers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-haiku-5-5",
            "content": [ { "type": "text", "text": "{\"ok\": true}" } ],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 30, "output_tokens": 5 }
        })))
        .mount(&server)
        .await;
    let client = AnthropicClient::new("sk-ant-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Ok?").build();
    let output = grauwolf_llm::StructuredOutput {
        name: "Ok".into(),
        schema: json!({ "type": "object", "properties": { "ok": { "type": "boolean" } } }),
    };

    let completion = client
        .execute_structured_with_usage(&prompt, &model(), &output)
        .await
        .unwrap();

    assert_eq!(completion.usage.map(|usage| usage.input_tokens), Some(30));
}

/// The model of `message_start` arrives as a `Model` frame (ADR-0094).
#[tokio::test]
async fn streams_the_model_the_provider_reports() {
    let frames = stream_frames(sse(&[
        (
            "message_start",
            json!({ "type": "message_start", "message": {
            "id": "msg_1", "model": "claude-haiku-5-5-20261001", "content": [] } }),
        ),
        ("message_stop", json!({ "type": "message_stop" })),
    ]))
    .await;

    assert_eq!(
        frames[0].as_ref().unwrap(),
        &grauwolf_llm::StreamFrame::Model("claude-haiku-5-5-20261001".into())
    );
}

/// Token usage in the stream (ADR-0067): input from `message_start`, output from
/// `message_delta`, as a `Usage` frame before `End`.
#[tokio::test]
async fn streams_the_token_usage_before_the_end() {
    let frames = stream_frames(sse(&[
        (
            "message_start",
            json!({ "type": "message_start", "message": {
            "id": "msg_1", "type": "message", "role": "assistant", "content": [],
            "usage": { "input_tokens": 4, "cache_creation_input_tokens": 20,
                       "cache_read_input_tokens": 5906, "output_tokens": 1 } } }),
        ),
        (
            "content_block_start",
            json!({ "type": "content_block_start", "index": 0,
            "content_block": { "type": "text", "text": "" } }),
        ),
        (
            "content_block_delta",
            json!({ "type": "content_block_delta", "index": 0,
            "delta": { "type": "text_delta", "text": "Hi" } }),
        ),
        (
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": 0 }),
        ),
        (
            "message_delta",
            json!({ "type": "message_delta",
            "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 12 } }),
        ),
        ("message_stop", json!({ "type": "message_stop" })),
    ]))
    .await;

    let frames: Vec<_> = frames.into_iter().map(Result::unwrap).collect();
    assert_eq!(
        frames[frames.len() - 2..],
        [
            grauwolf_llm::StreamFrame::Usage(grauwolf_prompt::Usage {
                input_tokens: 5930,
                output_tokens: 12,
                cache_read_tokens: Some(5906),
                cache_write_tokens: Some(20),
            }),
            grauwolf_llm::StreamFrame::End {
                finish_reason: Some("end_turn".into())
            }
        ]
    );
}

#[tokio::test]
async fn sends_the_temperature_of_the_model() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(body_json(json!({
            "model": "claude-opus-5-5",
            "max_tokens": 16000,
            "temperature": 0.3,
            "cache_control": { "type": "ephemeral" },
            "messages": [ { "role": "user", "content": "Hello" } ]
        })))
        .respond_with(response(
            json!([ { "type": "text", "text": "Hi" } ]),
            "end_turn",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let client = AnthropicClient::new("sk-ant-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Hello").build();

    client
        .execute(&prompt, &model().with_temperature(0.3), &[])
        .await
        .unwrap();
}
