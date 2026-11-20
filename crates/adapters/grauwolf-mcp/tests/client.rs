//! MCP client against a fake server (F09.1, ADR-0036).

use grauwolf_mcp::McpClient;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

/// Minimal MCP server: a tool `echo` that returns `text`; `fail` returns `isError`.
async fn fake_server(stream: DuplexStream) {
    let (read, mut write) = tokio::io::split(stream);
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let request: Value = serde_json::from_str(&line).unwrap();
        let Some(id) = request.get("id").cloned() else {
            continue; // Notification, e.g. notifications/initialized
        };
        let result = match request["method"].as_str().unwrap() {
            "initialize" => json!({
                "protocolVersion": request["params"]["protocolVersion"],
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "fake", "version": "1.0" }
            }),
            "tools/list" => json!({ "tools": [ {
                "name": "echo",
                "description": "Returns the text",
                "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } }
            } ] }),
            "tools/call" => {
                let text = request["params"]["arguments"]["text"]
                    .as_str()
                    .unwrap_or_default();
                json!({
                    "content": [ { "type": "text", "text": text } ],
                    "isError": text == "fail"
                })
            }
            "resources/list" => json!({ "resources": [
                { "uri": "file:///readme.md", "name": "README", "description": "Einstieg", "mimeType": "text/markdown" },
                { "uri": "file:///logo.png", "name": "Logo", "mimeType": "image/png" }
            ] }),
            "resources/read" => match request["params"]["uri"].as_str().unwrap() {
                "file:///readme.md" => {
                    json!({ "contents": [ { "uri": "file:///readme.md", "text": "# grauwolf" } ] })
                }
                _ => {
                    json!({ "contents": [ { "uri": "file:///logo.png", "blob": "iVBORw0KGgo=" } ] })
                }
            },
            "prompts/list" => json!({ "prompts": [
                { "name": "review", "description": "Check code", "arguments": [ { "name": "language", "required": true } ] }
            ] }),
            "prompts/get" => json!({ "messages": [
                { "role": "user", "content": { "type": "text", "text": format!("Check {} code.", request["params"]["arguments"]["language"].as_str().unwrap()) } },
                { "role": "assistant", "content": { "type": "text", "text": "Gern." } }
            ] }),
            other => panic!("unexpected method {other}"),
        };
        let response = json!({ "jsonrpc": "2.0", "id": id, "result": result });
        write
            .write_all(format!("{response}\n").as_bytes())
            .await
            .unwrap();
    }
}

async fn connected() -> McpClient {
    let (client_side, server_side) = tokio::io::duplex(4096);
    tokio::spawn(fake_server(server_side));
    let (read, write) = tokio::io::split(client_side);
    McpClient::connect(read, write).await.unwrap()
}

#[tokio::test]
async fn lists_and_calls_the_tools_of_a_server() {
    let client = connected().await;

    assert_eq!(client.server_name(), "fake");
    let tools = client.list_tools().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].description, "Returns the text");
    assert_eq!(tools[0].parameters["properties"]["text"]["type"], "string");
    assert_eq!(
        client
            .call_tool("echo", json!({ "text": "Hello" }))
            .await
            .unwrap(),
        "Hello"
    );
}

#[tokio::test]
async fn reports_tool_errors_of_the_server() {
    let client = connected().await;

    let error = client
        .call_tool("echo", json!({ "text": "fail" }))
        .await
        .unwrap_err();

    assert!(error.to_string().contains("fail"), "{error}");
}

#[tokio::test]
async fn offers_its_tools_for_a_tool_registry() {
    let client = std::sync::Arc::new(connected().await);
    let tools = client.tools().await.unwrap();
    let registry = tools
        .into_iter()
        .fold(grauwolf_tools::ToolRegistry::builder(), |builder, tool| {
            builder.shared(tool)
        })
        .build();

    assert_eq!(registry.descriptors()[0].name, "echo");
    assert_eq!(
        registry
            .call("echo", json!({ "text": "over MCP" }))
            .await
            .unwrap(),
        "over MCP"
    );
    assert!(matches!(
        registry.call("echo", json!({ "text": "fail" })).await,
        Err(grauwolf_tools::ToolError::Execution(message)) if message.contains("fail")
    ));
}

#[tokio::test]
async fn lists_and_reads_resources_as_text() {
    let client = connected().await;

    let resources = client.list_resources().await.unwrap();
    let readme = client.read_resource("file:///readme.md").await.unwrap();
    let logo = client.read_resource("file:///logo.png").await.unwrap();

    assert_eq!(resources.len(), 2);
    assert_eq!(resources[0].uri, "file:///readme.md");
    assert_eq!(resources[0].name, "README");
    assert_eq!(resources[0].mime_type.as_deref(), Some("text/markdown"));
    assert_eq!(readme, "# grauwolf");
    assert_eq!(
        logo,
        "[binary content file:///logo.png, 12 Base64 characters]"
    );
}

#[tokio::test]
async fn fetches_prompts_with_arguments_as_messages() {
    let client = connected().await;

    let prompts = client.list_prompts().await.unwrap();
    let messages = client
        .get_prompt("review", json!({ "language": "Rust" }))
        .await
        .unwrap();

    assert_eq!(prompts[0].name, "review");
    assert_eq!(prompts[0].arguments, ["language"]);
    assert_eq!(
        messages,
        [
            grauwolf_prompt::Message::user("Check Rust code."),
            grauwolf_prompt::Message::assistant("Gern.")
        ]
    );
}

#[tokio::test]
async fn offers_resources_through_two_tools() {
    let client = std::sync::Arc::new(connected().await);

    let tools = client.resource_tools("docs");
    let names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
    let listed = tools[0].call_json(json!({})).await.unwrap();
    let read = tools[1]
        .call_json(json!({ "uri": "file:///readme.md" }))
        .await
        .unwrap();

    assert_eq!(names, ["docs_list_resources", "docs_read_resource"]);
    assert_eq!(
        listed,
        "file:///readme.md – README: Einstieg (text/markdown)\nfile:///logo.png – Logo (image/png)"
    );
    assert_eq!(read, "# grauwolf");
    assert_eq!(tools[1].descriptor().parameters["required"], json!(["uri"]));
}

#[tokio::test]
async fn says_so_when_a_server_has_no_resources() {
    let (client_side, server_side) = tokio::io::duplex(4096);
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (read, mut write) = tokio::io::split(server_side);
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let request: Value = serde_json::from_str(&line).unwrap();
            let Some(id) = request.get("id").cloned() else {
                continue;
            };
            let result = match request["method"].as_str().unwrap() {
                "initialize" => {
                    json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "serverInfo": { "name": "leer" } })
                }
                _ => json!({ "resources": [] }),
            };
            let response = json!({ "jsonrpc": "2.0", "id": id, "result": result });
            write
                .write_all(format!("{response}\n").as_bytes())
                .await
                .unwrap();
        }
    });
    let (read, write) = tokio::io::split(client_side);
    let client = std::sync::Arc::new(McpClient::connect(read, write).await.unwrap());

    let listed = client.resource_tools("leer")[0]
        .call_json(json!({}))
        .await
        .unwrap();

    assert_eq!(listed, "No resources.");
}

/// Timeouts (ADR-0065): a server that sends nothing over stdio for too long results in an
/// error instead of waiting forever.
#[tokio::test]
async fn gives_up_on_a_silent_server_within_the_read_timeout() {
    let (client_side, _silent_server) = tokio::io::duplex(4096);
    let (read, write) = tokio::io::split(client_side);
    let started = std::time::Instant::now();

    let message =
        McpClient::connect_with_read_timeout(read, write, std::time::Duration::from_millis(200))
            .await
            .err()
            .unwrap()
            .to_string();

    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert!(message.contains("timeout"), "{message}");
    assert!(message.contains("may have been executed"), "{message}");
}
