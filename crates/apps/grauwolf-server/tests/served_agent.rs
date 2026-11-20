//! Any agent behind the trait `ServedAgent` gets the same routes as an interpreted one (ADR-0093).

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_agents::memory::MemoryChatHistoryStore;
use grauwolf_interpreter::RunExtras;
use grauwolf_mcp::{McpClient, McpServer};
use grauwolf_server::a2a::AgentCardInfo;
use grauwolf_server::{ServedAgent, ServiceOptions};
use grauwolf_spec::AgentSpec;
use serde_json::{Value, json};

/// An agent that is not interpreted: shouts its input and names the user.
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
            let text = input.as_str().ok_or("input is not text")?.to_uppercase();
            Ok(json!(format!(
                "{text} ({})",
                user.as_deref().unwrap_or("nobody")
            )))
        })
    }
}

/// Starts all routes for `Shout` and returns the base URL.
async fn serve() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/specs/echo.json");
    let spec = AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let agent: Arc<dyn ServedAgent> = Arc::new(Shout);
    let history = Arc::new(MemoryChatHistoryStore::default());
    let options = ServiceOptions::default();
    let router = grauwolf_server::router_for(agent.clone(), history.clone(), options.clone())
        .merge(grauwolf_server::a2a::router_for(
            agent.clone(),
            history,
            AgentCardInfo::from_spec(&spec, &base),
            options.clone(),
        ))
        .merge(grauwolf_server::mcp_router_for(
            McpServer::new("shout", Vec::new()),
            agent,
            "Shouts".into(),
            options,
        ));
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    base
}

#[tokio::test]
async fn serves_health_and_runs() {
    let base = serve().await;
    let client = reqwest::Client::new();

    let health = reqwest::get(format!("{base}/health")).await.unwrap();
    let answer: Value = client
        .post(format!("{base}/run"))
        .json(&json!({ "input": "hello", "user_id": "anna" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let failed = client
        .post(format!("{base}/run"))
        .json(&json!({ "input": 5 }))
        .send()
        .await
        .unwrap();

    assert_eq!(health.text().await.unwrap(), "ok");
    assert_eq!(answer, json!({ "output": "HELLO (anna)" }));
    assert_eq!(failed.status(), 500);
    assert_eq!(
        failed.json::<Value>().await.unwrap(),
        json!({ "error": "input is not text" })
    );
}

#[tokio::test]
async fn streams_the_output() {
    let base = serve().await;

    let body = reqwest::Client::new()
        .post(format!("{base}/run/stream"))
        .json(&json!({ "input": "hello" }))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(
        body.contains("event: output\ndata: \"HELLO (nobody)\""),
        "{body}"
    );
}

#[tokio::test]
async fn answers_a2a_tasks() {
    let base = serve().await;
    let client = reqwest::Client::new();

    let card: Value = reqwest::get(format!("{base}/.well-known/agent-card.json"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let reply: Value = client
        .post(format!("{base}/a2a"))
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "SendMessage",
            "params": { "message": { "role": "ROLE_USER", "messageId": "m1", "parts": [ { "text": "hi" } ] } }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(card["name"], "echo");
    assert_eq!(
        reply["result"]["task"]["status"]["state"],
        "TASK_STATE_COMPLETED"
    );
    assert_eq!(
        reply["result"]["task"]["artifacts"][0]["parts"][0]["text"],
        "HI (nobody)"
    );
}

#[tokio::test]
async fn offers_the_agent_as_an_mcp_tool() {
    let base = serve().await;

    let client = McpClient::connect_http(&format!("{base}/mcp"), &[])
        .await
        .unwrap();
    let tools = client.list_tools().await.unwrap();
    let answer = client
        .call_tool("shout", json!({ "request": "psst" }))
        .await
        .unwrap();

    assert_eq!(tools[0].name, "shout");
    assert_eq!(answer, "PSST (nobody)");
}
