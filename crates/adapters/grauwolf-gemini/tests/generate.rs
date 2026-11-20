//! Gemini `generateContent` (F02.5, ADR-0083) against mocked responses.

use futures_util::StreamExt;
use grauwolf_gemini::GeminiClient;
use grauwolf_llm::{LlmClient, LlmError, StreamFrame, StructuredOutput};
use grauwolf_prompt::{
    AssistantMessage, AssistantPart, LlModel, Message, Prompt, Reasoning, ToolCall, ToolChoice,
    ToolResult, Usage,
};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn model() -> LlModel {
    LlModel::new("gemini", "models/gemini-3-flash")
}

/// Response in the `generateContent` format with the given parts.
fn response(parts: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "candidates": [ {
            "content": { "role": "model", "parts": parts },
            "finishReason": "STOP",
            "index": 0
        } ],
        "usageMetadata": { "promptTokenCount": 10, "candidatesTokenCount": 2, "totalTokenCount": 12 }
    }))
}

#[tokio::test]
async fn sends_system_instruction_and_contents_and_returns_the_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-3-flash:generateContent"))
        .and(header("x-goog-api-key", "g-test"))
        .and(body_json(json!({
            "systemInstruction": { "parts": [ { "text": "Be brief." } ] },
            "contents": [
                { "role": "user", "parts": [ { "text": "Hello" } ] },
                { "role": "model", "parts": [ { "text": "Hi" } ] },
                { "role": "user", "parts": [ { "text": "Answer in German." } ] },
                { "role": "user", "parts": [ { "text": "How are you?" } ] }
            ]
        })))
        .respond_with(response(
            json!([ { "text": "Fine" }, { "text": ", thanks." } ]),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let client = GeminiClient::new("g-test").with_base_url(server.uri());
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

    let answer = client.execute(&prompt, &model(), &[]).await.unwrap();

    assert_eq!(answer, Message::assistant("Fine, thanks."));
}

fn add_tool() -> grauwolf_prompt::ToolDescriptor {
    grauwolf_prompt::ToolDescriptor {
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

#[tokio::test]
async fn offers_tools_sends_calls_and_results_and_returns_function_calls() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-3-flash:generateContent"))
        .and(body_json(json!({
            "contents": [
                { "role": "user", "parts": [ { "text": "Calculate" } ] },
                { "role": "model", "parts": [
                    { "text": "Calculating." },
                    { "functionCall": { "id": "c1", "name": "add", "args": { "a": 1, "b": 2 } } },
                    // Self-assigned IDs are not sent back to Gemini.
                    { "functionCall": { "name": "add", "args": { "a": 3, "b": 4 } } }
                ] },
                { "role": "user", "parts": [
                    { "functionResponse": { "id": "c1", "name": "add", "response": { "result": "3" } } },
                    { "functionResponse": { "name": "add", "response": { "result": "7" } } }
                ] }
            ],
            "tools": [ { "functionDeclarations": [ {
                "name": "add",
                "description": "Adds two numbers",
                "parametersJsonSchema": { "type": "object", "properties": { "a": { "type": "number" }, "b": { "type": "number" } } }
            } ] } ]
        })))
        .respond_with(response(json!([
            { "functionCall": { "id": "c9", "name": "add", "args": { "a": 3, "b": 7 } } },
            { "functionCall": { "name": "add", "args": { "a": 10, "b": 0 } } }
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let client = GeminiClient::new("g-test").with_base_url(server.uri());
    let mut prompt = Prompt::builder("p").user("Calculate").build();
    prompt.messages.extend([
        Message::Assistant(AssistantMessage {
            parts: vec![
                AssistantPart::Text {
                    text: "Calculating.".into(),
                },
                call("c1", 1, 2),
                call("gemini-call-1", 3, 4),
            ],
        }),
        Message::ToolResult(ToolResult {
            id: "c1".into(),
            tool: "add".into(),
            content: "3".into(),
        }),
        Message::ToolResult(ToolResult {
            id: "gemini-call-1".into(),
            tool: "add".into(),
            content: "7".into(),
        }),
    ]);

    let answer = client
        .execute(&prompt, &model(), &[add_tool()])
        .await
        .unwrap();

    assert_eq!(
        answer,
        Message::Assistant(AssistantMessage {
            parts: vec![call("c9", 3, 7), call("gemini-call-1", 10, 0)],
        })
    );
}

fn gemini_reasoning(data: serde_json::Value) -> AssistantPart {
    AssistantPart::Reasoning(Reasoning {
        provider: "gemini".into(),
        data,
    })
}

#[tokio::test]
async fn keeps_thoughts_and_thought_signatures_in_place_for_the_next_request() {
    let server = MockServer::start().await;
    let thinking = json!([
        { "text": "Thought", "thought": true },
        { "text": "One moment.", "thoughtSignature": "sig-a" },
        { "functionCall": { "name": "add", "args": { "a": 1, "b": 2 } }, "thoughtSignature": "sig-b" }
    ]);
    Mock::given(method("POST"))
        .and(body_json(json!({
            "contents": [ { "role": "user", "parts": [ { "text": "Calculate" } ] } ]
        })))
        .respond_with(response(thinking.clone()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_json(json!({
            "contents": [
                { "role": "user", "parts": [ { "text": "Calculate" } ] },
                { "role": "model", "parts": thinking },
                { "role": "user", "parts": [
                    { "functionResponse": { "name": "add", "response": { "result": "3" } } }
                ] },
                // Other providers' reasoning is dropped; the signature without a following part stays.
                { "role": "model", "parts": [ { "text": "", "thoughtSignature": "sig-c" } ] }
            ]
        })))
        .respond_with(response(json!([ { "text": "3" } ])))
        .expect(1)
        .mount(&server)
        .await;
    let client = GeminiClient::new("g-test").with_base_url(server.uri());
    let mut prompt = Prompt::builder("p").user("Calculate").build();

    let answer = client.execute(&prompt, &model(), &[]).await.unwrap();
    prompt.messages.extend([
        answer.clone(),
        Message::ToolResult(ToolResult {
            id: "gemini-call-0".into(),
            tool: "add".into(),
            content: "3".into(),
        }),
        Message::Assistant(AssistantMessage {
            parts: vec![
                AssistantPart::Reasoning(Reasoning {
                    provider: "anthropic".into(),
                    data: json!({ "type": "thinking" }),
                }),
                gemini_reasoning(json!({ "thoughtSignature": "sig-c" })),
            ],
        }),
    ]);
    let next = client.execute(&prompt, &model(), &[]).await.unwrap();

    assert_eq!(
        answer,
        Message::Assistant(AssistantMessage {
            parts: vec![
                gemini_reasoning(json!({ "text": "Thought", "thought": true })),
                gemini_reasoning(json!({ "thoughtSignature": "sig-a" })),
                AssistantPart::Text {
                    text: "One moment.".into()
                },
                gemini_reasoning(json!({ "thoughtSignature": "sig-b" })),
                call("gemini-call-0", 1, 2),
            ],
        })
    );
    assert_eq!(next, Message::assistant("3"));
}

/// The model the provider reports (ADR-0094).
#[tokio::test]
async fn reports_the_model_that_answered() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "candidates": [ { "content": { "role": "model", "parts": [ { "text": "Hi" } ] }, "finishReason": "STOP" } ],
            "modelVersion": "gemini-3-flash-001"
        })))
        .mount(&server)
        .await;
    let client = GeminiClient::new("g-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Hello").build();

    let completion = client
        .execute_with_usage(&prompt, &model(), &[])
        .await
        .unwrap();

    assert_eq!(completion.model.as_deref(), Some("gemini-3-flash-001"));
}

#[tokio::test]
async fn reports_usage_with_thoughts_as_output_and_cached_tokens() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "candidates": [ { "content": { "role": "model", "parts": [ { "text": "Hi" } ] }, "finishReason": "STOP" } ],
            "usageMetadata": {
                "promptTokenCount": 120,
                "cachedContentTokenCount": 100,
                "candidatesTokenCount": 5,
                "thoughtsTokenCount": 40,
                "totalTokenCount": 165
            }
        })))
        .mount(&server)
        .await;
    let client = GeminiClient::new("g-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Hello").build();

    let completion = client
        .execute_with_usage(&prompt, &model(), &[])
        .await
        .unwrap();

    assert_eq!(completion.message, Message::assistant("Hi"));
    assert_eq!(
        completion.usage,
        Some(Usage {
            input_tokens: 120,
            output_tokens: 45,
            cache_read_tokens: Some(100),
            cache_write_tokens: None,
        })
    );
}

#[tokio::test]
async fn reports_blocked_prompts_and_answers_as_client_errors() {
    for (body, reason) in [
        (
            json!({ "promptFeedback": { "blockReason": "PROHIBITED_CONTENT" } }),
            "PROHIBITED_CONTENT",
        ),
        (
            json!({ "candidates": [ { "finishReason": "SAFETY", "index": 0 } ] }),
            "SAFETY",
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let client = GeminiClient::new("g-test").with_base_url(server.uri());
        let prompt = Prompt::builder("p").user("Hello").build();

        let error = client.execute(&prompt, &model(), &[]).await.unwrap_err();

        assert!(matches!(error, LlmError::Client(_)), "{error}");
        assert!(error.to_string().contains(reason), "{error}");
    }
}

#[tokio::test]
async fn maps_the_tool_choice_to_the_function_calling_mode() {
    for (choice, config) in [
        (ToolChoice::Auto, None),
        (
            ToolChoice::Required,
            Some(json!({ "functionCallingConfig": { "mode": "ANY" } })),
        ),
        (
            ToolChoice::None,
            Some(json!({ "functionCallingConfig": { "mode": "NONE" } })),
        ),
        (
            ToolChoice::Tool("add".into()),
            Some(
                json!({ "functionCallingConfig": { "mode": "ANY", "allowedFunctionNames": ["add"] } }),
            ),
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(response(json!([ { "text": "ok" } ])))
            .mount(&server)
            .await;
        let client = GeminiClient::new("g-test").with_base_url(server.uri());
        let mut prompt = Prompt::builder("p").user("Calculate").build();
        prompt.params.tool_choice = choice.clone();

        client
            .execute(&prompt, &model(), &[add_tool()])
            .await
            .unwrap();

        let request: serde_json::Value = server.received_requests().await.unwrap()[0]
            .body_json()
            .unwrap();
        assert_eq!(request.get("toolConfig").cloned(), config, "{choice:?}");
    }
}

#[tokio::test]
async fn requests_structured_answers_natively_with_the_json_schema() {
    let schema = json!({
        "type": "object",
        "properties": { "title": { "type": "string" } },
        "required": ["title"]
    });
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_json(json!({
            "contents": [ { "role": "user", "parts": [ { "text": "Printer broken" } ] } ],
            "generationConfig": {
                "responseMimeType": "application/json",
                "responseJsonSchema": schema
            }
        })))
        .respond_with(response(json!([ { "text": "{\"title\":\"Printer\"}" } ])))
        .expect(1)
        .mount(&server)
        .await;
    let client = GeminiClient::new("g-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Printer broken").build();
    let output = StructuredOutput {
        name: "Ticket".into(),
        schema: schema.clone(),
    };

    let completion = client
        .execute_structured_with_usage(&prompt, &model(), &output)
        .await
        .unwrap();

    assert_eq!(
        completion.message,
        Message::assistant("{\"title\":\"Printer\"}")
    );
    assert_eq!(completion.usage.map(|usage| usage.input_tokens), Some(10));
}

/// SSE body from the given response chunks.
fn sse(chunks: &[serde_json::Value]) -> ResponseTemplate {
    let body: String = chunks
        .iter()
        .map(|chunk| format!("data: {chunk}\r\n\r\n"))
        .collect();
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

/// `modelVersion` of the chunks arrives once as a `Model` frame (ADR-0094).
#[tokio::test]
async fn streams_the_model_version_the_provider_reports() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse(&[
            json!({ "modelVersion": "gemini-3-flash-001", "candidates": [ { "content": { "role": "model", "parts": [ { "text": "Hi" } ] } } ] }),
            json!({ "modelVersion": "gemini-3-flash-001", "candidates": [ { "content": { "role": "model", "parts": [ { "text": "!" } ] }, "finishReason": "STOP" } ] }),
        ]))
        .mount(&server)
        .await;
    let client = GeminiClient::new("g-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Hello").build();

    let frames: Vec<StreamFrame> = client
        .execute_streaming(&prompt, &model(), &[])
        .map(|frame| frame.unwrap())
        .collect()
        .await;

    assert_eq!(
        frames
            .iter()
            .filter(|frame| matches!(frame, StreamFrame::Model(_)))
            .collect::<Vec<_>>(),
        [&StreamFrame::Model("gemini-3-flash-001".into())]
    );
}

#[tokio::test]
async fn streams_text_function_calls_usage_and_the_finish_reason() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-3-flash:streamGenerateContent"))
        .and(query_param("alt", "sse"))
        .and(header("x-goog-api-key", "g-test"))
        .respond_with(sse(&[
            json!({ "candidates": [ { "content": { "role": "model", "parts": [ { "text": "Hel" } ] } } ] }),
            json!({ "candidates": [ { "content": { "role": "model", "parts": [
                { "text": "lo" },
                { "functionCall": { "name": "add", "args": { "a": 1, "b": 2 } }, "thoughtSignature": "sig" }
            ] } } ] }),
            json!({
                "candidates": [ { "content": { "role": "model", "parts": [ { "text": "" } ] }, "finishReason": "STOP" } ],
                "usageMetadata": { "promptTokenCount": 10, "candidatesTokenCount": 4 }
            }),
        ]))
        .expect(1)
        .mount(&server)
        .await;
    let client = GeminiClient::new("g-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Hello").build();

    let frames: Vec<StreamFrame> = client
        .execute_streaming(&prompt, &model(), &[add_tool()])
        .map(|frame| frame.unwrap())
        .collect()
        .await;

    assert_eq!(
        frames,
        vec![
            StreamFrame::TextDelta { text: "Hel".into() },
            StreamFrame::TextDelta { text: "lo".into() },
            StreamFrame::Reasoning(Reasoning {
                provider: "gemini".into(),
                data: json!({ "thoughtSignature": "sig" }),
            }),
            StreamFrame::ToolCall(ToolCall {
                id: "gemini-call-0".into(),
                tool: "add".into(),
                args: json!({ "a": 1, "b": 2 }),
            }),
            StreamFrame::Usage(Usage {
                input_tokens: 10,
                output_tokens: 4,
                cache_read_tokens: None,
                cache_write_tokens: None,
            }),
            StreamFrame::End {
                finish_reason: Some("STOP".into())
            },
        ]
    );
}

#[tokio::test]
async fn sends_the_temperature_in_the_generation_config() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-3-flash:generateContent"))
        .and(body_json(json!({
            "contents": [ { "role": "user", "parts": [ { "text": "Hello" } ] } ],
            "generationConfig": { "temperature": 0.7 }
        })))
        .respond_with(response(json!([ { "text": "Hi" } ])))
        .expect(1)
        .mount(&server)
        .await;
    let client = GeminiClient::new("g-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Hello").build();

    client
        .execute(&prompt, &model().with_temperature(0.7), &[])
        .await
        .unwrap();
}
