//! A2A 1.0 over JSON-RPC (ADR-0054) against a real server on a free port.

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;
use std::sync::Arc;

use grauwolf_agents::memory::MemoryChatHistoryStore;
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_server::a2a::AgentCardInfo;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use serde_json::{Value, json};

async fn serve(spec_name: &str, mock: MockLlmClient) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/specs")
        .join(spec_name);
    let spec = AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
    let executor = PromptExecutor::builder()
        .client("ollama", Arc::new(mock))
        .build();
    let agent = Arc::new(Interpreter::new(Arc::new(executor)).load(&spec).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = grauwolf_server::a2a::router(
        agent,
        Arc::new(MemoryChatHistoryStore::default()),
        AgentCardInfo::from_spec(&spec, &base),
    );
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    base
}

async fn call(base: &str, method: &str, params: Value) -> Value {
    reqwest::Client::new()
        .post(format!("{base}/a2a"))
        .header("A2A-Version", "1.0")
        .json(&json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn message(text: &str, context: Option<&str>) -> Value {
    let mut message =
        json!({ "messageId": "m-1", "role": "ROLE_USER", "parts": [ { "text": text } ] });
    if let Some(context) = context {
        message["contextId"] = json!(context);
    }
    json!({ "message": message })
}

#[tokio::test]
async fn describes_the_agent_in_its_agent_card() {
    let base = serve("translator.json", MockLlmClient::new("?")).await;

    let card: Value = reqwest::get(format!("{base}/.well-known/agent-card.json"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(card["name"], "translator");
    assert!(
        card["description"]
            .as_str()
            .unwrap()
            .contains("into English")
    );
    assert_eq!(
        card["supportedInterfaces"],
        json!([ { "url": format!("{base}/a2a"), "protocolBinding": "JSONRPC", "protocolVersion": "1.0" } ])
    );
    assert_eq!(card["capabilities"]["streaming"], true);
    assert_eq!(card["defaultInputModes"], json!(["text/plain"]));
    assert_eq!(card["skills"][0]["id"], "translator");
}

#[tokio::test]
async fn answers_messages_as_completed_tasks_and_continues_their_context() {
    let mock = MockLlmClient::new("?")
        .answer_when_contains("Noted.\nWhat did I say?", "You said 4711.")
        .answer_when_contains("4711", "Noted.");
    let base = serve("echo.json", mock).await;

    let first = call(&base, "SendMessage", message("Remember 4711.", None)).await;
    let task = &first["result"]["task"];
    let context = task["contextId"].as_str().unwrap().to_owned();
    let second = call(
        &base,
        "SendMessage",
        message("What did I say?", Some(&context)),
    )
    .await;
    let fetched = call(&base, "GetTask", json!({ "id": task["id"] })).await;

    assert_eq!(first["id"], 7);
    assert_eq!(task["status"]["state"], "TASK_STATE_COMPLETED");
    assert_eq!(
        task["artifacts"][0]["parts"],
        json!([ { "text": "Noted." } ])
    );
    assert_eq!(task["history"][1]["role"], "ROLE_AGENT");
    assert_eq!(
        second["result"]["task"]["artifacts"][0]["parts"][0]["text"],
        "You said 4711."
    );
    assert_eq!(second["result"]["task"]["contextId"], context);
    assert_eq!(fetched["result"], *task);
}

#[tokio::test]
async fn reports_protocol_errors_with_their_codes() {
    let base = serve("echo.json", MockLlmClient::new("Hello")).await;
    let done = call(&base, "SendMessage", message("Hello", None)).await;

    let unknown_task = call(&base, "GetTask", json!({ "id": "does-not-exist" })).await;
    let not_cancelable = call(
        &base,
        "CancelTask",
        json!({ "id": done["result"]["task"]["id"] }),
    )
    .await;
    let unknown_method = call(&base, "message/send", json!({})).await;
    let unsupported = call(&base, "ListTasks", json!({})).await;
    let old_version: Value = reqwest::Client::new()
        .post(format!("{base}/a2a"))
        .header("A2A-Version", "0.3")
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "GetTask", "params": { "id": "x" } }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(unknown_task["error"]["code"], -32001);
    assert_eq!(not_cancelable["error"]["code"], -32002);
    assert_eq!(unknown_method["error"]["code"], -32601);
    assert_eq!(unsupported["error"]["code"], -32004);
    assert_eq!(old_version["error"]["code"], -32009);
}

#[tokio::test]
async fn streams_a_working_task_text_artifacts_and_the_final_status() {
    let mock = MockLlmClient::new("?").answer_when_contains("Hello", "Hello world");
    let base = serve("streaming_echo.json", mock).await;

    let body = reqwest::Client::new()
        .post(format!("{base}/a2a"))
        .header("A2A-Version", "1.0")
        .json(&json!({ "jsonrpc": "2.0", "id": 3, "method": "SendStreamingMessage", "params": message("Hello", None) }))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let events: Vec<Value> = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();

    assert_eq!(
        events.first().unwrap()["result"]["task"]["status"]["state"],
        "TASK_STATE_WORKING"
    );
    let text: String = events
        .iter()
        .filter_map(|event| {
            event["result"]["artifactUpdate"]["artifact"]["parts"][0]["text"].as_str()
        })
        .collect();
    assert_eq!(text, "Hello world");
    let last = &events.last().unwrap()["result"]["statusUpdate"];
    assert_eq!(last["status"]["state"], "TASK_STATE_COMPLETED");
    assert!(
        events
            .iter()
            .all(|event| event["id"] == 3 && event["jsonrpc"] == "2.0")
    );
}

#[tokio::test]
async fn streams_the_whole_answer_as_one_artifact_when_nothing_was_streamed() {
    let base = serve("echo.json", MockLlmClient::new("Whole answer")).await;

    let body = reqwest::Client::new()
        .post(format!("{base}/a2a"))
        .json(&json!({ "jsonrpc": "2.0", "id": 4, "method": "SendStreamingMessage", "params": message("Hello", None) }))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let events: Vec<Value> = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();

    let artifacts: Vec<&Value> = events
        .iter()
        .filter_map(|event| event["result"].get("artifactUpdate"))
        .collect();
    assert_eq!(artifacts.len(), 1, "{body}");
    assert_eq!(
        artifacts[0]["artifact"]["parts"],
        json!([ { "text": "Whole answer" } ])
    );
    assert_eq!(artifacts[0]["lastChunk"], true);
    assert!(
        events.last().unwrap()["result"]
            .get("statusUpdate")
            .is_some()
    );
}

#[tokio::test]
async fn rejects_context_ids_that_could_reach_conversations_of_users() {
    let base = serve("echo.json", MockLlmClient::new("Hello")).await;

    let scoped = call(
        &base,
        "SendMessage",
        message("Hello", Some("users/616e6e61/c1")),
    )
    .await;
    let streamed = call(
        &base,
        "SendStreamingMessage",
        message("Hello", Some("../c1")),
    )
    .await;
    let plain = call(&base, "SendMessage", message("Hello", Some("c1"))).await;

    assert_eq!(scoped["error"]["code"], -32602, "{scoped}");
    assert!(
        scoped["error"]["message"]
            .as_str()
            .unwrap()
            .contains("conversation ID"),
        "{scoped}"
    );
    assert_eq!(streamed["error"]["code"], -32602, "{streamed}");
    assert_eq!(plain["result"]["task"]["contextId"], "c1");
}
