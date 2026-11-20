//! MCP over Streamable HTTP (ADR-0056), checked with our own MCP client.

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;
use std::sync::Arc;

use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_mcp::{McpClient, McpServer};
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use serde_json::json;

#[tokio::test]
async fn offers_the_agent_to_mcp_clients_over_http() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/specs/echo.json");
    let spec = AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
    let executor = PromptExecutor::builder()
        .client("ollama", Arc::new(MockLlmClient::new("Echo!")))
        .build();
    let agent = Arc::new(Interpreter::new(Arc::new(executor)).load(&spec).unwrap());
    let server = McpServer::new("echo", vec![agent.into_tool("echo", "Repeats")]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, grauwolf_server::mcp_router(server))
            .await
            .unwrap()
    });

    let client = McpClient::connect_http(&format!("{base}/mcp"), &[])
        .await
        .unwrap();
    let tools = client.list_tools().await.unwrap();
    let answer = client
        .call_tool("echo", json!({ "request": "Hello" }))
        .await
        .unwrap();

    assert_eq!(client.server_name(), "echo");
    assert_eq!(tools[0].name, "echo");
    assert_eq!(answer, "Echo!");
}
