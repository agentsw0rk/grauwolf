//! A2A and MCP get the same handles as `/run` (ADR-0089): the user's tools from the
//! token, tools from the tools directory.

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_agents::memory::MemoryChatHistoryStore;
use grauwolf_interpreter::{InterpretedAgent, Interpreter};
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_server::{ServiceOptions, Users};
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

const ANNA: &str = "gw_00112233445566778899aabbccddeeff";

/// Tool with a fixed name and a fixed answer.
struct Fixed(&'static str, String);

impl DynTool for Fixed {
    fn name(&self) -> &str {
        self.0
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.0.into(),
            description: format!("Tool {}", self.0),
            parameters: json!({ "type": "object" }),
        }
    }

    fn call_json(&self, _args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        let answer = self.1.clone();
        Box::pin(async move { Ok(answer) })
    }
}

/// Agent that calls `whoami` (declared) and `weather` (from the tools directory).
fn agent() -> (AgentSpec, Arc<InterpretedAgent>) {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "personal",
            "model": { "provider": "ollama", "id": "m" },
            "mcp_servers": { "people": { "command": "people", "tools": ["whoami"] } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains("Who am I?", "whoami", json!({}))
        .call_tool_when_contains("Weather?", "weather", json!({}))
        .answer_when_contains("anna", "You are anna.")
        .answer_when_contains("Sun", "The sun is shining.");
    let executor = PromptExecutor::builder()
        .client("ollama", Arc::new(mock))
        .build();
    let agent = Interpreter::new(Arc::new(executor))
        .with_tools(vec![
            Arc::new(Fixed("whoami", "nobody".into())) as Arc<dyn DynTool>
        ])
        .load(&spec)
        .unwrap();
    (spec, Arc::new(agent))
}

fn options() -> ServiceOptions {
    ServiceOptions {
        user_tools: Some(Arc::new(|user: &str| {
            vec![Arc::new(Fixed("whoami", user.to_owned())) as Arc<dyn DynTool>]
        })),
        run_tools: Some(Arc::new(|| {
            Box::pin(async { vec![Arc::new(Fixed("weather", "Sun".into())) as Arc<dyn DynTool>] })
        })),
        ..ServiceOptions::default()
    }
}

/// Starts `router` with a user token for `anna`; returns the base URL.
async fn serve(router: axum::Router) -> String {
    let hash = format!("sha256:{}", grauwolf_server::token_hash(ANNA));
    let users = Users::from_json(&json!({ "anna": hash }).to_string()).unwrap();
    let router = grauwolf_server::with_auth(router, None, users);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    base
}

async fn rpc(url: String, method: &str, params: Value) -> Value {
    reqwest::Client::new()
        .post(url)
        .bearer_auth(ANNA)
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn a2a_runs_with_the_tools_of_the_user_and_the_tool_directory() {
    let (spec, agent) = agent();
    let base = serve(grauwolf_server::a2a::router_with(
        agent,
        Arc::new(MemoryChatHistoryStore::default()),
        grauwolf_server::a2a::AgentCardInfo::from_spec(&spec, "http://localhost"),
        options(),
    ))
    .await;
    let ask = |text: &str| {
        rpc(
            format!("{base}/a2a"),
            "SendMessage",
            json!({ "message": { "messageId": "m-1", "role": "ROLE_USER", "parts": [ { "text": text } ] } }),
        )
    };

    let who = ask("Who am I?").await;
    let weather = ask("Weather?").await;

    let text =
        |response: &Value| response["result"]["task"]["artifacts"][0]["parts"][0]["text"].clone();
    assert_eq!(text(&who), "You are anna.", "{who}");
    assert_eq!(text(&weather), "The sun is shining.", "{weather}");
}

#[tokio::test]
async fn mcp_runs_with_the_tools_of_the_user_and_the_tool_directory() {
    let (spec, agent) = agent();
    let base = serve(grauwolf_server::mcp_router_with(
        grauwolf_mcp::McpServer::new(spec.name.clone(), Vec::new()),
        agent,
        "Personal assistant".into(),
        options(),
    ))
    .await;
    let call = |request: &str| {
        rpc(
            format!("{base}/mcp"),
            "tools/call",
            json!({ "name": "personal", "arguments": { "request": request } }),
        )
    };

    let tools = rpc(format!("{base}/mcp"), "tools/list", json!({})).await;
    let who = call("Who am I?").await;
    let weather = call("Weather?").await;

    assert_eq!(tools["result"]["tools"][0]["name"], "personal", "{tools}");
    let text = |response: &Value| response["result"]["content"][0]["text"].clone();
    assert_eq!(text(&who), "You are anna.", "{who}");
    assert_eq!(text(&weather), "The sun is shining.", "{weather}");
}
