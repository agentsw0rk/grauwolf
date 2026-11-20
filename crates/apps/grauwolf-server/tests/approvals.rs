//! Approval of tool calls over `/run/stream` and `POST /approvals/{id}` (ADR-0061).

use grauwolf_spec::AgentSpecExt;
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

/// Writes text in uppercase; requires approval.
struct Upper;

impl DynTool for Upper {
    fn name(&self) -> &str {
        "upper"
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "upper".into(),
            description: "Writes text in uppercase".into(),
            parameters: json!({ "type": "object" }),
        }
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        let text = args["text"].as_str().unwrap_or_default().to_uppercase();
        Box::pin(async move { Ok(text) })
    }
}

/// Starts a service whose agent calls the approval-requiring `upper` for "loudly".
async fn serve() -> String {
    serve_wrapped(|router| router).await
}

/// Like [`serve`]; `wrap` e.g. puts an access check around the router.
async fn serve_wrapped(wrap: impl FnOnce(axum::Router) -> axum::Router) -> String {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "shouter",
            "model": { "provider": "mock", "id": "m" },
            "mcp_servers": { "text": { "command": "text-server", "tools": ["upper"] } },
            "graph": {
                "nodes": [ { "id": "call_llm", "kind": "llm_request" },
                           { "id": "execute_tool", "kind": "execute_tool" },
                           { "id": "send_result", "kind": "llm_send_tool_result" } ],
                "edges": [ { "from": "start", "to": "call_llm" },
                           { "from": "call_llm", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
                           { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } },
                           { "from": "execute_tool", "to": "send_result" },
                           { "from": "send_result", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
            }
        })
        .to_string(),
    )
    .unwrap();
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains("loudly", "upper", json!({ "text": "hello" }))
        .answer_when_contains("HELLO", "Executed")
        .answer_when_contains("approval", "Refused");
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(mock))
            .build(),
    ))
    .with_tools(vec![Arc::new(Upper) as Arc<dyn DynTool>])
    .with_tool_approval(["upper"])
    .load(&spec)
    .unwrap();
    let router = wrap(grauwolf_server::router(
        Arc::new(agent),
        Arc::new(MemoryChatHistoryStore::default()),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}")
}

/// Reads SSE blocks until one with `event: <name>` arrives, and returns its data.
async fn next_event(response: &mut reqwest::Response, buffer: &mut String, name: &str) -> Value {
    let marker = format!("event: {name}\n");
    loop {
        if let Some(start) = buffer.find(&marker)
            && let Some(end) = buffer[start..].find("\n\n")
        {
            let block = buffer[start..start + end].to_owned();
            buffer.replace_range(..start + end + 2, "");
            let data = block
                .lines()
                .filter_map(|line| line.strip_prefix("data: "))
                .collect::<Vec<_>>()
                .join("\n");
            return serde_json::from_str(&data).unwrap();
        }
        let chunk = response.chunk().await.unwrap().expect("stream ended early");
        buffer.push_str(&String::from_utf8_lossy(&chunk));
    }
}

#[tokio::test]
async fn asks_for_approval_in_the_stream_and_runs_the_tool_once_approved() {
    let base = serve().await;
    let client = reqwest::Client::new();
    let mut stream = client
        .post(format!("{base}/run/stream"))
        .json(&json!({ "input": "Say it loudly" }))
        .send()
        .await
        .unwrap();
    let mut buffer = String::new();

    let approval = next_event(&mut stream, &mut buffer, "approval").await;
    let id = approval["id"].as_str().unwrap();
    let decided = client
        .post(format!("{base}/approvals/{id}"))
        .json(&json!({ "approved": true }))
        .send()
        .await
        .unwrap();
    let output = next_event(&mut stream, &mut buffer, "output").await;

    assert_eq!(approval["tool"], "upper");
    assert_eq!(approval["args"], json!({ "text": "hello" }));
    assert_eq!(id.len(), 32, "{id}");
    assert_eq!(decided.status(), 204);
    assert_eq!(output, json!("Executed"));
}

#[tokio::test]
async fn accepts_decisions_only_from_the_user_of_the_run() {
    let base = serve().await;
    let client = reqwest::Client::new();
    let mut stream = client
        .post(format!("{base}/run/stream"))
        .json(&json!({ "input": "Say it loudly", "user_id": "anna" }))
        .send()
        .await
        .unwrap();
    let mut buffer = String::new();
    let decide = |id: String, body: Value| {
        client
            .post(format!("{base}/approvals/{id}"))
            .json(&body)
            .send()
    };

    let approval = next_event(&mut stream, &mut buffer, "approval").await;
    let id = approval["id"].as_str().unwrap().to_owned();
    let by_bob = decide(id.clone(), json!({ "approved": true, "user_id": "bob" }))
        .await
        .unwrap();
    let anonymous = decide(id.clone(), json!({ "approved": true }))
        .await
        .unwrap();
    let by_anna = decide(
        id.clone(),
        json!({ "approved": false, "reason": "approval refused", "user_id": "anna" }),
    )
    .await
    .unwrap();
    let again = decide(id, json!({ "approved": true, "user_id": "anna" }))
        .await
        .unwrap();
    let output = next_event(&mut stream, &mut buffer, "output").await;

    assert_eq!(by_bob.status(), 404);
    assert_eq!(anonymous.status(), 404);
    assert_eq!(by_anna.status(), 204);
    assert_eq!(again.status(), 404);
    assert_eq!(output, json!("Refused"));
}

/// User tokens (ADR-0068): the approval belongs to the user of the token.
#[tokio::test]
async fn accepts_decisions_only_with_the_token_of_the_user_of_the_run() {
    const ANNA: &str = "gw_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const BOB: &str = "gw_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let users = grauwolf_server::Users::from_json(
        &json!({
            "anna": format!("sha256:{}", grauwolf_server::token_hash(ANNA)),
            "bob": format!("sha256:{}", grauwolf_server::token_hash(BOB)),
        })
        .to_string(),
    )
    .unwrap();
    let base = serve_wrapped(|router| grauwolf_server::with_auth(router, None, users)).await;
    let client = reqwest::Client::new();
    let mut stream = client
        .post(format!("{base}/run/stream"))
        .bearer_auth(ANNA)
        .json(&json!({ "input": "Say it loudly" }))
        .send()
        .await
        .unwrap();
    let mut buffer = String::new();

    let approval = next_event(&mut stream, &mut buffer, "approval").await;
    let id = approval["id"].as_str().unwrap();
    let decide = |token: &'static str| {
        client
            .post(format!("{base}/approvals/{id}"))
            .bearer_auth(token)
            .json(&json!({ "approved": true }))
            .send()
    };
    let by_bob = decide(BOB).await.unwrap();
    let by_anna = decide(ANNA).await.unwrap();
    let output = next_event(&mut stream, &mut buffer, "output").await;

    assert_eq!(by_bob.status(), 404);
    assert_eq!(by_anna.status(), 204);
    assert_eq!(output, json!("Executed"));
}

/// Starts a service whose agent runs `echo ran > proof.txt` with `run_command` in shell mode
/// (ADR-0099) for "shell"; returns the base URL and the root directory of the tool.
#[cfg(unix)]
async fn serve_shell(name: &str) -> (String, grauwolf_test::TestDir) {
    let root = grauwolf_test::TestDir::new(&format!("shell-{name}"));
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "sheller",
            "model": { "provider": "mock", "id": "m" },
            "builtin_tools": {
                "tools": ["run_command"], "root": ".",
                "command": { "shell": true, "allowed": ["*"], "i_understand_shell": true }
            },
            "graph": {
                "nodes": [ { "id": "call_llm", "kind": "llm_request" },
                           { "id": "execute_tool", "kind": "execute_tool" },
                           { "id": "send_result", "kind": "llm_send_tool_result" } ],
                "edges": [ { "from": "start", "to": "call_llm" },
                           { "from": "call_llm", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
                           { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } },
                           { "from": "execute_tool", "to": "send_result" },
                           { "from": "send_result", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
            }
        })
        .to_string(),
    )
    .unwrap();
    let tools = grauwolf_host::provisioning::ProvisionedAgent::new(
        &spec,
        &root,
        grauwolf_host::provisioning::ProvisionOptions::new(Arc::new(
            grauwolf_host::config::ProcessEnv,
        ))
        .with_executor(Arc::new(PromptExecutor::builder().build())),
    )
    .await
    .unwrap()
    .tools();
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains(
            "shell",
            "run_command",
            json!({ "command": "echo ran > proof.txt" }),
        )
        .answer_when_contains("exit code: 0", "Ran")
        .answer_when_contains("approval", "Refused");
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", Arc::new(mock))
            .build(),
    ))
    .with_tools(tools)
    .load(&spec)
    .unwrap();
    let router =
        grauwolf_server::router(Arc::new(agent), Arc::new(MemoryChatHistoryStore::default()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (format!("http://{address}"), root)
}

/// Shell mode of `run_command` asks through `/approvals` for every call; the command runs only
/// once approved and not at all when refused (ADR-0099).
#[cfg(unix)]
#[tokio::test]
async fn asks_for_approval_of_every_shell_command_and_runs_it_only_once_approved() {
    let client = reqwest::Client::new();
    let mut outcomes = Vec::new();
    for approved in [true, false] {
        let (base, root) = serve_shell(&approved.to_string()).await;
        let mut stream = client
            .post(format!("{base}/run/stream"))
            .json(&json!({ "input": "Use the shell" }))
            .send()
            .await
            .unwrap();
        let mut buffer = String::new();

        let approval = next_event(&mut stream, &mut buffer, "approval").await;
        let ran_before_decision = root.join("proof.txt").exists();
        let id = approval["id"].as_str().unwrap();
        let decided = client
            .post(format!("{base}/approvals/{id}"))
            .json(&json!({ "approved": approved, "reason": "approval refused" }))
            .send()
            .await
            .unwrap();
        let output = next_event(&mut stream, &mut buffer, "output").await;
        let ran = root.join("proof.txt").exists();
        std::fs::remove_dir_all(&root).unwrap();

        assert_eq!(approval["tool"], "run_command");
        assert_eq!(
            approval["args"],
            json!({ "command": "echo ran > proof.txt" })
        );
        assert_eq!(decided.status(), 204);
        assert!(!ran_before_decision);
        outcomes.push((output, ran));
    }

    assert_eq!(
        outcomes,
        vec![(json!("Ran"), true), (json!("Refused"), false)]
    );
}
