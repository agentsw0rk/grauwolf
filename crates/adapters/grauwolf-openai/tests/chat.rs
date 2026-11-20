//! OpenAI Chat Completions (F02.2) against recorded responses.

use grauwolf_llm::LlmClient;
use grauwolf_openai::OpenAiClient;
use grauwolf_prompt::{LlModel, Message, Prompt};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn model() -> LlModel {
    LlModel::new("openai", "gpt-4o-mini")
}

/// Response in the Chat Completions API format with the given `message`.
fn completion(message: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "model": "gpt-4o-mini",
        "choices": [ { "index": 0, "message": message, "finish_reason": "stop" } ]
    }))
}

#[tokio::test]
async fn sends_the_conversation_with_the_api_key_and_returns_the_answer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer sk-test"))
        .and(body_json(json!({
            "model": "gpt-4o-mini",
            "messages": [
                { "role": "system", "content": "Be brief." },
                { "role": "user", "content": "Hello" }
            ]
        })))
        .respond_with(completion(json!({ "role": "assistant", "content": "Hi" })))
        .expect(1)
        .mount(&server)
        .await;
    let client = OpenAiClient::new("sk-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p")
        .system("Be brief.")
        .user("Hello")
        .build();

    let answer = client.execute(&prompt, &model(), &[]).await.unwrap();

    assert_eq!(answer, Message::assistant("Hi"));
}

fn add_tool() -> grauwolf_prompt::ToolDescriptor {
    grauwolf_prompt::ToolDescriptor {
        name: "add".into(),
        description: "Adds two numbers".into(),
        parameters: json!({ "type": "object", "properties": { "a": { "type": "number" }, "b": { "type": "number" } } }),
    }
}

#[tokio::test]
async fn offers_tools_and_returns_the_tool_call_with_parsed_arguments() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::body_partial_json(json!({
            "tools": [{
                "type": "function",
                "function": {
                    "name": "add",
                    "description": "Adds two numbers",
                    "parameters": { "type": "object", "properties": { "a": { "type": "number" }, "b": { "type": "number" } } }
                }
            }]
        })))
        .respond_with(completion(json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": "call_abc",
                "type": "function",
                "function": { "name": "add", "arguments": "{\"a\":2,\"b\":3}" }
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = OpenAiClient::new("sk-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("What is 2+3?").build();

    let answer = client
        .execute(&prompt, &model(), &[add_tool()])
        .await
        .unwrap();

    assert_eq!(
        answer,
        Message::tool_call("call_abc", "add", json!({ "a": 2, "b": 3 }))
    );
}

#[tokio::test]
async fn sends_tool_calls_and_results_from_the_history_with_their_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::body_partial_json(json!({
            "messages": [
                { "role": "user", "content": "What is 2+3?" },
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "call_abc", "type": "function",
                    "function": { "name": "add", "arguments": "{\"a\":2,\"b\":3}" }
                }] },
                { "role": "tool", "content": "5", "tool_call_id": "call_abc" }
            ]
        })))
        .respond_with(completion(
            json!({ "role": "assistant", "content": "The result is 5." }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let client = OpenAiClient::new("sk-test").with_base_url(server.uri());
    let mut prompt = Prompt::builder("p").user("What is 2+3?").build();
    prompt.messages.push(Message::tool_call(
        "call_abc",
        "add",
        json!({ "a": 2, "b": 3 }),
    ));
    prompt
        .messages
        .push(Message::tool_result("call_abc", "add", "5"));

    let answer = client
        .execute(&prompt, &model(), &[add_tool()])
        .await
        .unwrap();

    assert_eq!(answer, Message::assistant("The result is 5."));
}

#[tokio::test]
async fn reports_tool_arguments_that_are_not_json() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(completion(json!({
            "role": "assistant", "content": null,
            "tool_calls": [{ "id": "c", "type": "function", "function": { "name": "add", "arguments": "{a:2" } }]
        })))
        .mount(&server)
        .await;
    let client = OpenAiClient::new("sk-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("What is 2+3?").build();

    let error = client
        .execute(&prompt, &model(), &[add_tool()])
        .await
        .unwrap_err();

    assert!(error.to_string().contains("not JSON"), "{error}");
}

#[tokio::test]
async fn requests_structured_answers_natively_with_a_json_schema_response_format() {
    let schema = json!({ "type": "object", "properties": { "title": { "type": "string" } }, "required": ["title"] });
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_json(json!({
            "model": "gpt-4o-mini",
            "messages": [ { "role": "user", "content": "Login bug" } ],
            "response_format": {
                "type": "json_schema",
                "json_schema": { "name": "Ticket", "schema": schema }
            }
        })))
        .respond_with(completion(
            json!({ "role": "assistant", "content": "{\"title\":\"Login kaputt\"}" }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let client = OpenAiClient::new("sk-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Login bug").build();
    let output = grauwolf_llm::StructuredOutput {
        name: "Ticket".into(),
        schema: schema.clone(),
    };

    let answer = client
        .execute_structured(&prompt, &model(), &output)
        .await
        .unwrap();

    assert_eq!(answer, Message::assistant(r#"{"title":"Login kaputt"}"#));
}

#[tokio::test]
async fn returns_every_tool_call_of_an_answer_with_its_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(completion(json!({
            "role": "assistant",
            "content": "I am calculating both.",
            "tool_calls": [
                { "id": "call_1", "type": "function", "function": { "name": "add", "arguments": "{\"a\":2,\"b\":3}" } },
                { "id": "call_2", "type": "function", "function": { "name": "add", "arguments": "{\"a\":4,\"b\":5}" } }
            ]
        })))
        .mount(&server)
        .await;
    let client = OpenAiClient::new("sk-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("What is 2+3 and 4+5?").build();

    let answer = client
        .execute(&prompt, &model(), &[add_tool()])
        .await
        .unwrap();

    let Message::Assistant(answer) = answer else {
        panic!("Expected a response: {answer:?}");
    };
    assert_eq!(answer.text(), "I am calculating both.");
    let calls: Vec<&str> = answer.tool_calls().map(|call| call.id.as_str()).collect();
    assert_eq!(calls, ["call_1", "call_2"]);
}

#[tokio::test]
async fn streams_text_deltas_and_assembles_tool_call_fragments() {
    let chunk = |delta: serde_json::Value, finish: Option<&str>| {
        format!(
            "data: {}\n\n",
            json!({ "choices": [ { "index": 0, "delta": delta, "finish_reason": finish } ] })
        )
    };
    let body = [
        chunk(json!({ "role": "assistant", "content": "I " }), None),
        chunk(json!({ "content": "am calculating." }), None),
        chunk(
            json!({ "tool_calls": [ { "index": 0, "id": "call_1", "type": "function",
            "function": { "name": "add", "arguments": "" } } ] }),
            None,
        ),
        chunk(
            json!({ "tool_calls": [ { "index": 0, "function": { "arguments": "{\"a\":2," } } ] }),
            None,
        ),
        chunk(
            json!({ "tool_calls": [ { "index": 0, "function": { "arguments": "\"b\":3}" } } ] }),
            None,
        ),
        chunk(json!({}), Some("tool_calls")),
        "data: [DONE]\n\n".to_string(),
    ]
    .concat();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::body_partial_json(
            json!({ "stream": true }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .expect(1)
        .mount(&server)
        .await;
    let client = OpenAiClient::new("sk-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("What is 2+3?").build();

    let frames: Vec<grauwolf_llm::StreamFrame> = futures_util::StreamExt::collect::<Vec<_>>(
        client.execute_streaming(&prompt, &model(), &[add_tool()]),
    )
    .await
    .into_iter()
    .map(Result::unwrap)
    .collect();

    assert_eq!(
        frames,
        [
            grauwolf_llm::StreamFrame::TextDelta { text: "I ".into() },
            grauwolf_llm::StreamFrame::TextDelta {
                text: "am calculating.".into()
            },
            grauwolf_llm::StreamFrame::ToolCall(grauwolf_prompt::ToolCall {
                id: "call_1".into(),
                tool: "add".into(),
                args: json!({ "a": 2, "b": 3 }),
            }),
            grauwolf_llm::StreamFrame::End {
                finish_reason: Some("tool_calls".into())
            },
        ]
    );
}

#[tokio::test]
async fn sends_the_tool_choice_of_the_prompt() {
    use grauwolf_prompt::ToolChoice;

    let server = MockServer::start().await;
    for (choice, expected) in [
        (ToolChoice::Required, json!("required")),
        (ToolChoice::None, json!("none")),
        (
            ToolChoice::Tool("add".into()),
            json!({ "type": "function", "function": { "name": "add" } }),
        ),
    ] {
        server.reset().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(wiremock::matchers::body_partial_json(
                json!({ "tool_choice": expected }),
            ))
            .respond_with(completion(json!({ "role": "assistant", "content": "ok" })))
            .expect(1)
            .mount(&server)
            .await;
        let mut prompt = Prompt::builder("p").user("Rechne").build();
        prompt.params.tool_choice = choice;

        OpenAiClient::new("sk-test")
            .with_base_url(server.uri())
            .execute(&prompt, &model(), &[add_tool()])
            .await
            .unwrap();
        server.verify().await;
    }
}

#[tokio::test]
async fn requests_several_choices_natively_with_n() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(wiremock::matchers::body_partial_json(json!({ "n": 2 })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [
                { "index": 0, "message": { "role": "assistant", "content": "Erste" }, "finish_reason": "stop" },
                { "index": 1, "message": { "role": "assistant", "content": "Zweite" }, "finish_reason": "stop" }
            ],
            "usage": { "prompt_tokens": 12, "completion_tokens": 6, "total_tokens": 18 }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let prompt = Prompt::builder("p").user("Hello").build();

    let choices = OpenAiClient::new("sk-test")
        .with_base_url(server.uri())
        .execute_choices(&prompt, &model(), &[], 2)
        .await
        .unwrap();

    assert_eq!(
        choices.messages,
        [Message::assistant("Erste"), Message::assistant("Zweite")]
    );
    let usage = choices.usage.unwrap();
    assert_eq!((usage.input_tokens, usage.output_tokens), (12, 6));
}

/// Token usage (ADR-0067): `prompt_tokens` already includes cached tokens.
#[tokio::test]
async fn reports_token_usage_with_cached_tokens() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "model": "gpt-4o-mini",
            "choices": [ { "index": 0, "finish_reason": "stop",
                           "message": { "role": "assistant", "content": "Hi" } } ],
            "usage": {
                "prompt_tokens": 2006,
                "completion_tokens": 3,
                "total_tokens": 2009,
                "prompt_tokens_details": { "cached_tokens": 1920 }
            }
        })))
        .mount(&server)
        .await;
    let client = OpenAiClient::new("sk-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Hello").build();

    let completion = client
        .execute_with_usage(&prompt, &model(), &[])
        .await
        .unwrap();

    assert_eq!(completion.message, Message::assistant("Hi"));
    assert_eq!(
        completion.usage,
        Some(grauwolf_prompt::Usage {
            input_tokens: 2006,
            output_tokens: 3,
            cache_read_tokens: Some(1920),
            cache_write_tokens: None,
        })
    );
}

/// The model the provider reports (ADR-0094).
#[tokio::test]
async fn reports_the_model_that_answered() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-1", "object": "chat.completion",
            "model": "gpt-4o-mini-2024-07-18",
            "choices": [ { "index": 0, "finish_reason": "stop",
                           "message": { "role": "assistant", "content": "Hi" } } ]
        })))
        .mount(&server)
        .await;
    let client = OpenAiClient::new("sk-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Hello").build();

    let completion = client
        .execute_with_usage(&prompt, &model(), &[])
        .await
        .unwrap();

    assert_eq!(completion.model.as_deref(), Some("gpt-4o-mini-2024-07-18"));
}

/// Token usage of structured responses (ADR-0067).
#[tokio::test]
async fn reports_token_usage_of_structured_answers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-1", "object": "chat.completion", "model": "gpt-4o-mini",
            "choices": [ { "index": 0, "finish_reason": "stop",
                           "message": { "role": "assistant", "content": "{\"ok\": true}" } } ],
            "usage": { "prompt_tokens": 30, "completion_tokens": 5, "total_tokens": 35 }
        })))
        .mount(&server)
        .await;
    let client = OpenAiClient::new("sk-test").with_base_url(server.uri());
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

/// Token usage in the stream (ADR-0067): with `stream_options.include_usage` it arrives in a
/// last chunk without `choices`.
#[tokio::test]
async fn streams_the_token_usage_before_the_end() {
    let body = [
        format!(
            "data: {}\n\n",
            json!({ "choices": [ { "index": 0, "delta": { "content": "Hi" }, "finish_reason": "stop" } ] })
        ),
        format!(
            "data: {}\n\n",
            json!({ "choices": [], "usage": { "prompt_tokens": 2006, "completion_tokens": 3,
                    "prompt_tokens_details": { "cached_tokens": 1920 } } })
        ),
        "data: [DONE]\n\n".to_string(),
    ]
    .concat();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::body_partial_json(json!({
            "stream": true,
            "stream_options": { "include_usage": true }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .expect(1)
        .mount(&server)
        .await;
    let client = OpenAiClient::new("sk-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Hello").build();

    let frames: Vec<grauwolf_llm::StreamFrame> = futures_util::StreamExt::collect::<Vec<_>>(
        client.execute_streaming(&prompt, &model(), &[]),
    )
    .await
    .into_iter()
    .map(Result::unwrap)
    .collect();

    assert_eq!(
        frames[frames.len() - 2..],
        [
            grauwolf_llm::StreamFrame::Usage(grauwolf_prompt::Usage {
                input_tokens: 2006,
                output_tokens: 3,
                cache_read_tokens: Some(1920),
                cache_write_tokens: None,
            }),
            grauwolf_llm::StreamFrame::End {
                finish_reason: Some("stop".into())
            },
        ]
    );
}

/// The model the provider reports in the stream arrives as a `Model` frame (ADR-0094).
#[tokio::test]
async fn streams_the_model_the_provider_reports() {
    let body = [
        format!(
            "data: {}\n\n",
            json!({ "model": "gpt-4o-mini-2024-07-18", "choices": [ { "index": 0, "delta": { "content": "Hi" } } ] })
        ),
        format!(
            "data: {}\n\n",
            json!({ "model": "gpt-4o-mini-2024-07-18", "choices": [ { "index": 0, "delta": {}, "finish_reason": "stop" } ] })
        ),
        "data: [DONE]\n\n".to_string(),
    ]
    .concat();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let client = OpenAiClient::new("sk-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Hello").build();

    let frames: Vec<grauwolf_llm::StreamFrame> = futures_util::StreamExt::collect::<Vec<_>>(
        client.execute_streaming(&prompt, &model(), &[]),
    )
    .await
    .into_iter()
    .map(Result::unwrap)
    .collect();

    let models: Vec<_> = frames
        .iter()
        .filter(|frame| matches!(frame, grauwolf_llm::StreamFrame::Model(_)))
        .collect();
    assert_eq!(
        models,
        [&grauwolf_llm::StreamFrame::Model(
            "gpt-4o-mini-2024-07-18".into()
        )]
    );
}

#[tokio::test]
async fn sends_the_temperature_of_the_model() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_json(json!({
            "model": "gpt-4o-mini",
            "temperature": 0.0,
            "messages": [ { "role": "user", "content": "Hello" } ]
        })))
        .respond_with(completion(json!({ "role": "assistant", "content": "Hi" })))
        .expect(1)
        .mount(&server)
        .await;
    let client = OpenAiClient::new("sk-test").with_base_url(server.uri());
    let prompt = Prompt::builder("p").user("Hello").build();

    client
        .execute(&prompt, &model().with_temperature(0.0), &[])
        .await
        .unwrap();
}
