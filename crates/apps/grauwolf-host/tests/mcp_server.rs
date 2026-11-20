//! An agent as MCP server with resources and a prompt (ADR-0089).

use grauwolf_host::mcp::agent_server;
use grauwolf_spec::AgentSpec;
use grauwolf_spec::AgentSpecExt;
use serde_json::{Value, json};

#[tokio::test]
async fn offers_the_spec_the_knowledge_and_a_prompt_of_the_agent() {
    let dir = grauwolf_test::TestDir::new("mcp-server");
    std::fs::create_dir_all(dir.join("wiki/animals")).unwrap();
    std::fs::write(dir.join("wiki/wolf.md"), "# Wolf\nA pack animal.").unwrap();
    std::fs::write(dir.join("wiki/animals/fox.txt"), "A loner.").unwrap();
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "encyclopedia",
            "model": { "provider": "ollama", "id": "m" },
            "system_prompt": "You are an encyclopedia.",
            "knowledge": { "sources": ["wiki"], "embedding_model": { "provider": "ollama", "id": "e" } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let server = agent_server(&spec, &dir, Vec::new()).await.unwrap();
    let call = |method: &'static str, params: Value| {
        let server = server.clone();
        async move {
            server
                .handle(json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
                .await
                .unwrap()
        }
    };

    let resources = call("resources/list", json!({})).await;
    let wolf = call(
        "resources/read",
        json!({ "uri": "grauwolf://encyclopedia/knowledge/wiki/wolf.md" }),
    )
    .await;
    let spec_text = call(
        "resources/read",
        json!({ "uri": "grauwolf://encyclopedia/spec" }),
    )
    .await;
    let prompt = call(
        "prompts/get",
        json!({ "name": "encyclopedia", "arguments": { "request": "What is a wolf?" } }),
    )
    .await;

    let uris: Vec<&str> = resources["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|resource| resource["uri"].as_str().unwrap())
        .collect();
    assert_eq!(
        uris,
        [
            "grauwolf://encyclopedia/spec",
            "grauwolf://encyclopedia/knowledge/wiki/animals/fox.txt",
            "grauwolf://encyclopedia/knowledge/wiki/wolf.md"
        ]
    );
    assert_eq!(wolf["result"]["contents"][0]["mimeType"], "text/markdown");
    assert_eq!(
        wolf["result"]["contents"][0]["text"],
        "# Wolf\nA pack animal."
    );
    let read_back: Value =
        serde_json::from_str(spec_text["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(read_back["name"], "encyclopedia");
    assert_eq!(
        prompt["result"]["messages"][0]["content"]["text"],
        "You are an encyclopedia.\n\nWhat is a wolf?"
    );
}
