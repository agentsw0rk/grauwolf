//! MCP server for arbitrary tools (ADR-0056), checked with our own client.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_mcp::{McpClient, McpError, McpServer};
use grauwolf_prompt::ToolDescriptor;
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

/// Upper-cases text; "fail" fails.
struct Shout;

impl DynTool for Shout {
    fn name(&self) -> &str {
        "shout"
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "shout".into(),
            description: "Upper-cases text".into(),
            parameters: json!({ "type": "object", "properties": { "text": { "type": "string" } } }),
        }
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        let text = args["text"].as_str().unwrap_or_default().to_owned();
        Box::pin(async move {
            if text == "fail" {
                Err(ToolError::Execution("deliberately".into()))
            } else {
                Ok(text.to_uppercase())
            }
        })
    }
}

async fn connected() -> McpClient {
    let (client_side, server_side) = tokio::io::duplex(4096);
    let server = McpServer::new("grauwolf-test", vec![Arc::new(Shout) as Arc<dyn DynTool>]);
    tokio::spawn(async move {
        let (read, write) = tokio::io::split(server_side);
        server.serve(read, write).await.unwrap();
    });
    let (read, write) = tokio::io::split(client_side);
    McpClient::connect(read, write).await.unwrap()
}

#[tokio::test]
async fn offers_its_tools_to_mcp_clients() {
    let client = connected().await;

    let tools = client.list_tools().await.unwrap();
    let answer = client
        .call_tool("shout", json!({ "text": "hallo" }))
        .await
        .unwrap();
    let failure = client
        .call_tool("shout", json!({ "text": "fail" }))
        .await
        .unwrap_err();
    let unknown = client.call_tool("whisper", json!({})).await.unwrap_err();

    assert_eq!(client.server_name(), "grauwolf-test");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "shout");
    assert_eq!(tools[0].parameters["properties"]["text"]["type"], "string");
    assert_eq!(answer, "HALLO");
    assert!(
        matches!(&failure, McpError::Tool(text) if text.contains("deliberately")),
        "{failure}"
    );
    assert!(
        matches!(&unknown, McpError::Tool(text) if text.contains("whisper")),
        "{unknown}"
    );
}

#[tokio::test]
async fn answers_ping_and_rejects_unknown_methods() {
    let server = McpServer::new("grauwolf-test", Vec::new());

    let ping = server
        .handle(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .await;
    let unknown = server
        .handle(json!({ "jsonrpc": "2.0", "id": 2, "method": "completion/complete" }))
        .await;
    let notification = server
        .handle(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    let old = server
        .handle(json!({ "jsonrpc": "2.0", "id": 3, "method": "initialize", "params": { "protocolVersion": "2024-11-05" } }))
        .await;

    assert_eq!(
        ping,
        Some(json!({ "jsonrpc": "2.0", "id": 1, "result": {} }))
    );
    assert_eq!(unknown.unwrap()["error"]["code"], -32601);
    assert_eq!(notification, None);
    assert_eq!(old.unwrap()["result"]["protocolVersion"], "2024-11-05");
}
