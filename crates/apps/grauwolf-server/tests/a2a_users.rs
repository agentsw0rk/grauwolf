//! A2A with user tokens (ADR-0054 addendum, ADR-0068): tasks belong to the user of the token.

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;

use grauwolf_agents::memory::{ChatHistoryStore, MemoryChatHistoryStore, conversation_key};
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_server::Users;
use grauwolf_server::a2a::AgentCardInfo;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use serde_json::{Value, json};

const ANNA: &str = "gw_00112233445566778899aabbccddeeff";
const BOB: &str = "gw_ffeeddccbbaa99887766554433221100";

/// A2A service with user tokens for `anna` and `bob`; returns base URL and conversation store.
async fn serve() -> (String, Arc<MemoryChatHistoryStore>) {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "echo",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let executor = PromptExecutor::builder()
        .client("ollama", Arc::new(MockLlmClient::new("Noted.")))
        .build();
    let agent = Arc::new(Interpreter::new(Arc::new(executor)).load(&spec).unwrap());
    let history = Arc::new(MemoryChatHistoryStore::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = grauwolf_server::a2a::router(
        agent,
        Arc::clone(&history) as Arc<dyn ChatHistoryStore>,
        AgentCardInfo::from_spec(&spec, &base),
    );
    let hash = |token: &str| format!("sha256:{}", grauwolf_server::token_hash(token));
    let users =
        Users::from_json(&json!({ "anna": hash(ANNA), "bob": hash(BOB) }).to_string()).unwrap();
    let router = grauwolf_server::with_auth(router, Some("backend".into()), users);
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (base, history)
}

async fn call(base: &str, token: &str, method: &str, params: Value) -> Value {
    reqwest::Client::new()
        .post(format!("{base}/a2a"))
        .bearer_auth(token)
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn message(text: &str, context: &str) -> Value {
    json!({ "message": { "messageId": "m-1", "role": "ROLE_USER", "contextId": context,
                         "parts": [ { "text": text } ] } })
}

#[tokio::test]
async fn keeps_the_conversations_of_token_users_apart() {
    let (base, history) = serve().await;

    let anna = call(
        &base,
        ANNA,
        "SendMessage",
        message("My name is Anna.", "c1"),
    )
    .await;

    assert_eq!(anna["result"]["task"]["contextId"], "c1", "{anna}");
    let own = history
        .load(&conversation_key(Some("anna"), "c1").unwrap())
        .await
        .unwrap();
    let shared = history.load("c1").await.unwrap();
    assert!(
        !own.is_empty(),
        "Anna's conversation is missing from her area"
    );
    assert!(
        shared.is_empty(),
        "Anna's conversation is in the shared area"
    );
}

#[tokio::test]
async fn shows_and_cancels_tasks_only_for_their_owner() {
    let (base, _) = serve().await;
    let anna = call(&base, ANNA, "SendMessage", message("Hello", "c2")).await;
    let task = json!({ "id": anna["result"]["task"]["id"] });

    let own = call(&base, ANNA, "GetTask", task.clone()).await;
    let foreign = call(&base, BOB, "GetTask", task.clone()).await;
    let cancelled = call(&base, BOB, "CancelTask", task).await;

    assert_eq!(own["result"]["id"], anna["result"]["task"]["id"], "{own}");
    assert_eq!(foreign["error"]["code"], -32001, "{foreign}");
    assert_eq!(cancelled["error"]["code"], -32001, "{cancelled}");
}

#[tokio::test]
async fn gives_tasks_and_contexts_ids_with_128_random_bits() {
    let (base, _) = serve().await;

    let first = call(
        &base,
        ANNA,
        "SendMessage",
        json!({ "message": {
        "messageId": "m-1", "role": "ROLE_USER", "parts": [ { "text": "Hello" } ] } }),
    )
    .await;
    let second = call(&base, ANNA, "SendMessage", message("Hello", "c3")).await;

    let random = |id: &Value, prefix: &str| {
        let hex = id
            .as_str()
            .unwrap()
            .strip_prefix(prefix)
            .unwrap()
            .to_owned();
        assert_eq!(hex.len(), 32, "{id}");
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
        hex
    };
    let one = random(&first["result"]["task"]["id"], "task-");
    let two = random(&second["result"]["task"]["id"], "task-");
    random(&first["result"]["task"]["contextId"], "context-");
    assert_ne!(one, two);
}
