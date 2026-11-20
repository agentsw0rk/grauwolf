//! Loads specs with their tools and sub-agents (ADR-0040, ADR-0050).

use std::path::PathBuf;

use grauwolf_host::loader::AgentLoader;
use grauwolf_host::provisioning::SubAgentSupplier as _;
use grauwolf_toolbuild::ToolBuilder;

fn specs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/specs")
}

#[tokio::test]
async fn loads_specs_with_sub_agents_and_offers_them_as_tools() {
    let tools_cache = grauwolf_test::TestDir::new("loader-tools");
    let loader = AgentLoader::new(ToolBuilder::new(tools_cache.path()));

    let (spec, _agent) = loader
        .load(&specs().join("coordinator.json"), None)
        .await
        .unwrap();
    let tools: Vec<_> = loader
        .supply(&spec, &specs())
        .await
        .unwrap()
        .iter()
        .map(|agent| agent.tool_for(None))
        .collect();

    assert_eq!(spec.name, "coordinator");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name(), "translator");
    assert_eq!(
        tools[0].descriptor().description,
        "Translates a German text into English"
    );
}

#[tokio::test]
async fn reports_invalid_specs_with_their_file_and_pointer() {
    let dir = grauwolf_test::TestDir::new("loader");
    let file = dir.join("broken.json");
    std::fs::write(
        &file,
        r#"{ "spec_version": 1, "name": "broken", "model": { "provider": "ollama", "id": "m" },
            "graph": { "edges": [ { "from": "start", "to": "nowhere" } ] } }"#,
    )
    .unwrap();

    let error = AgentLoader::new(ToolBuilder::new(&dir))
        .load(&file, None)
        .await
        .err()
        .unwrap();

    assert!(error.contains("broken.json:/graph/edges/0/to"), "{error}");
}

/// A fixed answer, to stand in for a System One service.
struct Technical;

impl grauwolf_agents::decision::Decider for Technical {
    fn decide<'a>(
        &'a self,
        _decision: &'a grauwolf_agents::decision::Decision,
        _state: &'a serde_json::Value,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<grauwolf_agents::decision::Verdict, grauwolf_agents::decision::DecideError>,
    > {
        Box::pin(std::future::ready(Ok(
            grauwolf_agents::decision::Verdict::new("technical"),
        )))
    }
}

#[tokio::test]
async fn gives_the_decider_to_specs_with_system_one_decisions() {
    let dir = grauwolf_test::TestDir::new("decider");
    let mut spec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(specs().join("ticket_router.json")).unwrap())
            .unwrap();
    spec["graph"]["nodes"][0]["params"]["decider"] = "system_one".into();
    let file = dir.join("ticket_router.json");
    std::fs::write(&file, spec.to_string()).unwrap();
    // No service in the environment, whatever the process has.
    let loader = || {
        AgentLoader::new(ToolBuilder::new(&dir))
            .with_env(std::sync::Arc::new(grauwolf_host::config::MapEnv::default()))
    };

    let without = loader().load(&file, None).await.err().unwrap();
    let with = loader().with_decider(std::sync::Arc::new(Technical));

    assert!(without.contains("system_one"), "{without}");
    assert!(with.load(&file, None).await.is_ok());
}

#[tokio::test]
async fn loads_the_flow_example_and_refuses_agent_types_that_differ_from_the_sub_agent() {
    let tools_cache = grauwolf_test::TestDir::new("loader-tools");
    let loader = AgentLoader::new(ToolBuilder::new(tools_cache.path()));
    loader
        .load(&specs().join("topic_report.json"), None)
        .await
        .unwrap();

    let dir = grauwolf_test::TestDir::new("loader-flow");
    std::fs::copy(specs().join("echo.json"), dir.join("echo.json")).unwrap();
    let file = dir.join("flow.json");
    std::fs::write(
        &file,
        r#"{ "spec_version": 1, "name": "flow", "model": { "provider": "ollama", "id": "m" },
             "output": "Topic",
             "types": { "Topic": { "type": "object", "properties": { "title": { "type": "string" } } } },
             "agent_tools": { "worker": { "spec": "echo.json", "description": "Echoes" } },
             "graph": {
               "nodes": [ { "id": "work", "kind": "agent", "params": { "agent": "worker", "output": "Topic" } } ],
               "edges": [ { "from": "start", "to": "work" }, { "from": "work", "to": "finish" } ]
             } }"#,
    )
    .unwrap();

    let error = loader.load(&file, None).await.err().unwrap();

    assert!(
        error.contains(
            "the agent 'worker' has the output type 'string', but the node declares 'Topic'"
        ),
        "{error}"
    );
}

/// A sub-agent runs on behalf of the user of the calling run (ADR-0114): its key-value store is
/// that user's.
#[tokio::test]
async fn sub_agents_use_the_key_value_store_of_the_calling_user() {
    use grauwolf_host::config::MapEnv;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let ollama = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "", "tool_calls": [
                { "function": { "name": "store_value",
                                "arguments": { "key": "color", "value": "red" } } }
            ] },
            "done": true
        })))
        .up_to_n_times(1)
        .mount(&ollama)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "stored" }, "done": true
        })))
        .mount(&ollama)
        .await;
    let dir = grauwolf_test::TestDir::new("sub-user");
    let model = serde_json::json!({ "provider": "ollama", "id": "m" });
    std::fs::write(
        dir.join("helper.json"),
        serde_json::json!({
            "spec_version": 1, "name": "helper", "model": model,
            "builtin_tools": { "tools": ["store_value"], "root": "." },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("main.json"),
        serde_json::json!({
            "spec_version": 1, "name": "main", "model": model,
            "agent_tools": { "helper": { "spec": "helper.json", "description": "Helps" } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let loader = AgentLoader::new(ToolBuilder::new(dir.join("tools"))).with_env(
        std::sync::Arc::new(MapEnv::new([("OLLAMA_HOST", ollama.uri().as_str())])),
    );
    let (spec, _agent, provisioned) = loader
        .load_provisioned(&dir.join("main.json"), None)
        .await
        .unwrap();

    let tools = provisioned.run(Some("anna")).unwrap().user_tools();
    let helper = tools.iter().find(|tool| tool.name() == "helper").unwrap();
    helper
        .call_json(serde_json::json!({ "request": "remember red" }))
        .await
        .unwrap();

    assert_eq!(spec.name, "main");
    let hex: String = "anna".bytes().map(|byte| format!("{byte:02x}")).collect();
    assert!(
        dir.join(format!("users/{hex}/kv.json")).exists(),
        "{:?}",
        std::fs::read_dir(&dir).unwrap().collect::<Vec<_>>()
    );
}

/// A sub-agent that two parents name (a diamond) is loaded and provisioned once and shared:
/// one connection to its MCP server (ADR-0115).
#[tokio::test]
async fn a_sub_agent_under_two_parents_is_provisioned_once() {
    use grauwolf_mcp::testing::SseTestServer;
    let server = SseTestServer::start().await;
    let dir = grauwolf_test::TestDir::new("diamond");
    let model = serde_json::json!({ "provider": "ollama", "id": "m" });
    let write = |file: &str, subs: &[(&str, &str)], extra: serde_json::Value| {
        let mut value = serde_json::json!({
            "spec_version": 1, "name": file.trim_end_matches(".json"), "model": model,
            "strategy": { "kind": "single_run" },
            "agent_tools": subs.iter().map(|(n, f)| ((*n).to_owned(),
                serde_json::json!({ "spec": f, "description": "d" })))
                .collect::<serde_json::Map<_, _>>()
        });
        for (key, field) in extra.as_object().into_iter().flatten() {
            value[key] = field.clone();
        }
        std::fs::write(dir.join(file), value.to_string()).unwrap();
    };
    write(
        "root.json",
        &[("left", "left.json"), ("right", "right.json")],
        serde_json::json!({}),
    );
    write(
        "left.json",
        &[("shared", "shared.json")],
        serde_json::json!({}),
    );
    write(
        "right.json",
        &[("shared", "shared.json")],
        serde_json::json!({}),
    );
    write(
        "shared.json",
        &[],
        serde_json::json!({ "mcp_servers": { "legacy": {
            "url": server.url(), "transport": "sse", "tools": ["reverse"] } } }),
    );
    let loader = AgentLoader::new(ToolBuilder::new(dir.join("tools")))
        .with_env(std::sync::Arc::new(grauwolf_host::config::MapEnv::default()));

    loader.load(&dir.join("root.json"), None).await.unwrap();

    assert_eq!(server.connections(), 1);
}
