//! The handles of a provisioned agent in the service (ADR-0114): per user, per request.

use std::sync::Arc;

use grauwolf_host::config::MapEnv;
use grauwolf_host::provisioning::{ProvisionOptions, ProvisionedAgent};
use grauwolf_server::ServiceOptions;
use grauwolf_spec::{AgentSpec, AgentSpecExt};
use serde_json::json;

#[tokio::test]
async fn every_user_of_the_service_has_their_own_key_value_store() {
    let base = grauwolf_test::TestDir::new("served-kv");
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "kv",
            "model": { "provider": "ollama", "id": "m" },
            "builtin_tools": { "tools": ["store_value", "retrieve_value"], "root": "." },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let agent = ProvisionedAgent::new(
        &spec,
        &base,
        ProvisionOptions::new(Arc::new(MapEnv::new([])))
            .with_executor(Arc::new(grauwolf_llm::PromptExecutor::builder().build())),
    )
    .await
    .unwrap();
    let options = ServiceOptions::provisioned(Arc::new(agent));

    let anna = options.extras_for(Some("anna")).await.tools;
    let bob = options.extras_for(Some("bob")).await.tools;
    let find = |tools: &[Arc<dyn grauwolf_tools::DynTool>], name: &str| {
        Arc::clone(tools.iter().find(|tool| tool.name() == name).unwrap())
    };
    find(&anna, "store_value")
        .call_json(json!({ "key": "color", "value": "red" }))
        .await
        .unwrap();

    let anna_sees = find(&anna, "retrieve_value")
        .call_json(json!({ "key": "color" }))
        .await
        .unwrap();
    let bob_sees = find(&bob, "retrieve_value")
        .call_json(json!({ "key": "color" }))
        .await
        .unwrap();

    assert!(anna_sees.contains("red"), "{anna_sees}");
    assert!(!bob_sees.contains("red"), "{bob_sees}");
}

/// Many concurrent `/run` requests of two users against one provisioned agent: every value the
/// model stores lands in the key-value store of the user who asked, never in the other's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_runs_of_two_users_keep_their_key_value_stores_apart() {
    const RUNS: usize = 8;
    let base = grauwolf_test::TestDir::new("served-kv-race");
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "kv",
            "model": { "provider": "ollama", "id": "m" },
            "builtin_tools": { "tools": ["store_value", "retrieve_value", "list_keys"], "root": "." },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    // The model stores exactly the value named in the request, so a value in the wrong store
    // can only come from the service handing a run the wrong user's tools.
    let mut mock =
        grauwolf_test::MockLlmClient::new("?").answer_when_contains("Stored key", "done");
    for user in ["anna", "bob"] {
        for run in 0..RUNS {
            mock = mock.call_tool_when_contains(
                format!("<{user}-{run}>"),
                "store_value",
                json!({ "key": format!("{user}-{run}"), "value": format!("{user}-{run}") }),
            );
        }
    }
    let executor = Arc::new(
        grauwolf_llm::PromptExecutor::builder()
            .client("ollama", Arc::new(mock))
            .build(),
    );
    let provisioned = Arc::new(
        ProvisionedAgent::new(
            &spec,
            &base,
            ProvisionOptions::new(Arc::new(MapEnv::new([]))).with_executor(Arc::clone(&executor)),
        )
        .await
        .unwrap(),
    );
    let agent = grauwolf_interpreter::Interpreter::new(executor)
        .with_tools(provisioned.tools())
        .load(&spec)
        .unwrap();
    let router = grauwolf_server::router_with(
        Arc::new(agent),
        Arc::new(grauwolf_agents::memory::MemoryChatHistoryStore::default()),
        ServiceOptions::provisioned(Arc::clone(&provisioned)),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/run", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = reqwest::Client::new();
    let requests = (0..RUNS).flat_map(|run| {
        ["anna", "bob"].map(|user| {
            let request = client
                .post(&url)
                .json(&json!({ "input": format!("Store <{user}-{run}>"), "user_id": user }));
            async move {
                let response = request.send().await.unwrap();
                assert!(
                    response.status().is_success(),
                    "{user}-{run}: {}",
                    response.status()
                );
            }
        })
    });
    futures_util::future::join_all(requests).await;

    let options = ServiceOptions::provisioned(provisioned);
    for (user, other) in [("anna", "bob"), ("bob", "anna")] {
        let tools = options.extras_for(Some(user)).await.tools;
        let keys = tools
            .iter()
            .find(|tool| tool.name() == "list_keys")
            .unwrap()
            .call_json(json!({}))
            .await
            .unwrap();
        for run in 0..RUNS {
            assert!(
                keys.contains(&format!("{user}-{run}")),
                "{user} misses {user}-{run}: {keys}"
            );
        }
        assert!(!keys.contains(other), "{user} sees keys of {other}: {keys}");
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// A user ID the stores cannot hold (ADR-0060: 1 to 128 bytes) is refused at the entry instead of
/// running the request without the user's tools.
#[tokio::test]
async fn a_run_with_an_invalid_user_id_is_refused() {
    let base = grauwolf_test::TestDir::new("served-bad-user");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "kv",
            "model": { "provider": "ollama", "id": "m" },
            "builtin_tools": { "tools": ["store_value"], "root": "." },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let executor = Arc::new(
        grauwolf_llm::PromptExecutor::builder()
            .client("ollama", Arc::new(grauwolf_test::MockLlmClient::new("ran")))
            .build(),
    );
    let provisioned = Arc::new(
        ProvisionedAgent::new(
            &spec,
            &base,
            ProvisionOptions::new(Arc::new(MapEnv::new([]))).with_executor(Arc::clone(&executor)),
        )
        .await
        .unwrap(),
    );
    let agent = grauwolf_interpreter::Interpreter::new(executor)
        .with_tools(provisioned.tools())
        .load(&spec)
        .unwrap();
    let router = grauwolf_server::router_with(
        Arc::new(agent),
        Arc::new(grauwolf_agents::memory::MemoryChatHistoryStore::default()),
        ServiceOptions::provisioned(provisioned),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::new();
    let post = |path: &str, user: &str| {
        client
            .post(format!("{base_url}{path}"))
            .json(&json!({ "input": "Hello", "user_id": user }))
            .send()
    };

    let too_long = "x".repeat(129);
    let refused = post("/run", &too_long).await.unwrap();
    let refused_stream = post("/run/stream", &too_long).await.unwrap();
    let empty = post("/run", "").await.unwrap();
    let fine = post("/run", &"x".repeat(128)).await.unwrap();

    assert_eq!(refused.status(), 400);
    let body: serde_json::Value = refused.json().await.unwrap();
    assert!(
        body["error"].as_str().unwrap().contains("invalid user ID"),
        "{body}"
    );
    assert_eq!(refused_stream.status(), 400);
    assert_eq!(empty.status(), 400);
    assert_eq!(fine.status(), 200);
    let _ = std::fs::remove_dir_all(&base);
}
