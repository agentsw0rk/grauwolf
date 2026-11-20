//! Self-created tools in specs (ADR-0075).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, SelfToolsSpec, ToolOrigin, ValidationError, ValidationErrorKind};
use serde_json::{Value, json};

fn parse(extra: Value) -> Result<AgentSpec, grauwolf_spec::SpecError> {
    let mut spec = json!({
        "spec_version": 1, "name": "agent",
        "model": { "provider": "ollama", "id": "m" },
        "strategy": { "kind": "single_run" }
    });
    spec.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    AgentSpec::from_json(&spec.to_string())
}

#[test]
fn parses_self_tools_with_safe_defaults() {
    let spec = parse(json!({ "self_tools": {} })).unwrap();

    assert_eq!(
        spec.self_tools,
        Some(SelfToolsSpec {
            enabled: true,
            tool: "create_tool".into(),
            max_tools: 5,
            max_code_bytes: 20_000,
            confirm: true,
            save: false,
        })
    );
    assert_eq!(parse(json!({})).unwrap().self_tools, None);
}

#[test]
fn reports_a_taken_tool_name_and_saving_without_a_tools_directory() {
    let spec = parse(json!({
        "builtin_tools": { "tools": ["say_to_user"] },
        "self_tools": { "tool": "say_to_user", "save": true }
    }))
    .unwrap();

    assert_eq!(
        spec.validate().unwrap_err(),
        vec![
            ValidationError {
                path: "/self_tools/tool".into(),
                kind: ValidationErrorKind::DuplicateToolName {
                    name: "say_to_user".into(),
                    other_origin: ToolOrigin::Builtin,
                    other_pointer: "/builtin_tools/tools/0".into()
                },
            },
            ValidationError {
                path: "/self_tools/save".into(),
                kind: ValidationErrorKind::SaveWithoutToolsDir,
            },
        ]
    );
}

#[test]
fn parses_the_switch_and_the_permission_of_subgraphs() {
    let spec = parse(json!({
        "self_tools": { "enabled": false },
        "subgraphs": { "werkstatt": {
            "tools": [], "self_tools": true,
            "graph": {
                "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
                "edges": [ { "from": "start", "to": "call_llm" },
                           { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
            }
        } }
    }))
    .unwrap();

    assert!(!spec.self_tools.unwrap().enabled);
    assert_eq!(spec.subgraphs["werkstatt"].self_tools, Some(true));
}

#[test]
fn reports_a_subgraph_permission_without_self_tools() {
    let spec = parse(json!({
        "subgraphs": { "werkstatt": {
            "self_tools": true,
            "graph": {
                "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
                "edges": [ { "from": "start", "to": "call_llm" },
                           { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
            }
        } }
    }))
    .unwrap();

    assert_eq!(
        spec.validate().unwrap_err(),
        vec![ValidationError {
            path: "/subgraphs/werkstatt/self_tools".into(),
            kind: ValidationErrorKind::SelfToolsNotConfigured,
        }]
    );
}
