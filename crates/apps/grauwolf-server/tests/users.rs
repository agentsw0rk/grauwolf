//! User tokens (ADR-0068): the service determines the user itself.

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_agents::memory::MemoryChatHistoryStore;
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_server::Users;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

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

const ANNA_TOKEN: &str = "gw_00112233445566778899aabbccddeeff";

/// Service with user tokens for `anna` and the shared token `backend`.
async fn serve() -> String {
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
        .answer_when_contains("bob", "You are bob.")
        .answer_when_contains("nobody", "I do not know that.");
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("ollama", Arc::new(mock))
            .build(),
    ))
    .with_tools(vec![Arc::new(WhoAmI("nobody".into())) as Arc<dyn DynTool>])
    .load(&spec)
    .unwrap();
    let router = grauwolf_server::router_with_user_tools(
        Arc::new(agent),
        Arc::new(MemoryChatHistoryStore::default()),
        Arc::new(|user: &str| vec![Arc::new(WhoAmI(user.to_owned())) as Arc<dyn DynTool>]),
    );
    let users = Users::from_json(
        &json!({ "anna": format!("sha256:{}", grauwolf_server::token_hash(ANNA_TOKEN)) })
            .to_string(),
    )
    .unwrap();
    let router = grauwolf_server::with_auth(router, Some("backend".into()), users);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    base
}

#[tokio::test]
async fn runs_requests_with_a_user_token_as_that_user() {
    let base = serve().await;

    let answer: Value = reqwest::Client::new()
        .post(format!("{base}/run"))
        .bearer_auth(ANNA_TOKEN)
        .json(&json!({ "input": "Who am I?" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(answer, json!({ "output": "You are anna." }));
}

#[tokio::test]
async fn refuses_other_user_ids_from_users_but_not_from_the_backend() {
    let base = serve().await;
    let client = reqwest::Client::new();
    let run = |token: &str, body: Value| {
        client
            .post(format!("{base}/run"))
            .bearer_auth(token)
            .json(&body)
            .send()
    };

    let impersonation = run(
        ANNA_TOKEN,
        json!({ "input": "Who am I?", "user_id": "bob" }),
    )
    .await
    .unwrap();
    let own = run(
        ANNA_TOKEN,
        json!({ "input": "Who am I?", "user_id": "anna" }),
    )
    .await
    .unwrap();
    let backend: Value = run("backend", json!({ "input": "Who am I?", "user_id": "bob" }))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let unknown = run("gw_wrong", json!({ "input": "Who am I?" }))
        .await
        .unwrap();

    assert_eq!(impersonation.status(), 403);
    assert_eq!(own.status(), 200);
    assert_eq!(backend, json!({ "output": "You are bob." }));
    assert_eq!(unknown.status(), 401);
}

/// Memory that records for which user it was queried.
struct Facts {
    user: Option<String>,
    asked: Arc<std::sync::Mutex<Vec<Option<String>>>>,
}

impl grauwolf_agents::memory::FactMemory for Facts {
    fn recall<'a>(
        &'a self,
        _query: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, grauwolf_agents::memory::MemoryError>> {
        self.asked.lock().unwrap().push(self.user.clone());
        Box::pin(async { Ok(Vec::new()) })
    }

    fn remember<'a>(
        &'a self,
        _facts: &'a [String],
    ) -> BoxFuture<'a, Result<(), grauwolf_agents::memory::MemoryError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Automatic memory (ADR-0069) in the service: per user of the token.
#[tokio::test]
async fn recalls_from_the_memory_of_the_user_of_the_token() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "assistant",
            "model": { "provider": "ollama", "id": "m" },
            "long_term_memory": {
                "store": "m.json",
                "embedding_model": { "provider": "ollama", "id": "embed" },
                "auto_recall": true
            },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("ollama", Arc::new(MockLlmClient::new("Hello")))
            .build(),
    ))
    .with_tools(vec![
        Arc::new(Named("remember")) as Arc<dyn DynTool>,
        Arc::new(Named("recall")),
    ])
    .load(&spec)
    .unwrap();
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let facts_asked = Arc::clone(&asked);
    let router = grauwolf_server::router_with(
        Arc::new(agent),
        Arc::new(MemoryChatHistoryStore::default()),
        grauwolf_server::ServiceOptions {
            facts: Some(Arc::new(move |user: Option<&str>| {
                Arc::new(Facts {
                    user: user.map(str::to_owned),
                    asked: Arc::clone(&facts_asked),
                }) as Arc<dyn grauwolf_agents::memory::FactMemory>
            })),
            ..grauwolf_server::ServiceOptions::default()
        },
    );
    let users = Users::from_json(
        &json!({ "anna": format!("sha256:{}", grauwolf_server::token_hash(ANNA_TOKEN)) })
            .to_string(),
    )
    .unwrap();
    let router = grauwolf_server::with_auth(router, None, users);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let status = reqwest::Client::new()
        .post(format!("{base}/run"))
        .bearer_auth(ANNA_TOKEN)
        .json(&json!({ "input": "What do I drink?" }))
        .send()
        .await
        .unwrap()
        .status();

    assert_eq!(status, 200);
    assert_eq!(*asked.lock().unwrap(), [Some("anna".to_owned())]);
}

/// Stands in for a tool with a fixed name.
struct Named(&'static str);

impl DynTool for Named {
    fn name(&self) -> &str {
        self.0
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.0.into(),
            description: String::new(),
            parameters: json!({ "type": "object" }),
        }
    }

    fn call_json(&self, _args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async { Ok(String::new()) })
    }
}

#[test]
fn a_users_file_with_an_invalid_user_name_is_refused() {
    let hash = format!("sha256:{}", grauwolf_server::token_hash("gw_x"));

    let too_long = Users::from_json(&json!({ "x".repeat(129): hash }).to_string());
    let empty = Users::from_json(&json!({ "": hash }).to_string());

    let error = too_long.err().unwrap();
    assert!(error.contains("invalid user ID"), "{error}");
    assert!(empty.is_err());
}
