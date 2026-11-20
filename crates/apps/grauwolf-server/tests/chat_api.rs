//! `POST /api/chat` and `GET /api/signature` (ADR-0094).

use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_agents::memory::MemoryChatHistoryStore;
use grauwolf_interpreter::RunExtras;
use grauwolf_server::{ChatApiOptions, ServedAgent, ServiceOptions};
use serde::Deserialize;
use serde_json::{Value, json};

/// Text in, text out: shouts, or fails on the word `fail`.
struct Shout;

impl ServedAgent for Shout {
    fn input_schema(&self) -> Option<Value> {
        None
    }

    fn run(
        &self,
        input: Value,
        _extras: RunExtras,
        user: Option<String>,
    ) -> BoxFuture<'_, Result<Value, String>> {
        Box::pin(async move {
            let text = input.as_str().ok_or("input is not text")?;
            if text == "fail" {
                return Err("the run failed".into());
            }
            Ok(json!(format!(
                "{} ({})",
                text.to_uppercase(),
                user.as_deref().unwrap_or("nobody")
            )))
        })
    }
}

/// Typed in, typed out: the input object with a `length` of its `text`.
struct Measure;

impl ServedAgent for Measure {
    fn input_schema(&self) -> Option<Value> {
        Some(json!({ "type": "object", "properties": { "text": { "type": "string" } } }))
    }

    fn run(
        &self,
        input: Value,
        _extras: RunExtras,
        _user: Option<String>,
    ) -> BoxFuture<'_, Result<Value, String>> {
        Box::pin(async move {
            let text = input["text"].as_str().ok_or("no text")?;
            Ok(json!({ "text": text, "length": text.len() }))
        })
    }
}

fn options(signature: Option<Value>) -> ServiceOptions {
    ServiceOptions {
        chat_api: Some(ChatApiOptions {
            signature,
            observations: Arc::new(|| {
                Box::pin(async {
                    json!({
                        "startedAt": "2026-11-11T10:00:00.000Z",
                        "models": { "main": [ {
                            "modelId": "m-1",
                            "firstSeen": "2026-11-11T10:00:05.000Z",
                            "lastSeen": "2026-11-11T10:00:09.000Z"
                        } ] },
                        "mcpTools": { "docs": [ {
                            "tools": [ { "name": "search", "description": "Searches", "schemaHash": "ab" } ],
                            "firstSeen": "2026-11-11T10:00:05.000Z",
                            "lastSeen": "2026-11-11T10:00:05.000Z"
                        } ] }
                    })
                })
            }),
        }),
        ..ServiceOptions::default()
    }
}

async fn serve(
    agent: Arc<dyn ServedAgent>,
    options: ServiceOptions,
    token: Option<&str>,
) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router =
        grauwolf_server::router_for(agent, Arc::new(MemoryChatHistoryStore::default()), options);
    let router = match token {
        Some(token) => grauwolf_server::with_token(router, token),
        None => router,
    };
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    base
}

async fn chat(base: &str, body: &str) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("{base}/api/chat"))
        .header("content-type", "application/json")
        .body(body.to_owned())
        .send()
        .await
        .unwrap();
    (response.status().as_u16(), response.json().await.unwrap())
}

#[tokio::test]
async fn answers_a_message_with_a_response() {
    let base = serve(Arc::new(Shout), options(None), None).await;

    let (status, body) = chat(&base, r#"{"message": "hello"}"#).await;

    assert_eq!(
        (status, body),
        (200, json!({ "response": "HELLO (nobody)" }))
    );
}

#[tokio::test]
async fn typed_agents_take_the_input_object_as_the_body_and_answer_the_output_object() {
    let base = serve(Arc::new(Measure), options(None), None).await;

    let (status, body) = chat(&base, r#"{"text": "wolf"}"#).await;

    assert_eq!(
        (status, body),
        (200, json!({ "text": "wolf", "length": 4 }))
    );
}

#[tokio::test]
async fn refuses_bodies_of_the_wrong_shape_with_400() {
    let text = serve(Arc::new(Shout), options(None), None).await;
    let typed = serve(Arc::new(Measure), options(None), None).await;

    for (base, body) in [
        (&text, "not json"),
        (&text, r#"{"message": 5}"#),
        (&text, r#"{"input": "hello"}"#),
        (&typed, "[1, 2]"),
        (&typed, "not json"),
    ] {
        let (status, answer) = chat(base, body).await;
        assert_eq!(status, 400, "{body}");
        assert!(answer["error"].is_string(), "{body}: {answer}");
    }
}

#[tokio::test]
async fn reports_a_failed_run_as_500_with_the_error() {
    let base = serve(Arc::new(Shout), options(None), None).await;

    let (status, body) = chat(&base, r#"{"message": "fail"}"#).await;

    assert_eq!((status, body), (500, json!({ "error": "the run failed" })));
}

#[tokio::test]
async fn there_are_no_chat_routes_without_the_opt_in() {
    let base = serve(Arc::new(Shout), ServiceOptions::default(), None).await;

    let chat = reqwest::Client::new()
        .post(format!("{base}/api/chat"))
        .json(&json!({ "message": "hi" }))
        .send()
        .await
        .unwrap();
    let signature = reqwest::get(format!("{base}/api/signature")).await.unwrap();

    assert_eq!(chat.status(), 404);
    assert_eq!(signature.status(), 404);
}

#[tokio::test]
async fn chat_needs_a_token_but_the_signature_does_not() {
    let base = serve(
        Arc::new(Shout),
        options(Some(json!({ "harnessId": "h1" }))),
        Some("secret"),
    )
    .await;
    let client = reqwest::Client::new();

    let refused = client
        .post(format!("{base}/api/chat"))
        .json(&json!({ "message": "hi" }))
        .send()
        .await
        .unwrap();
    let allowed = client
        .post(format!("{base}/api/chat"))
        .bearer_auth("secret")
        .json(&json!({ "message": "hi" }))
        .send()
        .await
        .unwrap();
    let signature = reqwest::get(format!("{base}/api/signature")).await.unwrap();

    assert_eq!(refused.status(), 401);
    assert_eq!(allowed.status(), 200);
    assert_eq!(signature.status(), 200);
}

#[tokio::test]
async fn the_signature_is_the_file_content_and_null_without_one() {
    let with = serve(
        Arc::new(Shout),
        options(Some(json!({ "harnessId": "h1" }))),
        None,
    )
    .await;
    let without = serve(Arc::new(Shout), options(None), None).await;

    let with: Value = reqwest::get(format!("{with}/api/signature"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let without: Value = reqwest::get(format!("{without}/api/signature"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(with["signature"], json!({ "harnessId": "h1" }));
    assert_eq!(without["signature"], Value::Null);
    assert!(with["observations"].is_object());
}

/// The shape the poller's parser expects (`ObservationStore.parse`): camelCase names,
/// ISO-8601 instants, `observations` required.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Report {
    signature: Option<Value>,
    observations: Observations,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Observations {
    started_at: String,
    models: std::collections::BTreeMap<String, Vec<Model>>,
    mcp_tools: std::collections::BTreeMap<String, Vec<ToolList>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Model {
    model_id: Option<String>,
    first_seen: String,
    last_seen: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ToolList {
    tools: Vec<Tool>,
    first_seen: String,
    last_seen: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Tool {
    name: String,
    description: String,
    schema_hash: String,
}

#[tokio::test]
async fn the_signature_response_has_the_shape_the_poller_parses() {
    let base = serve(Arc::new(Shout), options(None), None).await;

    let text = reqwest::get(format!("{base}/api/signature"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    let report: Report = serde_json::from_str(&text).unwrap();
    assert!(report.signature.is_none());
    assert!(report.observations.started_at.ends_with('Z'));
    let model = &report.observations.models["main"][0];
    assert_eq!(model.model_id.as_deref(), Some("m-1"));
    assert!(model.first_seen < model.last_seen);
    let tools = &report.observations.mcp_tools["docs"][0];
    assert_eq!(tools.tools[0].name, "search");
    assert_eq!(tools.tools[0].description, "Searches");
    assert_eq!(tools.tools[0].schema_hash, "ab");
    assert_eq!(tools.first_seen, tools.last_seen);
}
