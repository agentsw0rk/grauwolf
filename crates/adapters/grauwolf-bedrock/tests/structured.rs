//! Structured answers (ADR-0031, ADR-0105) via a forced tool.

use grauwolf_bedrock::BedrockClient;
use grauwolf_llm::{LlmClient, LlmError, StructuredOutput};
use grauwolf_prompt::{LlModel, Message, Prompt};
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(server: &MockServer) -> BedrockClient {
    BedrockClient::with_api_key("key", "us-west-2").with_base_url(server.uri())
}

fn model() -> LlModel {
    LlModel::new("bedrock", "amazon.nova-pro-v1:0")
}

fn ticket() -> StructuredOutput {
    StructuredOutput {
        name: "Ticket".into(),
        schema: json!({ "type": "object", "properties": { "title": { "type": "string" } }, "required": ["title"] }),
    }
}

async fn server_answering(content: Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "output": { "message": { "role": "assistant", "content": content } },
            "stopReason": "tool_use",
            "usage": { "inputTokens": 7, "outputTokens": 3 }
        })))
        .mount(&server)
        .await;
    server
}

async fn body_of(server: &MockServer) -> Value {
    server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap()
}

#[tokio::test]
async fn forces_a_tool_with_the_schema_and_returns_its_input_as_json() {
    let server = server_answering(json!([
        { "toolUse": { "toolUseId": "t1", "name": "Ticket", "input": { "title": "Printer broken" } } }
    ]))
    .await;
    let prompt = Prompt::builder("p").user("Printer broken").build();

    let completion = client(&server)
        .execute_structured_with_usage(&prompt, &model(), &ticket())
        .await
        .unwrap();

    assert_eq!(
        serde_json::from_str::<Value>(&text_of(&completion.message)).unwrap(),
        json!({ "title": "Printer broken" })
    );
    assert_eq!(completion.usage.unwrap().output_tokens, 3);
    let body = body_of(&server).await;
    assert_eq!(
        body["toolConfig"]["toolChoice"],
        json!({ "tool": { "name": "Ticket" } })
    );
    assert_eq!(
        body["toolConfig"]["tools"][0]["toolSpec"]["inputSchema"]["json"],
        ticket().schema
    );
}

fn text_of(message: &Message) -> String {
    match message {
        Message::Assistant(answer) => answer
            .parts
            .iter()
            .filter_map(|part| match part {
                grauwolf_prompt::AssistantPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect(),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn wraps_a_schema_that_is_not_an_object_and_unwraps_the_answer() {
    let server = server_answering(json!([
        { "toolUse": { "toolUseId": "t1", "name": "Tags", "input": { "value": ["a", "b"] } } }
    ]))
    .await;
    let output = StructuredOutput {
        name: "Tags".into(),
        schema: json!({ "type": "array", "items": { "type": "string" } }),
    };
    let prompt = Prompt::builder("p").user("Tag it").build();

    let answer = client(&server)
        .execute_structured(&prompt, &model(), &output)
        .await
        .unwrap();

    assert_eq!(text_of(&answer), r#"["a","b"]"#);
    let body = body_of(&server).await;
    assert_eq!(
        body["toolConfig"]["tools"][0]["toolSpec"]["inputSchema"]["json"]["required"],
        json!(["value"])
    );
}

#[tokio::test]
async fn gives_the_tool_a_valid_name_for_any_type_name() {
    let server = server_answering(json!([ { "text": "{}" } ])).await;
    let output = StructuredOutput {
        name: "Ticket<Summary> v2".into(),
        ..ticket()
    };
    let prompt = Prompt::builder("p").user("x").build();

    client(&server)
        .execute_structured(&prompt, &model(), &output)
        .await
        .unwrap();

    let body = body_of(&server).await;
    assert_eq!(
        body["toolConfig"]["tools"][0]["toolSpec"]["name"],
        "Ticket_Summary__v2"
    );
}

#[tokio::test]
async fn fails_when_the_model_neither_calls_the_tool_nor_answers() {
    let server = server_answering(json!([])).await;
    let prompt = Prompt::builder("p").user("x").build();

    let error = client(&server)
        .execute_structured(&prompt, &model(), &ticket())
        .await
        .unwrap_err();

    assert!(matches!(error, LlmError::Client(_)), "{error:?}");
}

#[tokio::test]
async fn asks_for_json_in_the_prompt_for_models_that_cannot_force_a_tool() {
    let server = server_answering(json!([ { "text": "{\"title\":\"x\"}" } ])).await;
    let prompt = Prompt::builder("p").user("x").build();
    let model = LlModel::new("bedrock", "eu.anthropic.claude-opus-5-5-v1:0");

    let answer = client(&server)
        .execute_structured(&prompt, &model, &ticket())
        .await
        .unwrap();

    assert_eq!(text_of(&answer), r#"{"title":"x"}"#);
    let body = body_of(&server).await;
    assert!(body.get("toolConfig").is_none(), "{body}");
    assert!(
        body["messages"][0]["content"][1]["text"]
            .as_str()
            .unwrap()
            .contains("JSON schema")
    );
}
