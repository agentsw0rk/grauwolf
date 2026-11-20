//! Provides the external tools of a spec (ADR-0050).

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;
use std::sync::Arc;

mod support;
use grauwolf_spec::AgentSpec;

#[tokio::test]
async fn provides_the_builtin_tools_of_a_spec_relative_to_its_directory() {
    let specs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/specs");
    let spec =
        AgentSpec::from_json(&std::fs::read_to_string(specs.join("notes_reader.json")).unwrap())
            .unwrap();

    let tools = support::tools(&spec, &specs).await.unwrap();
    let read = tools
        .iter()
        .find(|tool| tool.name() == "read_file")
        .unwrap();
    let notes = read
        .call_json(serde_json::json!({ "path": "notes.md" }))
        .await
        .unwrap();

    assert_eq!(tools.len(), spec.external_tools().len());
    assert!(notes.contains("grauwolf"), "{notes}");
}

#[tokio::test]
async fn provides_the_think_tool_without_a_root() {
    let spec = AgentSpec::from_json(
        &serde_json::json!({
            "name": "thinker",
            "spec_version": 1,
            "model": { "provider": "ollama", "id": "q" },
            "system_prompt": "Think first.",
            "builtin_tools": { "tools": ["think"] },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    assert_eq!(spec.validate(), Ok(()));
    let tools = support::tools(&spec, &PathBuf::from(".")).await.unwrap();
    let answer = tools[0]
        .call_json(serde_json::json!({ "thought": "hm" }))
        .await
        .unwrap();

    assert_eq!(answer, "ok");
    assert_eq!(spec.external_tools(), ["think"]);
}

#[tokio::test]
async fn provides_the_fetch_tool_for_the_hosts_of_the_spec() {
    let spec = AgentSpec::from_json(
        &serde_json::json!({
            "name": "fetcher",
            "spec_version": 1,
            "model": { "provider": "ollama", "id": "q" },
            "system_prompt": "Fetch.",
            "builtin_tools": { "tools": ["fetch"], "fetch": { "allowed_hosts": ["example.com"] } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    let tools = support::tools(&spec, &PathBuf::from(".")).await.unwrap();
    let refused = tools[0]
        .call_json(serde_json::json!({ "url": "https://other.example.net/" }))
        .await
        .unwrap_err();

    assert_eq!(spec.validate(), Ok(()));
    assert!(
        refused.to_string().contains("not in allowed_hosts"),
        "{refused}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn provides_the_command_tool_in_the_root_with_the_programs_of_the_spec() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("host-command");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("work")).unwrap();
    std::fs::write(base.join("work/notes.txt"), "a\nb\n").unwrap();
    let spec = AgentSpec::from_json(
        &serde_json::json!({
            "name": "runner",
            "spec_version": 1,
            "model": { "provider": "ollama", "id": "q" },
            "system_prompt": "Run.",
            "builtin_tools": {
                "tools": ["run_command"], "root": "work",
                "command": { "allowed": ["wc"], "env": ["PATH"] }
            },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    let tools = support::tools_in(&spec, &base, grauwolf_host::config::ProcessEnv, None)
        .await
        .unwrap();
    let counted = tools[0]
        .call_json(serde_json::json!({ "program": "wc", "args": ["-l", "notes.txt"] }))
        .await
        .unwrap();
    let refused = tools[0]
        .call_json(serde_json::json!({ "program": "ls" }))
        .await
        .unwrap_err();

    assert_eq!(spec.validate(), Ok(()));
    assert!(
        counted.starts_with("exit code: 0") && counted.contains("2 notes.txt"),
        "{counted}"
    );
    assert!(refused.to_string().contains("not allowed"), "{refused}");
    assert_eq!(spec.external_tools(), ["run_command"]);
}

fn kv_spec() -> AgentSpec {
    AgentSpec::from_json(
        &serde_json::json!({
            "name": "keeper",
            "spec_version": 1,
            "model": { "provider": "ollama", "id": "q" },
            "system_prompt": "Keep values.",
            "builtin_tools": { "tools": ["store_value", "retrieve_value", "list_keys"], "root": "data" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap()
}

#[tokio::test]
async fn keeps_key_values_in_kv_json_under_the_root_and_separately_per_user() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/facade-kv");
    let _ = std::fs::remove_dir_all(&base);
    let spec = kv_spec();
    assert_eq!(spec.validate(), Ok(()));
    let tools_of = |user: Option<&'static str>| {
        let (spec, base) = (spec.clone(), base.clone());
        async move { support::tools_for(&spec, &base, user).await.unwrap() }
    };
    let store = |tools: Vec<Arc<dyn grauwolf_tools::DynTool>>, key: &'static str| async move {
        let tool = tools.iter().find(|t| t.name() == "store_value").unwrap();
        tool.call_json(serde_json::json!({ "key": key, "value": "v" }))
            .await
            .unwrap();
    };

    store(tools_of(None).await, "shared").await;
    store(tools_of(Some("anna")).await, "private").await;
    let list = |tools: Vec<Arc<dyn grauwolf_tools::DynTool>>| async move {
        let tool = tools.iter().find(|t| t.name() == "list_keys").unwrap();
        tool.call_json(serde_json::json!({})).await.unwrap()
    };

    assert_eq!(list(tools_of(None).await).await, "shared");
    assert_eq!(list(tools_of(Some("anna")).await).await, "private");
    assert_eq!(list(tools_of(Some("bob")).await).await, "No keys stored");
    assert!(base.join("data/kv.json").exists());
}
