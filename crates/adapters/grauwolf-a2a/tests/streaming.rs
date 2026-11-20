//! A2A client with `SendStreamingMessage` (ADR-0089).

use std::sync::{Arc, Mutex};

use grauwolf_a2a::A2aClient;
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// SSE body built from JSON-RPC responses with the given results.
fn sse(results: &[Value]) -> String {
    results
        .iter()
        .map(|result| {
            format!(
                "data: {}\n\n",
                json!({ "jsonrpc": "2.0", "id": 1, "result": result })
            )
        })
        .collect()
}

/// Agent that can stream and delivers "Hello world" in two chunks.
async fn streaming_agent() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/agent-card.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "storyteller", "description": "Tells a story",
            "supportedInterfaces": [
                { "url": format!("{}/rpc", server.uri()), "protocolBinding": "JSONRPC", "protocolVersion": "1.0" }
            ],
            "version": "1", "capabilities": { "streaming": true },
            "defaultInputModes": ["text/plain"], "defaultOutputModes": ["text/plain"], "skills": []
        })))
        .mount(&server)
        .await;
    let task = json!({ "id": "t1", "contextId": "c1" });
    Mock::given(method("POST"))
        .and(path("/rpc"))
        .and(body_partial_json(json!({ "method": "SendStreamingMessage" })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse(&[
                    json!({ "task": { "id": "t1", "contextId": "c1", "status": { "state": "TASK_STATE_WORKING" } } }),
                    json!({ "artifactUpdate": { "taskId": task["id"], "contextId": task["contextId"],
                        "artifact": { "artifactId": "answer", "parts": [ { "text": "Hello " } ] }, "append": true } }),
                    json!({ "artifactUpdate": { "taskId": task["id"], "contextId": task["contextId"],
                        "artifact": { "artifactId": "answer", "parts": [ { "text": "world" } ] }, "append": true } }),
                    json!({ "statusUpdate": { "taskId": task["id"], "contextId": task["contextId"],
                        "status": { "state": "TASK_STATE_COMPLETED" } } }),
                ])),
        )
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn streams_the_text_of_a_remote_agent_piece_by_piece() {
    let server = streaming_agent().await;
    let client = Arc::new(A2aClient::connect(&server.uri(), &[]).await.unwrap());
    let pieces = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&pieces);

    let streamed = client
        .send_streaming("Tell me something.", move |text| {
            seen.lock().unwrap().push(text.to_owned())
        })
        .await
        .unwrap();
    // The tool streams because the agent card offers it.
    let tool = client.tool("storyteller", None);
    let via_tool = tool
        .call_json(json!({ "request": "Tell me something." }))
        .await
        .unwrap();

    assert_eq!(streamed, "Hello world");
    assert_eq!(*pieces.lock().unwrap(), ["Hello ", "world"]);
    assert_eq!(via_tool, "Hello world");
}
