//! Chat API mode against servers (ADR-0094): MCP tool lists, the Ollama fallback and digests.

use grauwolf_spec::AgentSpecExt;
use std::collections::HashMap;

use grauwolf_agents::events::AgentEvent;
use grauwolf_prompt::{LlModel, Message, Prompt};
use grauwolf_server::chat_api::ChatApi;
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn scratch(name: &str) -> grauwolf_test::TestDir {
    grauwolf_test::TestDir::new(&format!("chat_api-global-{name}"))
}

fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + Clone + use<> {
    let vars: HashMap<String, String> = vars
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    move |key| vars.get(key).cloned()
}

/// Listing the tools of an MCP server is recorded with its schema hash.
#[tokio::test]
async fn records_the_tools_an_mcp_server_offers() {
    use wiremock::matchers::path;
    let rpc = |id: u64, result: Value| {
        ResponseTemplate::new(200)
            .set_body_json(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    };
    let mcp = MockServer::start().await;
    Mock::given(path("/mcp"))
        .and(body_partial_json(json!({ "method": "initialize" })))
        .respond_with(rpc(
            1,
            json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "serverInfo": { "name": "docs" } }),
        ))
        .mount(&mcp)
        .await;
    Mock::given(path("/mcp"))
        .and(body_partial_json(
            json!({ "method": "notifications/initialized" }),
        ))
        .respond_with(ResponseTemplate::new(202))
        .mount(&mcp)
        .await;
    Mock::given(path("/mcp"))
        .and(body_partial_json(json!({ "method": "tools/list" })))
        .respond_with(rpc(
            2,
            json!({ "tools": [ {
                "name": "search_docs", "description": "Searches",
                "inputSchema": {
                    "type": "object",
                    "properties": { "query": { "type": "string", "description": "Search text" } },
                    "required": ["query"]
                }
            } ] }),
        ))
        .mount(&mcp)
        .await;
    let spec = grauwolf_spec::AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "docs_agent",
            "model": { "provider": "ollama", "id": "m" },
            "mcp_servers": { "docs": { "url": format!("{}/mcp", mcp.uri()), "tools": ["search_docs"] } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let chat_api = ChatApi::from_dir(&scratch("mcp"), env(&[]));

    let observer = chat_api.observations().mcp_tools_observer();
    grauwolf_host::provisioning::ProvisionedAgent::new(
        &spec,
        &scratch("mcp-base"),
        grauwolf_host::provisioning::ProvisionOptions::new(std::sync::Arc::new(
            grauwolf_host::config::MapEnv::new([]),
        ))
        .with_mcp_tools_observer(Some(observer)),
    )
    .await
    .unwrap();

    let report = chat_api.report().await;
    let tools = &report["mcpTools"]["docs"][0]["tools"];
    assert_eq!(tools[0]["name"], "search_docs");
    assert_eq!(tools[0]["description"], "Searches");
    assert_eq!(tools[0]["schemaHash"].as_str().unwrap().len(), 64);
}

#[cfg(feature = "ollama")]
/// In chat API mode its deployments' `OLLAMA_BASE_URL` stands in for `OLLAMA_HOST`.
#[tokio::test]
async fn ollama_base_url_replaces_a_missing_ollama_host() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "message": { "role": "assistant", "content": "Hi" }, "done": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    let settings = env(&[("OLLAMA_BASE_URL", &server.uri())]);
    let config =
        grauwolf_host::config::ProvidersConfig::from_lookup(&settings).with_chat_api_mode(true);
    let client = grauwolf_host::providers::client_with("ollama", &config).unwrap();
    let prompt = Prompt::builder("p").user("Hello").build();

    let answer = client
        .execute(&prompt, &LlModel::new("ollama", "m"), &[])
        .await
        .unwrap();

    assert_eq!(answer, Message::assistant("Hi"));
}

#[cfg(feature = "ollama")]
/// The digest comes from the Ollama server at the same address.
#[tokio::test]
async fn names_ollama_models_with_their_digest() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "models": [ { "name": "qwen3.5:9b", "digest": "0123456789abcdef0123" } ]
        })))
        .mount(&server)
        .await;
    let chat_api = ChatApi::from_dir(
        &scratch("digest"),
        env(&[("OLLAMA_BASE_URL", &server.uri())]),
    );
    let feature = chat_api.observations().feature_for("agent");
    feature.on_event(&AgentEvent::LlmCallStarting {
        model: LlModel::new("ollama", "qwen3.5:9b"),
        tools: vec![],
    });
    feature.on_event(&AgentEvent::LlmCallCompleted {
        answer: Message::assistant("Hi"),
        usage: None,
        cost: None,
        model: Some("qwen3.5:9b".into()),
    });

    let report = chat_api.report().await;

    assert_eq!(
        report["models"]["agent"][0]["modelId"],
        "qwen3.5:9b@0123456789ab"
    );
}
