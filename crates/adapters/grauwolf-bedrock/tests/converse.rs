//! Bedrock `converse` (F14.18, ADR-0105) against handwritten responses in the documented format.

use grauwolf_bedrock::{BedrockClient, Credentials};
use grauwolf_llm::{LlmClient, LlmError};
use grauwolf_prompt::{
    AssistantMessage, AssistantPart, LlModel, Message, Prompt, Reasoning, ToolCall, ToolChoice,
    ToolDescriptor, ToolResult, Usage,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, header_regex, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MODEL_ID: &str = "eu.anthropic.claude-3-5-sonnet-20240620-v1:0";
const CONVERSE_PATH: &str = "/model/eu.anthropic.claude-3-5-sonnet-20240620-v1%3A0/converse";

fn model() -> LlModel {
    LlModel::new("bedrock", MODEL_ID)
}

fn credentials() -> Credentials {
    Credentials {
        access_key_id: "AKIDEXAMPLE".into(),
        secret_access_key: "secret-key".into(),
        session_token: None,
    }
}

fn client(server: &MockServer) -> BedrockClient {
    BedrockClient::new(credentials(), "eu-central-1").with_base_url(server.uri())
}

/// A `converse` response with the given content blocks.
fn response(content: Value, stop_reason: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "output": { "message": { "role": "assistant", "content": content } },
        "stopReason": stop_reason,
        "usage": { "inputTokens": 10, "outputTokens": 2, "totalTokens": 12 },
        "metrics": { "latencyMs": 300 }
    }))
}

async fn mount(server: &MockServer, request: Value, template: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(CONVERSE_PATH))
        .and(body_json(request))
        .respond_with(template)
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn sends_system_and_messages_and_returns_the_text() {
    let server = MockServer::start().await;
    mount(
        &server,
        json!({
            "system": [ { "text": "Be brief." } ],
            "messages": [
                { "role": "user", "content": [ { "text": "Hello" } ] },
                { "role": "assistant", "content": [ { "text": "Hi" } ] },
                { "role": "user", "content": [ { "text": "Answer in German." }, { "text": "How are you?" } ] }
            ]
        }),
        response(json!([ { "text": "Gut, danke." } ]), "end_turn"),
    )
    .await;
    let mut prompt = Prompt::builder("p")
        .system("Be brief.")
        .user("Hello")
        .build();
    prompt.messages.extend([
        Message::assistant("Hi"),
        Message::System {
            content: "Answer in German.".into(),
        },
        Message::user("How are you?"),
    ]);

    let answer = client(&server)
        .execute(&prompt, &model(), &[])
        .await
        .unwrap();

    assert_eq!(answer, Message::assistant("Gut, danke."));
}

#[tokio::test]
async fn signs_requests_with_sigv4_for_the_bedrock_service_and_region() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header_regex(
            "authorization",
            r"^AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/\d{8}/eu-central-1/bedrock/aws4_request, SignedHeaders=host;x-amz-date, Signature=[0-9a-f]{64}$",
        ))
        .and(header_regex("x-amz-date", r"^\d{8}T\d{6}Z$"))
        .respond_with(response(json!([ { "text": "ok" } ]), "end_turn"))
        .expect(1)
        .mount(&server)
        .await;
    let prompt = Prompt::builder("p").user("Hello").build();

    client(&server)
        .execute(&prompt, &model(), &[])
        .await
        .unwrap();
}

#[tokio::test]
async fn signs_the_session_token_of_temporary_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("x-amz-security-token", "session-token"))
        .and(header_regex(
            "authorization",
            r"SignedHeaders=host;x-amz-date;x-amz-security-token, ",
        ))
        .respond_with(response(json!([ { "text": "ok" } ]), "end_turn"))
        .expect(1)
        .mount(&server)
        .await;
    let credentials = Credentials {
        session_token: Some("session-token".into()),
        ..credentials()
    };
    let prompt = Prompt::builder("p").user("Hello").build();

    BedrockClient::new(credentials, "eu-central-1")
        .with_base_url(server.uri())
        .execute(&prompt, &model(), &[])
        .await
        .unwrap();
}

#[tokio::test]
async fn authenticates_with_a_bedrock_api_key_as_bearer_token_without_signing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("authorization", "Bearer bedrock-api-key"))
        .respond_with(response(json!([ { "text": "ok" } ]), "end_turn"))
        .expect(1)
        .mount(&server)
        .await;
    let prompt = Prompt::builder("p").user("Hello").build();

    BedrockClient::with_api_key("bedrock-api-key", "us-west-2")
        .with_base_url(server.uri())
        .execute(&prompt, &model(), &[])
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert!(requests[0].headers.get("x-amz-date").is_none());
}

#[test]
fn debug_output_shows_no_credentials() {
    let shown = format!(
        "{:?} {:?}",
        BedrockClient::new(credentials(), "us-west-2"),
        BedrockClient::with_api_key("bedrock-api-key", "us-west-2")
    );
    assert!(
        !shown.contains("secret-key")
            && !shown.contains("bedrock-api-key")
            && !shown.contains("AKID"),
        "{shown}"
    );
}

fn add_tool() -> ToolDescriptor {
    ToolDescriptor {
        name: "add".into(),
        description: "Adds two numbers".into(),
        parameters: json!({ "type": "object", "properties": { "a": { "type": "number" }, "b": { "type": "number" } } }),
    }
}

fn call(id: &str, a: i64, b: i64) -> AssistantPart {
    AssistantPart::ToolCall(ToolCall {
        id: id.into(),
        tool: "add".into(),
        args: json!({ "a": a, "b": b }),
    })
}

fn result(id: &str, content: &str) -> Message {
    Message::ToolResult(ToolResult {
        id: id.into(),
        tool: "add".into(),
        content: content.into(),
    })
}

#[tokio::test]
async fn offers_tools_sends_calls_and_results_and_returns_tool_uses() {
    let server = MockServer::start().await;
    mount(
        &server,
        json!({
            "messages": [
                { "role": "user", "content": [ { "text": "Calculate" } ] },
                { "role": "assistant", "content": [
                    { "text": "Let me see." },
                    { "toolUse": { "toolUseId": "t1", "name": "add", "input": { "a": 1, "b": 2 } } },
                    { "toolUse": { "toolUseId": "t2", "name": "add", "input": { "a": 3, "b": 4 } } }
                ] },
                { "role": "user", "content": [
                    { "toolResult": { "toolUseId": "t1", "content": [ { "text": "3" } ] } },
                    { "toolResult": { "toolUseId": "t2", "content": [ { "json": { "sum": 7 } } ] } }
                ] }
            ],
            "toolConfig": { "tools": [ { "toolSpec": {
                "name": "add",
                "description": "Adds two numbers",
                "inputSchema": { "json": add_tool().parameters }
            } } ] }
        }),
        response(
            json!([ { "toolUse": { "toolUseId": "t3", "name": "add", "input": { "a": 5, "b": 6 } } } ]),
            "tool_use",
        ),
    )
    .await;
    let mut prompt = Prompt::builder("p").user("Calculate").build();
    prompt.messages.extend([
        Message::Assistant(AssistantMessage {
            parts: vec![
                AssistantPart::Text {
                    text: "Let me see.".into(),
                },
                call("t1", 1, 2),
                call("t2", 3, 4),
            ],
        }),
        result("t1", "3"),
        result("t2", r#"{"sum":7}"#),
    ]);

    let answer = client(&server)
        .execute(&prompt, &model(), &[add_tool()])
        .await
        .unwrap();

    assert_eq!(
        answer,
        Message::Assistant(AssistantMessage {
            parts: vec![call("t3", 5, 6)]
        })
    );
}

#[tokio::test]
async fn maps_the_tool_choice_to_the_converse_tool_choice() {
    let cases = [
        (ToolChoice::Auto, None),
        (ToolChoice::Required, Some(json!({ "any": {} }))),
        (
            ToolChoice::Tool("add".into()),
            Some(json!({ "tool": { "name": "add" } })),
        ),
    ];
    for (choice, expected) in cases {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(response(json!([ { "text": "ok" } ]), "end_turn"))
            .mount(&server)
            .await;
        let mut prompt = Prompt::builder("p").user("Calculate").build();
        prompt.params.tool_choice = choice.clone();

        client(&server)
            .execute(&prompt, &model(), &[add_tool()])
            .await
            .unwrap();

        let body: Value = server.received_requests().await.unwrap()[0]
            .body_json()
            .unwrap();
        assert_eq!(
            body["toolConfig"].get("toolChoice").cloned(),
            expected,
            "{choice:?}"
        );
    }
}

#[tokio::test]
async fn leaves_the_tools_out_for_tool_choice_none_because_converse_has_no_such_choice() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(response(json!([ { "text": "ok" } ]), "end_turn"))
        .mount(&server)
        .await;
    let mut prompt = Prompt::builder("p").user("Calculate").build();
    prompt.params.tool_choice = ToolChoice::None;

    client(&server)
        .execute(&prompt, &model(), &[add_tool()])
        .await
        .unwrap();

    let body: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert!(body.get("toolConfig").is_none(), "{body}");
}

#[tokio::test]
async fn keeps_the_tools_for_tool_choice_none_when_the_history_has_tool_calls() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(response(json!([ { "text": "ok" } ]), "end_turn"))
        .mount(&server)
        .await;
    let mut prompt = Prompt::builder("p").user("Calculate").build();
    prompt.messages.extend([
        Message::Assistant(AssistantMessage {
            parts: vec![call("t1", 1, 2)],
        }),
        result("t1", "3"),
    ]);
    prompt.params.tool_choice = ToolChoice::None;

    client(&server)
        .execute(&prompt, &model(), &[add_tool()])
        .await
        .unwrap();

    let body: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert!(body["toolConfig"].get("toolChoice").is_none(), "{body}");
    assert_eq!(body["toolConfig"]["tools"][0]["toolSpec"]["name"], "add");
}

#[tokio::test]
async fn sends_temperature_and_the_configured_maximum_of_tokens() {
    let server = MockServer::start().await;
    mount(
        &server,
        json!({
            "messages": [ { "role": "user", "content": [ { "text": "Hello" } ] } ],
            "inferenceConfig": { "maxTokens": 500, "temperature": 0.25 }
        }),
        response(json!([ { "text": "ok" } ]), "end_turn"),
    )
    .await;
    let prompt = Prompt::builder("p").user("Hello").build();
    let mut model = model();
    model.temperature = Some(0.25);

    client(&server)
        .with_max_tokens(500)
        .execute(&prompt, &model, &[])
        .await
        .unwrap();
}

#[tokio::test]
async fn reports_usage_with_cache_tokens_added_to_the_input() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "output": { "message": { "role": "assistant", "content": [ { "text": "ok" } ] } },
            "stopReason": "end_turn",
            "usage": { "inputTokens": 10, "outputTokens": 2, "totalTokens": 52,
                       "cacheReadInputTokens": 30, "cacheWriteInputTokens": 10 }
        })))
        .mount(&server)
        .await;
    let prompt = Prompt::builder("p").user("Hello").build();

    let completion = client(&server)
        .execute_with_usage(&prompt, &model(), &[])
        .await
        .unwrap();

    assert_eq!(
        completion.usage,
        Some(Usage {
            input_tokens: 50,
            output_tokens: 2,
            cache_read_tokens: Some(30),
            cache_write_tokens: Some(10),
        })
    );
}

#[tokio::test]
async fn reports_no_model_because_converse_does_not_say_which_model_answered() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(response(json!([ { "text": "ok" } ]), "end_turn"))
        .mount(&server)
        .await;
    let prompt = Prompt::builder("p").user("Hello").build();

    let completion = client(&server)
        .execute_with_usage(&prompt, &model(), &[])
        .await
        .unwrap();

    assert_eq!(completion.model, None);
}

#[tokio::test]
async fn keeps_reasoning_blocks_and_returns_them_unchanged_in_the_next_request() {
    let server = MockServer::start().await;
    let reasoning = json!({ "reasoningContent": { "reasoningText": { "text": "Think…", "signature": "sig" } } });
    let redacted = json!({ "reasoningContent": { "redactedContent": "b3BhcXVl" } });
    mount(
        &server,
        json!({
            "messages": [
                { "role": "user", "content": [ { "text": "Calculate" } ] },
                { "role": "assistant", "content": [ reasoning.clone(), redacted.clone(), { "text": "Done" } ] },
                { "role": "user", "content": [ { "text": "Again" } ] }
            ]
        }),
        response(json!([ { "text": "ok" } ]), "end_turn"),
    )
    .await;
    let mut prompt = Prompt::builder("p").user("Calculate").build();
    let own = |data: Value| {
        AssistantPart::Reasoning(Reasoning {
            provider: "bedrock".into(),
            data,
        })
    };
    prompt.messages.extend([
        Message::Assistant(AssistantMessage {
            parts: vec![
                own(reasoning),
                AssistantPart::Reasoning(Reasoning {
                    provider: "anthropic".into(),
                    data: json!({ "type": "thinking" }),
                }),
                own(redacted),
                AssistantPart::Text {
                    text: "Done".into(),
                },
            ],
        }),
        Message::user("Again"),
    ]);

    client(&server)
        .execute(&prompt, &model(), &[])
        .await
        .unwrap();
}

#[tokio::test]
async fn returns_reasoning_blocks_of_the_response_in_order() {
    let server = MockServer::start().await;
    let reasoning = json!({ "reasoningContent": { "reasoningText": { "text": "Think…", "signature": "sig" } } });
    Mock::given(method("POST"))
        .respond_with(response(
            json!([ reasoning.clone(), { "text": "Done" } ]),
            "end_turn",
        ))
        .mount(&server)
        .await;
    let prompt = Prompt::builder("p").user("Hello").build();

    let answer = client(&server)
        .execute(&prompt, &model(), &[])
        .await
        .unwrap();

    assert_eq!(
        answer,
        Message::Assistant(AssistantMessage {
            parts: vec![
                AssistantPart::Reasoning(Reasoning {
                    provider: "bedrock".into(),
                    data: reasoning
                }),
                AssistantPart::Text {
                    text: "Done".into()
                }
            ]
        })
    );
}

#[tokio::test]
async fn reports_answers_withheld_by_guardrails_as_client_errors() {
    for reason in ["guardrail_intervened", "content_filtered"] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(response(json!([ { "text": "Sorry" } ]), reason))
            .mount(&server)
            .await;
        let prompt = Prompt::builder("p").user("Hello").build();

        let error = client(&server)
            .execute(&prompt, &model(), &[])
            .await
            .unwrap_err();

        assert!(
            matches!(&error, LlmError::Client(source) if source.to_string().contains(reason)),
            "{error:?}"
        );
    }
}

async fn error_for(status: u16, body: Value) -> LlmError {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(&server)
        .await;
    let prompt = Prompt::builder("p").user("Hello").build();
    client(&server)
        .execute(&prompt, &model(), &[])
        .await
        .unwrap_err()
}

#[tokio::test]
async fn explains_validation_access_and_unknown_model_errors() {
    let cases = [
        (400, "ValidationException", "rejected the request"),
        (
            403,
            "AccessDeniedException",
            "access to the model is granted",
        ),
        (404, "ResourceNotFoundException", "check the model id"),
    ];
    for (status, kind, explanation) in cases {
        let error = error_for(status, json!({ "message": kind })).await;

        assert!(
            matches!(&error, LlmError::Client(source)
                if source.to_string().contains(explanation) && source.to_string().contains(kind)),
            "{status}: {error:?}"
        );
    }
}

#[tokio::test]
async fn classifies_throttling_and_unavailable_service_as_transient() {
    for status in [429, 503, 500] {
        let error = error_for(status, json!({ "message": "slow down" })).await;

        assert!(
            matches!(error, LlmError::Transient { .. }),
            "{status}: {error:?}"
        );
    }
}
