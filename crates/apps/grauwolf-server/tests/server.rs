//! Agents as an HTTP service (ADR-0053) against a real server on a free port.

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_agents::memory::MemoryChatHistoryStore;
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

/// Starts the service for `spec` and returns its base URL.
async fn serve(spec: &str, mock: MockLlmClient) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/specs")
        .join(spec);
    let spec = AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
    let executor = PromptExecutor::builder()
        .client("ollama", Arc::new(mock))
        .build();
    let agent = Interpreter::new(Arc::new(executor)).load(&spec).unwrap();
    let router =
        grauwolf_server::router(Arc::new(agent), Arc::new(MemoryChatHistoryStore::default()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}")
}

#[tokio::test]
async fn runs_the_agent_and_continues_conversations() {
    let mock = MockLlmClient::new("Hello")
        .answer_when_contains("Noted.\nWhat did I say?", "You said 4711.")
        .answer_when_contains("4711", "Noted.");
    let base = serve("echo.json", mock).await;
    let client = reqwest::Client::new();
    let post = |body: Value| client.post(format!("{base}/run")).json(&body).send();

    let health = client
        .get(format!("{base}/health"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let first: Value = post(json!({ "input": "Remember 4711.", "conversation_id": "anna" }))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second: Value = post(json!({ "input": "What did I say?", "conversation_id": "anna" }))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let invalid = post(json!({ "input": 42 })).await.unwrap();
    let status = invalid.status();
    let error: Value = invalid.json().await.unwrap();

    assert_eq!(health, "ok");
    assert_eq!(first, json!({ "output": "Noted." }));
    assert_eq!(second, json!({ "output": "You said 4711." }));
    assert_eq!(status, 500);
    assert!(error["error"].as_str().is_some(), "{error}");
}

#[tokio::test]
async fn streams_events_text_and_output_as_server_sent_events() {
    let mock = MockLlmClient::new("?").answer_when_contains("Hello", "Hello world");
    let base = serve("streaming_echo.json", mock).await;

    let body = reqwest::Client::new()
        .post(format!("{base}/run/stream"))
        .json(&json!({ "input": "Hello" }))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(
        body.contains("event: agent\ndata: {\"event\":\"agent_starting\""),
        "{body}"
    );
    assert!(body.contains("event: text\ndata: Hello world"), "{body}");
    assert!(
        body.trim_end()
            .ends_with("event: output\ndata: \"Hello world\""),
        "{body}"
    );
}

#[tokio::test]
async fn requires_the_bearer_token_for_runs_but_not_for_health() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/specs/echo.json");
    let spec = AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
    let executor = PromptExecutor::builder()
        .client("ollama", Arc::new(MockLlmClient::new("Hello")))
        .build();
    let agent = Interpreter::new(Arc::new(executor)).load(&spec).unwrap();
    let router = grauwolf_server::with_token(
        grauwolf_server::router(Arc::new(agent), Arc::new(MemoryChatHistoryStore::default())),
        "secret-123",
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::new();
    let run = |token: Option<&str>| {
        let mut request = client
            .post(format!("{base}/run"))
            .json(&json!({ "input": "Hello" }));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        request.send()
    };

    let health = client
        .get(format!("{base}/health"))
        .send()
        .await
        .unwrap()
        .status();
    let missing = run(None).await.unwrap().status();
    let wrong = run(Some("wrong")).await.unwrap().status();
    let right = run(Some("secret-123")).await.unwrap();

    assert_eq!(health, 200);
    assert_eq!(missing, 401);
    assert_eq!(wrong, 401);
    assert_eq!(right.status(), 200);
    assert_eq!(
        right.json::<Value>().await.unwrap(),
        json!({ "output": "Hello" })
    );
}

/// Names the user it was built for – stands in for their memory tools.
struct WhoAmI(String);

impl DynTool for WhoAmI {
    fn name(&self) -> &str {
        "whoami"
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "whoami".into(),
            description: "Names the user".into(),
            parameters: json!({ "type": "object" }),
        }
    }

    fn call_json(&self, _args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        let user = self.0.clone();
        Box::pin(async move { Ok(user) })
    }
}

#[tokio::test]
async fn gives_each_run_the_tools_of_its_user() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "personal",
            "model": { "provider": "ollama", "id": "m" },
            "mcp_servers": { "people": { "command": "people", "tools": ["whoami"] } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains("Who am I?", "whoami", json!({}))
        .answer_when_contains("anna", "You are anna.")
        .answer_when_contains("nobody", "I do not know that.");
    let executor = PromptExecutor::builder()
        .client("ollama", Arc::new(mock))
        .build();
    let agent = Interpreter::new(Arc::new(executor))
        .with_tools(vec![Arc::new(WhoAmI("nobody".into())) as Arc<dyn DynTool>])
        .load(&spec)
        .unwrap();
    let router = grauwolf_server::router_with_user_tools(
        Arc::new(agent),
        Arc::new(MemoryChatHistoryStore::default()),
        Arc::new(|user: &str| vec![Arc::new(WhoAmI(user.to_owned())) as Arc<dyn DynTool>]),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::new();
    let run = |body: Value| {
        let response = client.post(format!("{base}/run")).json(&body).send();
        async { response.await.unwrap().json::<Value>().await.unwrap() }
    };

    let anna = run(json!({ "input": "Who am I?", "user_id": "anna" })).await;
    let anonymous = run(json!({ "input": "Who am I?" })).await;

    assert_eq!(anna, json!({ "output": "You are anna." }));
    assert_eq!(anonymous, json!({ "output": "I do not know that." }));
}

#[tokio::test]
async fn binds_conversations_to_their_user() {
    let mock = MockLlmClient::new("New here.")
        .answer_when_contains("Noted.\nWhat did I say?", "You said 4711.")
        .answer_when_contains("4711", "Noted.");
    let base = serve("echo.json", mock).await;
    let client = reqwest::Client::new();
    let post = |body: Value| client.post(format!("{base}/run")).json(&body).send();
    let output = |body: Value| async {
        post(body).await.unwrap().json::<Value>().await.unwrap()["output"].clone()
    };

    output(json!({ "input": "Remember 4711.", "conversation_id": "c1", "user_id": "anna" })).await;
    let anna =
        output(json!({ "input": "What did I say?", "conversation_id": "c1", "user_id": "anna" }))
            .await;
    let bob =
        output(json!({ "input": "What did I say?", "conversation_id": "c1", "user_id": "bob" }))
            .await;
    let anonymous = output(json!({ "input": "What did I say?", "conversation_id": "c1" })).await;
    let invalid = post(json!({ "input": "Hello", "conversation_id": "users/616e6e61/c1" }))
        .await
        .unwrap();
    let status = invalid.status();
    let error: Value = invalid.json().await.unwrap();

    assert_eq!(anna, json!("You said 4711."));
    assert_eq!(bob, json!("New here."));
    assert_eq!(anonymous, json!("New here."));
    assert_eq!(status, 400);
    assert!(
        error["error"].as_str().unwrap().contains("conversation ID"),
        "{error}"
    );
}

#[tokio::test]
async fn loads_the_run_tools_for_each_request() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "personal",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let mock = MockLlmClient::new("No tool.")
        .call_tool_when_contains("Who am I?", "whoami", json!({}))
        .answer_when_contains("berta", "You are berta.");
    let executor = PromptExecutor::builder()
        .client("ollama", Arc::new(mock))
        .build();
    let agent = Interpreter::new(Arc::new(executor)).load(&spec).unwrap();
    // The tools directory is only filled after the first request (ADR-0074).
    let available: Arc<std::sync::Mutex<Vec<Arc<dyn DynTool>>>> = Arc::default();
    let run_tools: grauwolf_server::RunTools = {
        let available = Arc::clone(&available);
        Arc::new(move || {
            let tools = available.lock().unwrap().clone();
            Box::pin(async move { tools })
        })
    };
    let router = grauwolf_server::router_with(
        Arc::new(agent),
        Arc::new(MemoryChatHistoryStore::default()),
        grauwolf_server::ServiceOptions {
            run_tools: Some(run_tools),
            ..Default::default()
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::new();
    let run = |body: Value| {
        let response = client.post(format!("{base}/run")).json(&body).send();
        async { response.await.unwrap().json::<Value>().await.unwrap() }
    };

    let before = run(json!({ "input": "Who am I?" })).await;
    available
        .lock()
        .unwrap()
        .push(Arc::new(WhoAmI("berta".into())));
    let after = run(json!({ "input": "Who am I?" })).await;

    assert_eq!(before, json!({ "output": "No tool." }));
    assert_eq!(after, json!({ "output": "You are berta." }));
}
