//! The Txn-Token of a run acting for a person (ADR-0111) goes with every message to an MCP server
//! with a workload identity, over Streamable HTTP and over HTTP+SSE – and never to one without.

use std::sync::Arc;
use std::time::Duration;

use grauwolf_mcp::testing::SseTestServer;
use grauwolf_mcp::{McpClient, PROTOCOL_VERSION};
use grauwolf_spiffe::testing::FakeIdentity;
use grauwolf_spiffe::{AuthMode, ClientAuth};
use serde_json::json;
use wiremock::matchers::{body_partial_json, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CLIENT: &str = "spiffe://agentswork.de/ns/coordinator/sa/coordinator";

/// A run acting for a person, with a fixed Txn-Token.
struct Person;

impl grauwolf_tools::Delegation for Person {
    fn txn_token(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
    {
        Box::pin(async { Ok("txn-alice".to_owned()) })
    }
}

fn person() -> Option<Arc<dyn grauwolf_tools::Delegation>> {
    Some(Arc::new(Person))
}

fn auth() -> Option<Arc<ClientAuth>> {
    let auth = ClientAuth::new(
        Arc::new(FakeIdentity::new(CLIENT)),
        AuthMode::Jwt,
        None,
        Some("tickets".into()),
    )
    .unwrap();
    Some(Arc::new(auth))
}

fn rpc(id: u64, result: serde_json::Value) -> serde_json::Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

async fn http_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({ "method": "initialize" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(rpc(
            1,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "tickets", "version": "1.0" }
            }),
        )))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            json!({ "method": "notifications/initialized" }),
        ))
        .respond_with(ResponseTemplate::new(202))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({ "method": "tools/call" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(rpc(
            2,
            json!({ "content": [ { "type": "text", "text": "ok" } ] }),
        )))
        .mount(&server)
        .await;
    server
}

/// The `Txn-Token` header of every `tools/call` the server received.
async fn calls(server: &MockServer) -> Vec<Option<String>> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| String::from_utf8_lossy(&request.body).contains("tools/call"))
        .map(|request| {
            request
                .headers
                .get("txn-token")
                .map(|value| value.to_str().unwrap().to_owned())
        })
        .collect()
}

#[tokio::test]
async fn streamable_http_sends_the_txn_token_only_with_a_workload_identity() {
    for (auth, expected) in [(auth(), Some("txn-alice".to_owned())), (None, None)] {
        let server = http_server().await;
        let client = McpClient::connect_http_with_auth(
            &format!("{}/mcp", server.uri()),
            &[],
            Duration::from_secs(5),
            auth,
        )
        .await
        .unwrap();
        grauwolf_tools::with_delegation(person(), client.call_tool("echo", json!({})))
            .await
            .unwrap();
        assert_eq!(calls(&server).await, [expected]);
    }
}

#[tokio::test]
async fn http_sse_sends_the_txn_token_only_with_a_workload_identity() {
    for (auth, expected) in [(auth(), Some("txn-alice".to_owned())), (None, None)] {
        let server = SseTestServer::start().await;
        let client =
            McpClient::connect_sse_with_auth(&server.url(), &[], Duration::from_secs(5), auth)
                .await
                .unwrap();
        grauwolf_tools::with_delegation(
            person(),
            client.call_tool("echo", json!({ "text": "hi" })),
        )
        .await
        .unwrap();
        let methods: Vec<_> = server
            .posts()
            .iter()
            .map(|post| post["method"].as_str().unwrap_or_default().to_owned())
            .collect();
        let headers = server.post_header("txn-token");
        let call = methods.iter().position(|m| m == "tools/call").unwrap();
        assert_eq!(headers[call], expected);
        assert_eq!(headers[0], None, "the handshake ran outside the run");
    }
}
