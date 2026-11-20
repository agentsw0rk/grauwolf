//! Resources and prompts of the MCP server (ADR-0089).

use grauwolf_mcp::{McpPrompt, McpPromptArgument, McpResource, McpServer};
use serde_json::{Value, json};

fn server() -> McpServer {
    McpServer::new("lexicon", Vec::new())
        .with_resources(vec![McpResource {
            uri: "grauwolf://lexicon/spec".into(),
            name: "spec".into(),
            description: "The agent's spec".into(),
            mime_type: "application/json".into(),
            text: r#"{"name":"lexicon"}"#.into(),
        }])
        .with_prompts(vec![McpPrompt {
            name: "lexicon".into(),
            description: "Queries the lexicon".into(),
            arguments: vec![McpPromptArgument {
                name: "request".into(),
                description: "Auftrag".into(),
                required: true,
            }],
            template: "You are a lexicon.\n\n{request}".into(),
        }])
}

async fn call(server: &McpServer, method: &str, params: Value) -> Value {
    server
        .handle(json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .await
        .unwrap()
}

#[tokio::test]
async fn offers_resources_and_prompts() {
    let server = server();

    let init = call(
        &server,
        "initialize",
        json!({ "protocolVersion": "2025-06-18" }),
    )
    .await;
    let resources = call(&server, "resources/list", json!({})).await;
    let read = call(
        &server,
        "resources/read",
        json!({ "uri": "grauwolf://lexicon/spec" }),
    )
    .await;
    let missing = call(
        &server,
        "resources/read",
        json!({ "uri": "grauwolf://lexicon/nothing" }),
    )
    .await;
    let prompts = call(&server, "prompts/list", json!({})).await;
    let prompt = call(
        &server,
        "prompts/get",
        json!({ "name": "lexicon", "arguments": { "request": "What is a wolf?" } }),
    )
    .await;
    let incomplete = call(&server, "prompts/get", json!({ "name": "lexicon" })).await;

    let capabilities = &init["result"]["capabilities"];
    assert!(capabilities["resources"].is_object(), "{init}");
    assert!(capabilities["prompts"].is_object(), "{init}");
    assert_eq!(
        resources["result"]["resources"],
        json!([ { "uri": "grauwolf://lexicon/spec", "name": "spec",
                  "description": "The agent's spec", "mimeType": "application/json" } ])
    );
    assert_eq!(
        read["result"]["contents"],
        json!([ { "uri": "grauwolf://lexicon/spec", "mimeType": "application/json",
                  "text": r#"{"name":"lexicon"}"# } ])
    );
    assert_eq!(missing["error"]["code"], -32002, "{missing}");
    assert_eq!(
        prompts["result"]["prompts"],
        json!([ { "name": "lexicon", "description": "Queries the lexicon",
                  "arguments": [ { "name": "request", "description": "Auftrag", "required": true } ] } ])
    );
    assert_eq!(
        prompt["result"]["messages"],
        json!([ { "role": "user",
                  "content": { "type": "text", "text": "You are a lexicon.\n\nWhat is a wolf?" } } ])
    );
    assert_eq!(incomplete["error"]["code"], -32602, "{incomplete}");
}
