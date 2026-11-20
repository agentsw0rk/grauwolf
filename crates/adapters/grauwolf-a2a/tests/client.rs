//! A2A client against a fake agent (ADR-0055).

use grauwolf_a2a::A2aClient;
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Fake agent with a card; `SendMessage` answers `answer` for every message.
async fn agent(answer: Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/agent-card.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "wetter",
            "description": "Knows the weather",
            "supportedInterfaces": [
                { "url": format!("{}/grpc", server.uri()), "protocolBinding": "GRPC", "protocolVersion": "1.0" },
                { "url": format!("{}/rpc", server.uri()), "protocolBinding": "JSONRPC", "protocolVersion": "1.0" }
            ],
            "version": "1", "capabilities": {}, "defaultInputModes": ["text/plain"],
            "defaultOutputModes": ["text/plain"], "skills": []
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/rpc"))
        .and(header("a2a-version", "1.0"))
        .and(header("authorization", "Bearer geheim"))
        .and(body_partial_json(
            json!({ "method": "SendMessage", "params": { "message": {
            "role": "ROLE_USER", "parts": [ { "text": "What is the weather like?" } ] } } }),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "jsonrpc": "2.0", "id": 1, "result": answer })),
        )
        .mount(&server)
        .await;
    server
}

async fn ask(answer: Value) -> Result<String, grauwolf_a2a::A2aError> {
    let server = agent(answer).await;
    let client = A2aClient::connect(
        &server.uri(),
        &[("Authorization".into(), "Bearer geheim".into())],
    )
    .await
    .unwrap();
    assert_eq!(client.name(), "wetter");
    assert_eq!(client.description(), "Knows the weather");
    client.send("What is the weather like?").await
}

#[tokio::test]
async fn sends_a_message_to_the_jsonrpc_interface_and_reads_the_task_artifacts() {
    let answer = ask(json!({ "task": {
        "id": "t-1", "contextId": "c-1",
        "status": { "state": "TASK_STATE_COMPLETED" },
        "artifacts": [ { "artifactId": "a", "parts": [ { "text": "Sonnig" }, { "data": { "grad": 21 } } ] } ]
    } }))
    .await
    .unwrap();

    assert_eq!(answer, "Sonnig\n{\"grad\":21}");
}

#[tokio::test]
async fn returns_follow_up_questions_and_reports_failed_tasks() {
    let question = ask(json!({ "task": {
        "id": "t-2", "contextId": "c-2",
        "status": { "state": "TASK_STATE_INPUT_REQUIRED", "message": {
            "messageId": "m", "role": "ROLE_AGENT", "parts": [ { "text": "For which city?" } ] } }
    } }))
    .await
    .unwrap();
    let direct = ask(json!({ "message": { "messageId": "m", "role": "ROLE_AGENT", "parts": [ { "text": "Rain" } ] } }))
        .await
        .unwrap();
    let failed = ask(json!({ "task": {
        "id": "t-3", "contextId": "c-3",
        "status": { "state": "TASK_STATE_FAILED", "message": {
            "messageId": "m", "role": "ROLE_AGENT", "parts": [ { "text": "Service unreachable" } ] } }
    } }))
    .await
    .unwrap_err();

    assert_eq!(question, "Question from the agent: For which city?");
    assert_eq!(direct, "Rain");
    assert!(
        failed
            .to_string()
            .contains("TASK_STATE_FAILED: Service unreachable"),
        "{failed}"
    );
}

/// Timeouts (ADR-0065): an A2A agent that sends nothing for too long results in an error
/// instead of waiting forever.
#[tokio::test]
async fn gives_up_on_an_agent_that_sends_nothing_within_the_read_timeout() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "name": "langsam" }))
                .set_delay(std::time::Duration::from_secs(5)),
        )
        .mount(&server)
        .await;
    let started = std::time::Instant::now();

    let result = grauwolf_a2a::A2aClient::connect_with_read_timeout(
        &server.uri(),
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
async fn says_that_nothing_ran_when_the_agent_is_unreachable() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);

    let message = A2aClient::connect(&format!("http://{address}"), &[])
        .await
        .err()
        .unwrap()
        .to_string();

    assert!(message.contains("no connection"), "{message}");
    assert!(message.contains("not executed"), "{message}");
}
