//! Ollama `/api/chat` (F02.4) against recorded responses.

use grauwolf_llm::LlmClient;
use grauwolf_ollama::OllamaClient;
use grauwolf_prompt::{LlModel, Message, Prompt};
use serde_json::json;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn model() -> LlModel {
    LlModel::new("ollama", "qwen3.5:9b")
}

#[tokio::test]
async fn sends_the_conversation_and_returns_the_assistant_answer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(body_json(json!({
            "model": "qwen3.5:9b",
            "stream": false,
            "messages": [
                { "role": "system", "content": "Be brief." },
                { "role": "user", "content": "Hello" }
            ]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "qwen3.5:9b",
            "created_at": "2026-11-06T12:00:00Z",
            "message": { "role": "assistant", "content": "Hi" },
            "done": true,
            "done_reason": "stop"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = OllamaClient::new(server.uri());
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
async fn offers_tools_and_returns_the_tool_call() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
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
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "qwen3.5:9b",
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [ { "function": { "index": 0, "name": "add", "arguments": { "a": 2, "b": 3 } } } ]
            },
            "done": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = OllamaClient::new(server.uri());
    let prompt = Prompt::builder("p").user("What is 2+3?").build();

    let answer = client
        .execute(&prompt, &model(), &[add_tool()])
        .await
        .unwrap();

    assert_eq!(
        answer,
        Message::tool_call("add", "add", json!({ "a": 2, "b": 3 }))
    );
}

#[tokio::test]
async fn sends_tool_calls_and_results_from_the_history() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(wiremock::matchers::body_partial_json(json!({
            "messages": [
                { "role": "user", "content": "What is 2+3?" },
                { "role": "assistant", "content": "",
                  "tool_calls": [ { "function": { "name": "add", "arguments": { "a": 2, "b": 3 } } } ] },
                { "role": "tool", "content": "5", "tool_name": "add" }
            ]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "message": { "role": "assistant", "content": "The result is 5." },
            "done": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = OllamaClient::new(server.uri());
    let mut prompt = Prompt::builder("p").user("What is 2+3?").build();
    prompt
        .messages
        .push(Message::tool_call("add", "add", json!({ "a": 2, "b": 3 })));
    prompt
        .messages
        .push(Message::tool_result("add", "add", "5"));

    let answer = client
        .execute(&prompt, &model(), &[add_tool()])
        .await
        .unwrap();

    assert_eq!(answer, Message::assistant("The result is 5."));
}

#[tokio::test]
async fn requests_structured_answers_natively_with_format() {
    let schema = json!({ "type": "object", "properties": { "title": { "type": "string" } }, "required": ["title"] });
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(body_json(json!({
            "model": "qwen3.5:9b",
            "stream": false,
            "messages": [ { "role": "user", "content": "Login bug" } ],
            "format": schema
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "message": { "role": "assistant", "content": "{\"title\":\"Login kaputt\"}" },
            "done": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = OllamaClient::new(server.uri());
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
async fn returns_every_tool_call_of_an_answer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    { "function": { "name": "add", "arguments": { "a": 2, "b": 3 } } },
                    { "id": "call_2", "function": { "name": "add", "arguments": { "a": 4, "b": 5 } } }
                ]
            },
            "done": true
        })))
        .mount(&server)
        .await;
    let client = OllamaClient::new(server.uri());
    let prompt = Prompt::builder("p").user("What is 2+3 and 4+5?").build();

    let answer = client
        .execute(&prompt, &model(), &[add_tool()])
        .await
        .unwrap();

    let Message::Assistant(answer) = answer else {
        panic!("Expected a response: {answer:?}");
    };
    let calls: Vec<(&str, &str)> = answer
        .tool_calls()
        .map(|call| (call.id.as_str(), call.tool.as_str()))
        .collect();
    assert_eq!(calls, [("add", "add"), ("call_2", "add")]);
}

#[tokio::test]
async fn reports_overload_as_a_transient_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let client = OllamaClient::new(server.uri());
    let prompt = Prompt::builder("p").user("Hello").build();

    let error = client.execute(&prompt, &model(), &[]).await.unwrap_err();

    assert!(
        matches!(error, grauwolf_llm::LlmError::Transient { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn streams_text_deltas_and_tool_calls_from_ndjson() {
    let body = [
        json!({ "message": { "role": "assistant", "content": "I " }, "done": false }),
        json!({ "message": { "role": "assistant", "content": "am calculating." }, "done": false }),
        json!({ "message": { "role": "assistant", "content": "",
            "tool_calls": [ { "function": { "name": "add", "arguments": { "a": 2, "b": 3 } } } ] }, "done": false }),
        json!({ "message": { "role": "assistant", "content": "" }, "done": true, "done_reason": "stop" }),
    ]
    .iter()
    .map(|line| line.to_string())
    .collect::<Vec<_>>()
    .join("\n");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::body_partial_json(
            json!({ "stream": true }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .expect(1)
        .mount(&server)
        .await;
    let client = OllamaClient::new(server.uri());
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
                id: "add".into(),
                tool: "add".into(),
                args: json!({ "a": 2, "b": 3 }),
            }),
            grauwolf_llm::StreamFrame::End {
                finish_reason: Some("stop".into())
            },
        ]
    );
}

#[tokio::test]
async fn sends_the_think_setting_when_configured() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::body_partial_json(
            json!({ "think": false }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "message": { "role": "assistant", "content": "Hi" }, "done": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = OllamaClient::new(server.uri()).with_think(false);
    let prompt = Prompt::builder("p").user("Hello").build();

    let answer = client.execute(&prompt, &model(), &[]).await.unwrap();

    assert_eq!(answer, Message::assistant("Hi"));
}

fn multiply_tool() -> grauwolf_prompt::ToolDescriptor {
    grauwolf_prompt::ToolDescriptor {
        name: "multiply".into(),
        ..add_tool()
    }
}

/// Ollama has no tool choice; it is approximated via the offered tools and an instruction
/// (ADR-0042).
#[tokio::test]
async fn approximates_the_tool_choice_with_offered_tools_and_an_instruction() {
    use grauwolf_prompt::ToolChoice;

    let server = MockServer::start().await;
    let answer = ResponseTemplate::new(200).set_body_json(json!({
        "message": { "role": "assistant", "content": "ok" }, "done": true
    }));
    let client = OllamaClient::new(server.uri());
    let tools = [add_tool(), multiply_tool()];
    for (choice, offered, instruction) in [
        (
            ToolChoice::Tool("multiply".into()),
            vec!["multiply"],
            Some("Call the tool multiply now."),
        ),
        (
            ToolChoice::Required,
            vec!["add", "multiply"],
            Some("Call one of the tools now; do not answer with text."),
        ),
        (ToolChoice::None, vec![], None),
    ] {
        server.reset().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(answer.clone())
            .expect(1)
            .mount(&server)
            .await;
        let mut prompt = Prompt::builder("p").user("Rechne").build();
        prompt.params.tool_choice = choice;

        client.execute(&prompt, &model(), &tools).await.unwrap();

        let body: serde_json::Value = server.received_requests().await.unwrap()[0]
            .body_json()
            .unwrap();
        let names: Vec<&str> = body["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|tool| tool["function"]["name"].as_str().unwrap())
            .collect();
        let last = body["messages"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(names, offered);
        match instruction {
            Some(text) => assert_eq!(last, json!({ "role": "system", "content": text })),
            None => assert_eq!(last["role"], "user"),
        }
    }
}

/// The model the provider reports (ADR-0094).
#[tokio::test]
async fn reports_the_model_that_answered() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "qwen3.5:9b",
            "message": { "role": "assistant", "content": "Hi" },
            "done": true
        })))
        .mount(&server)
        .await;
    let client = OllamaClient::new(server.uri());
    let prompt = Prompt::builder("p").user("Hello").build();

    let completion = client
        .execute_with_usage(&prompt, &model(), &[])
        .await
        .unwrap();

    assert_eq!(completion.model.as_deref(), Some("qwen3.5:9b"));
}

/// Token usage (ADR-0067): `prompt_eval_count` and `eval_count`, without cache details.
#[tokio::test]
async fn reports_token_usage() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "llama3.2",
            "message": { "role": "assistant", "content": "Hi" },
            "done": true,
            "prompt_eval_count": 26,
            "eval_count": 3
        })))
        .mount(&server)
        .await;
    let client = OllamaClient::new(server.uri());
    let prompt = Prompt::builder("p").user("Hello").build();

    let completion = client
        .execute_with_usage(&prompt, &model(), &[])
        .await
        .unwrap();

    assert_eq!(completion.message, Message::assistant("Hi"));
    assert_eq!(
        completion.usage,
        Some(grauwolf_prompt::Usage {
            input_tokens: 26,
            output_tokens: 3,
            cache_read_tokens: None,
            cache_write_tokens: None,
        })
    );
}

/// Token usage of structured responses (ADR-0067).
#[tokio::test]
async fn reports_token_usage_of_structured_answers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "llama3.2",
            "message": { "role": "assistant", "content": "{\"ok\": true}" },
            "done": true,
            "prompt_eval_count": 30,
            "eval_count": 5
        })))
        .mount(&server)
        .await;
    let client = OllamaClient::new(server.uri());
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

/// The `model` of the last line arrives as a `Model` frame before the end (ADR-0094).
#[tokio::test]
async fn streams_the_model_the_server_reports() {
    let body = [
        json!({ "model": "qwen3.5:9b", "message": { "role": "assistant", "content": "Hi" }, "done": false }),
        json!({ "model": "qwen3.5:9b", "message": { "role": "assistant", "content": "" }, "done": true,
                "done_reason": "stop" }),
    ]
    .iter()
    .map(|line| line.to_string())
    .collect::<Vec<_>>()
    .join("\n");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let client = OllamaClient::new(server.uri());
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
            grauwolf_llm::StreamFrame::Model("qwen3.5:9b".into()),
            grauwolf_llm::StreamFrame::End {
                finish_reason: Some("stop".into())
            },
        ]
    );
}

/// Token usage in the stream (ADR-0067): from the last line with `done: true`.
#[tokio::test]
async fn streams_the_token_usage_before_the_end() {
    let body = [
        json!({ "message": { "role": "assistant", "content": "Hi" }, "done": false }),
        json!({ "message": { "role": "assistant", "content": "" }, "done": true,
                "done_reason": "stop", "prompt_eval_count": 26, "eval_count": 3 }),
    ]
    .iter()
    .map(|line| line.to_string())
    .collect::<Vec<_>>()
    .join("\n");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let client = OllamaClient::new(server.uri());
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
                input_tokens: 26,
                output_tokens: 3,
                cache_read_tokens: None,
                cache_write_tokens: None,
            }),
            grauwolf_llm::StreamFrame::End {
                finish_reason: Some("stop".into())
            },
        ]
    );
}

#[tokio::test]
async fn sends_temperature_and_think_of_the_model_and_think_wins_over_the_client() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(body_json(json!({
            "model": "qwen3.5:9b",
            "stream": false,
            "messages": [ { "role": "user", "content": "Hello" } ],
            "think": true,
            "options": { "temperature": 0.0 }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "qwen3.5:9b",
            "message": { "role": "assistant", "content": "Hi" },
            "done": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = OllamaClient::new(server.uri()).with_think(false);
    let prompt = Prompt::builder("p").user("Hello").build();

    client
        .execute(
            &prompt,
            &model().with_temperature(0.0).with_think(true),
            &[],
        )
        .await
        .unwrap();
}
