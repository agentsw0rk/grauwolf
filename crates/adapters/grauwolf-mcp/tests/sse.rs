//! MCP over the HTTP+SSE transport of protocol 2024-11-05 (ADR-0112) against the in-memory
//! test server.

use grauwolf_mcp::McpClient;
use grauwolf_mcp::McpError;
use grauwolf_mcp::testing::{SseBehavior, SseTestServer};
use serde_json::json;

async fn connect(server: &SseTestServer) -> McpClient {
    McpClient::connect_sse(&server.url(), &[]).await.unwrap()
}

#[tokio::test]
async fn performs_the_handshake_and_lists_tools_across_pages() {
    let server = SseTestServer::start().await;

    let client = connect(&server).await;
    let tools = client.list_tools().await.unwrap();

    assert_eq!(client.server_name(), "sse-test");
    let names: Vec<_> = tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, ["echo", "reverse", "fail", "ask_back", "hang"]);
    let methods: Vec<_> = server
        .posts()
        .iter()
        .map(|post| post["method"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "notifications/initialized",
            "tools/list",
            "tools/list"
        ]
    );
    assert_eq!(
        server.posts()[0]["params"]["protocolVersion"],
        grauwolf_mcp::PROTOCOL_VERSION
    );
}

#[tokio::test]
async fn calls_tools_and_reports_error_results() {
    let server = SseTestServer::start().await;
    let client = connect(&server).await;

    let reversed = client.call_tool("reverse", json!({ "text": "abc" })).await;
    let failed = client.call_tool("fail", json!({})).await;

    assert_eq!(reversed.unwrap(), "cba");
    assert!(matches!(failed, Err(McpError::Tool(text)) if text == "boom"));
}

#[tokio::test]
async fn ignores_notifications_and_answers_to_unknown_calls() {
    let server = SseTestServer::start().await;
    let client = connect(&server).await;

    let echoed = client.call_tool("echo", json!({ "text": "hi" })).await;

    assert_eq!(echoed.unwrap(), "echo: hi");
}

#[tokio::test]
async fn reads_events_with_crlf_and_data_over_several_lines() {
    let server = SseTestServer::start_with(SseBehavior {
        crlf: true,
        multi_line: true,
        ..SseBehavior::default()
    })
    .await;
    let client = connect(&server).await;

    let reversed = client
        .call_tool("reverse", json!({ "text": "line one\nline two" }))
        .await;

    assert_eq!(reversed.unwrap(), "owt enil\neno enil");
}

#[tokio::test]
async fn sends_the_headers_with_the_stream_request_and_every_message() {
    let server = SseTestServer::start_with(SseBehavior {
        require_header: Some(("Authorization".into(), "Bearer secret".into())),
        ..SseBehavior::default()
    })
    .await;
    let headers = [("Authorization".to_owned(), "Bearer secret".to_owned())];

    let client = McpClient::connect_sse(&server.url(), &headers)
        .await
        .unwrap();
    let reversed = client.call_tool("reverse", json!({ "text": "ab" })).await;
    let refused = McpClient::connect_sse(&server.url(), &[]).await;

    assert_eq!(reversed.unwrap(), "ba");
    let error = refused
        .err()
        .expect("without the header the server answers 401");
    assert!(error.to_string().contains("401"), "{error}");
    assert!(!error.to_string().contains("secret-body"), "{error}");
}

#[tokio::test]
async fn names_the_status_of_a_rejected_message_but_not_its_body() {
    let server = SseTestServer::start_with(SseBehavior {
        post_status: Some(500),
        ..SseBehavior::default()
    })
    .await;

    let error = McpClient::connect_sse(&server.url(), &[])
        .await
        .err()
        .expect("the handshake is rejected");

    assert!(error.to_string().contains("500"), "{error}");
    assert!(!error.to_string().contains("secret-body"), "{error}");
}

#[tokio::test]
async fn refuses_an_endpoint_on_another_origin_and_sends_nothing_there() {
    let server = SseTestServer::start_with(SseBehavior {
        endpoint: Some("http://127.0.0.1:1/messages".into()),
        ..SseBehavior::default()
    })
    .await;
    let headers = [("Authorization".to_owned(), "Bearer secret".to_owned())];

    let error = McpClient::connect_sse(&server.url(), &headers)
        .await
        .err()
        .expect("the endpoint is refused");

    assert!(error.to_string().contains("another origin"), "{error}");
    assert!(!error.to_string().contains("secret"), "{error}");
    assert!(server.posts().is_empty());
}

#[tokio::test]
async fn fails_when_the_stream_ends_before_the_endpoint_event() {
    let server = SseTestServer::start_with(SseBehavior {
        no_endpoint: true,
        ..SseBehavior::default()
    })
    .await;

    let error = McpClient::connect_sse(&server.url(), &[])
        .await
        .err()
        .expect("no endpoint");

    assert!(error.to_string().contains("endpoint"), "{error}");
}

#[tokio::test]
async fn answers_requests_of_the_server_with_method_not_found() {
    let server = SseTestServer::start().await;
    let client = connect(&server).await;

    // The tool sends a `roots/list` request to the client and reports what it answered.
    let answer = client.call_tool("ask_back", json!({})).await;

    assert_eq!(answer.unwrap(), "client answered with error -32601");
}

/// Gives the client time to notice that the server ended the stream.
async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

#[tokio::test]
async fn reconnects_once_with_a_new_handshake_on_the_next_call_after_the_stream_was_lost() {
    let server = SseTestServer::start().await;
    let client = connect(&server).await;
    assert_eq!(
        client
            .call_tool("reverse", json!({ "text": "ab" }))
            .await
            .unwrap(),
        "ba"
    );

    server.drop_streams();
    settle().await;
    let again = client.call_tool("reverse", json!({ "text": "cd" })).await;

    assert_eq!(again.unwrap(), "dc");
    assert_eq!(server.connections(), 2);
    let handshakes = server
        .posts()
        .iter()
        .filter(|post| post["method"] == "initialize")
        .count();
    assert_eq!(handshakes, 2);
}

#[tokio::test]
async fn fails_pending_calls_when_the_stream_is_lost() {
    let server = SseTestServer::start().await;
    let client = std::sync::Arc::new(connect(&server).await);
    let waiting = tokio::spawn({
        let client = std::sync::Arc::clone(&client);
        async move { client.call_tool("hang", json!({})).await }
    });
    settle().await;

    server.drop_streams();
    let error = waiting
        .await
        .unwrap()
        .expect_err("the call cannot be answered");

    assert!(error.to_string().contains("stream"), "{error}");
    assert!(
        error.to_string().contains("may have been executed"),
        "{error}"
    );
}

#[tokio::test]
async fn gives_up_when_the_reconnect_fails() {
    let server = SseTestServer::start().await;
    let client = connect(&server).await;
    server.drop_streams();
    settle().await;
    drop(server);
    settle().await;

    let error = client
        .call_tool("reverse", json!({ "text": "ab" }))
        .await
        .expect_err("the server is gone");

    assert!(error.to_string().contains("no connection"), "{error}");
}

#[tokio::test]
async fn times_out_when_the_server_does_not_answer() {
    let server = SseTestServer::start().await;
    let client = McpClient::connect_sse_with_read_timeout(
        &server.url(),
        &[],
        std::time::Duration::from_millis(300),
    )
    .await
    .unwrap();

    let error = client
        .call_tool("hang", json!({}))
        .await
        .expect_err("no answer");

    assert!(matches!(error, McpError::Timeout { .. }), "{error}");
    assert!(
        error.to_string().contains("may have been executed"),
        "{error}"
    );
}
