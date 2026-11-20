//! `params` are checked against the parameter schema of the catalog kind (F13.5, ADR-0018).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ValidationError, ValidationErrorKind};
use serde_json::{Value, json};

fn spec(nodes: Value, edges: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "types": { "Ticket": { "type": "object" } },
            "output": "Ticket",
            "graph": { "nodes": nodes, "edges": edges }
        })
        .to_string(),
    )
    .unwrap()
}

fn params_errors(spec: &AgentSpec) -> Vec<ValidationError> {
    spec.validate()
        .err()
        .unwrap_or_default()
        .into_iter()
        .filter(|error| matches!(error.kind, ValidationErrorKind::InvalidParams { .. }))
        .collect()
}

#[test]
fn rejects_params_for_kinds_without_parameters() {
    let spec = spec(
        json!([ { "id": "call_llm", "kind": "llm_request", "params": { "foo": 1 } } ]),
        json!([ { "from": "start", "to": "call_llm" } ]),
    );

    assert_eq!(
        params_errors(&spec),
        [ValidationError {
            path: "/graph/nodes/0/params".into(),
            kind: ValidationErrorKind::InvalidParams {
                kind: "llm_request".into(),
                reason: "/foo: unknown parameter".into(),
            },
        }]
    );
}

#[test]
fn checks_params_against_the_schema_of_the_kind() {
    let spec = spec(
        json!([ { "id": "triage", "kind": "llm_request_structured",
                  "params": { "type": "Ticket", "retries": "zwei", "extra": true } } ]),
        json!([ { "from": "start", "to": "triage" }, { "from": "triage", "to": "finish" } ]),
    );

    let reasons: Vec<String> = params_errors(&spec)
        .into_iter()
        .map(|error| match error.kind {
            ValidationErrorKind::InvalidParams { reason, .. } => reason,
            _ => unreachable!(),
        })
        .collect();

    assert_eq!(
        reasons,
        ["/extra: unknown parameter; /retries: expected integer"]
    );
}

#[test]
fn rejects_params_of_conditions_without_parameters() {
    let spec = spec(
        json!([ { "id": "call_llm", "kind": "llm_request" } ]),
        json!([ { "from": "start", "to": "call_llm" },
                { "from": "call_llm", "to": "finish", "when": { "kind": "on_tool_call", "params": { "x": 1 } } } ]),
    );

    let errors = spec.validate().err().unwrap_or_default();

    assert!(
        errors.contains(&ValidationError {
            path: "/graph/edges/1/when/params".into(),
            kind: ValidationErrorKind::InvalidParams {
                kind: "on_tool_call".into(),
                reason: "/x: unknown parameter".into(),
            },
        }),
        "{errors:?}"
    );
}
