//! Collisions in the tool namespace (ADR-0025, ADR-0117): one rule, the error is at the later
//! declaration and names the earlier tool.

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ToolOrigin, ValidationErrorKind};
use serde_json::{Value, json};

fn spec(extra: Value) -> AgentSpec {
    let mut doc = json!({
        "spec_version": 1, "name": "agent",
        "model": { "provider": "ollama", "id": "m" },
        "strategy": { "kind": "single_run" }
    });
    doc.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    AgentSpec::from_json(&doc.to_string()).unwrap()
}

fn errors(extra: Value) -> Vec<(String, ValidationErrorKind)> {
    match spec(extra).validate() {
        Ok(()) => Vec::new(),
        Err(found) => found.into_iter().map(|e| (e.path, e.kind)).collect(),
    }
}

fn duplicate(name: &str, origin: ToolOrigin, pointer: &str) -> ValidationErrorKind {
    ValidationErrorKind::DuplicateToolName {
        name: name.into(),
        other_origin: origin,
        other_pointer: pointer.into(),
    }
}

const EMBEDDING: &str = r#"{ "provider": "ollama", "id": "nomic-embed-text" }"#;

fn embedding() -> Value {
    serde_json::from_str(EMBEDDING).unwrap()
}

#[test]
fn a_builtin_tool_named_like_the_knowledge_tool_is_reported_at_the_builtin_tool() {
    let found = errors(json!({
        "knowledge": { "sources": ["docs"], "embedding_model": embedding(), "tool": "think" },
        "builtin_tools": { "tools": ["think"] }
    }));
    assert_eq!(
        found,
        [(
            "/builtin_tools/tools/0".to_owned(),
            duplicate("think", ToolOrigin::Knowledge, "/knowledge/tool")
        )]
    );
}

#[test]
fn a_builtin_tool_named_like_a_memory_tool_is_reported_at_the_builtin_tool() {
    let found = errors(json!({
        "long_term_memory": {
            "store": "memory.json", "embedding_model": embedding(), "recall_tool": "think"
        },
        "builtin_tools": { "tools": ["think"] }
    }));
    assert_eq!(
        found,
        [(
            "/builtin_tools/tools/0".to_owned(),
            duplicate("think", ToolOrigin::Memory, "/long_term_memory/recall_tool")
        )]
    );
}

#[test]
fn a_skill_tool_named_like_the_knowledge_tool_is_reported_at_the_skill_tool() {
    let found = errors(json!({
        "skills": [{ "dir": "skills" }],
        "knowledge": { "sources": ["docs"], "embedding_model": embedding(), "tool": "load_skill" }
    }));
    // The knowledge tool comes first; the skill tool is the later declaration.
    assert_eq!(
        found,
        [(
            "/skills".to_owned(),
            duplicate("load_skill", ToolOrigin::Knowledge, "/knowledge/tool")
        )]
    );
}

#[test]
fn an_own_tool_named_like_a_skill_tool_is_reported_at_the_skill_tool() {
    let found = errors(json!({
        "skills": [{ "dir": "skills" }],
        "tools": [{ "name": "load_skill", "description": "d",
            "args": { "type": "object" }, "code": "Ok(String::new())" }]
    }));
    assert_eq!(
        found,
        [(
            "/skills".to_owned(),
            duplicate("load_skill", ToolOrigin::Own, "/tools/0/name")
        )]
    );
}

#[test]
fn an_mcp_tool_named_like_the_memory_tool_is_reported_at_the_later_memory_tool() {
    let found = errors(json!({
        "mcp_servers": { "docs": { "command": "docs", "tools": ["remember"] } },
        "long_term_memory": { "store": "memory.json", "embedding_model": embedding() }
    }));
    assert_eq!(
        found,
        [(
            "/long_term_memory/remember_tool".to_owned(),
            duplicate("remember", ToolOrigin::Mcp, "/mcp_servers/docs/tools/0")
        )]
    );
}

#[test]
fn a_package_tool_named_like_a_sub_agent_is_reported_at_the_package_tool() {
    let found = errors(json!({
        "agent_tools": { "helper": { "url": "https://agent.example/" } },
        "tool_packages": [{ "oci": "ghcr.io/a/b:1", "tools": ["add"], "as": { "add": "helper" } }]
    }));
    assert_eq!(
        found,
        [(
            "/tool_packages/0/as/add".to_owned(),
            duplicate("helper", ToolOrigin::SubAgent, "/agent_tools/helper")
        )]
    );
}

#[test]
fn the_self_build_tool_is_reported_once_even_if_a_package_tool_has_its_name() {
    let found = errors(json!({
        "tool_packages": [{ "oci": "ghcr.io/a/b:1", "tools": ["create_tool"] }],
        "self_tools": {}
    }));
    assert_eq!(
        found,
        [(
            "/self_tools/tool".to_owned(),
            duplicate(
                "create_tool",
                ToolOrigin::Package,
                "/tool_packages/0/tools/0"
            )
        )]
    );
}

#[test]
fn the_error_text_names_the_other_tool_and_its_pointer() {
    let text = ValidationErrorKind::DuplicateToolName {
        name: "think".into(),
        other_origin: ToolOrigin::Knowledge,
        other_pointer: "/knowledge/tool".into(),
    }
    .to_string();
    assert_eq!(
        text,
        "tool name 'think' is used more than once (the other one is the knowledge search tool at /knowledge/tool)"
    );
}
