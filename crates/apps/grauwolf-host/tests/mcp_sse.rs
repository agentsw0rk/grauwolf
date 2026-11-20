//! A spec with an MCP server over the HTTP+SSE transport provides its tools (ADR-0112).

use std::path::Path;

use grauwolf_mcp::testing::SseTestServer;
use grauwolf_spec::{AgentSpec, AgentSpecExt};
use serde_json::json;

mod support;

fn spec(url: &str, transport: &str) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "mcp_servers": { "legacy": {
                "url": url, "transport": transport, "tools": ["reverse"]
            } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap()
}

#[tokio::test]
async fn provides_the_tools_of_a_server_with_the_sse_transport() {
    let server = SseTestServer::start().await;

    let tools = support::tools(&spec(&server.url(), "sse"), Path::new("."))
        .await
        .unwrap();

    assert_eq!(tools.len(), 1);
    let result = tools[0].call_json(json!({ "text": "wolf" })).await.unwrap();
    assert_eq!(result, "flow");
}

#[tokio::test]
async fn does_not_fall_back_to_the_sse_transport() {
    let server = SseTestServer::start().await;

    let error = support::tools(&spec(&server.url(), "streamable_http"), Path::new("."))
        .await
        .err()
        .expect("the SSE server does not speak Streamable HTTP");

    assert!(error.contains("MCP server 'legacy'"), "{error}");
}

#[tokio::test]
async fn lists_every_tool_a_server_offers() {
    let server = SseTestServer::start().await;
    let entry: grauwolf_spec::McpServerSpec = serde_json::from_value(json!({
        "url": server.url(), "transport": "sse", "tools": []
    }))
    .unwrap();

    let tools =
        grauwolf_host::mcp::list_tools("legacy", &entry, &grauwolf_host::config::MapEnv::new([]))
            .await
            .unwrap();

    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
    assert!(
        names.contains(&"echo") && names.contains(&"reverse"),
        "{names:?}"
    );
}
