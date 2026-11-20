//! MCP over Streamable HTTP against a fake server (ADR-0051).

use std::sync::Arc;

use grauwolf_mcp::{McpClient, PROTOCOL_VERSION};
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn rpc(id: u64, result: serde_json::Value) -> serde_json::Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

#[tokio::test]
async fn talks_to_a_server_over_streamable_http_with_a_session() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header("authorization", "Bearer geheim"))
        .and(body_partial_json(json!({ "method": "initialize" })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Mcp-Session-Id", "session-1")
                .set_body_json(rpc(
                    1,
                    json!({
                        "protocolVersion": PROTOCOL_VERSION,
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "fern", "version": "1.0" }
                    }),
                )),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(header("mcp-session-id", "session-1"))
        .and(body_partial_json(
            json!({ "method": "notifications/initialized" }),
        ))
        .respond_with(ResponseTemplate::new(202))
        .expect(1)
        .mount(&server)
        .await;
    // List as Server-Sent Events, preceded by a notification without an ID.
    let events = format!(
        "event: message\ndata: {}\n\nevent: message\ndata: {}\n\n",
        json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": {} }),
        rpc(
            2,
            json!({ "tools": [ { "name": "echo", "description": "Echo", "inputSchema": { "type": "object" } } ] })
        )
    );
    Mock::given(method("POST"))
        .and(header("mcp-session-id", "session-1"))
        .and(header("mcp-protocol-version", PROTOCOL_VERSION))
        .and(body_partial_json(json!({ "method": "tools/list" })))
        .respond_with(ResponseTemplate::new(200).set_body_raw(events, "text/event-stream"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(header("mcp-session-id", "session-1"))
        .and(body_partial_json(
            json!({ "method": "tools/call", "params": { "name": "echo" } }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(rpc(
            3,
            json!({
                "content": [ { "type": "text", "text": "Hello back" } ]
            }),
        )))
        .mount(&server)
        .await;

    let client = Arc::new(
        McpClient::connect_http(
            &format!("{}/mcp", server.uri()),
            &[("Authorization".to_owned(), "Bearer geheim".to_owned())],
        )
        .await
        .unwrap(),
    );
    let tools = client.tools().await.unwrap();
    let answer = tools[0]
        .call_json(json!({ "text": "Hello" }))
        .await
        .unwrap();

    assert_eq!(client.server_name(), "fern");
    assert_eq!(tools[0].name(), "echo");
    assert_eq!(answer, "Hello back");
}

/// Timeouts (ADR-0065): an MCP server that sends nothing for too long results in an error
/// instead of waiting forever.
#[tokio::test]
async fn gives_up_on_a_server_that_sends_nothing_within_the_read_timeout() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": {} }))
                .set_delay(std::time::Duration::from_secs(5)),
        )
        .mount(&server)
        .await;
    let started = std::time::Instant::now();

    let result = McpClient::connect_http_with_read_timeout(
        &format!("{}/mcp", server.uri()),
        &[],
        std::time::Duration::from_millis(200),
    )
    .await;

    let message = result.err().unwrap().to_string();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert!(message.contains("timeout"), "{message}");
    assert!(message.contains("may have been executed"), "{message}");
}

/// Timeouts (ADR-0065): if no connection is established, the message says nothing was run.
#[tokio::test]
async fn says_that_nothing_ran_when_the_server_is_unreachable() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);

    let message = McpClient::connect_http(&format!("http://{address}/mcp"), &[])
        .await
        .err()
        .unwrap()
        .to_string();

    assert!(message.contains("no connection"), "{message}");
    assert!(message.contains("not executed"), "{message}");
}
