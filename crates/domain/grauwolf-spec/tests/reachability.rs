//! Reachability in the graph (F13.5).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ValidationError, ValidationErrorKind};
use serde_json::{Value, json};

fn spec(graph: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "graph": graph
        })
        .to_string(),
    )
    .unwrap()
}

fn errors(spec: &AgentSpec) -> Vec<ValidationError> {
    spec.validate().err().unwrap_or_default()
}

#[test]
fn reports_graphs_where_finish_cannot_be_reached() {
    let errors = errors(&spec(json!({})));

    assert_eq!(
        errors,
        [ValidationError {
            path: "/graph".into(),
            kind: ValidationErrorKind::FinishUnreachable,
        }]
    );
}

#[test]
fn reports_nodes_that_cannot_be_reached_from_start() {
    let errors = errors(&spec(json!({
        "nodes": [ { "id": "call_llm", "kind": "llm_request" },
                   { "id": "orphan", "kind": "llm_request" } ],
        "edges": [ { "from": "start", "to": "call_llm" },
                   { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
    })));

    assert_eq!(
        errors,
        [ValidationError {
            path: "/graph/nodes/1".into(),
            kind: ValidationErrorKind::UnreachableNode {
                id: "orphan".into()
            },
        }]
    );
}

#[test]
fn reports_nodes_from_which_finish_cannot_be_reached() {
    let errors = errors(&spec(json!({
        "nodes": [ { "id": "call_llm", "kind": "llm_request" },
                   { "id": "execute_tools", "kind": "execute_multiple_tools" },
                   { "id": "send_results", "kind": "llm_send_multiple_tool_results" } ],
        "edges": [ { "from": "start", "to": "call_llm" },
                   { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } },
                   { "from": "call_llm", "to": "execute_tools", "when": { "kind": "on_multiple_tool_calls" } },
                   { "from": "execute_tools", "to": "send_results" } ]
    })));

    // `execute_tools` also leads only into the dead end `send_results`.
    assert_eq!(
        errors,
        [
            ValidationError {
                path: "/graph/nodes/1".into(),
                kind: ValidationErrorKind::DeadEnd {
                    id: "execute_tools".into()
                },
            },
            ValidationError {
                path: "/graph/nodes/2".into(),
                kind: ValidationErrorKind::DeadEnd {
                    id: "send_results".into()
                },
            },
        ]
    );
}
