use grauwolf_graph::DataType;
use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ToolOrigin, ValidationError, ValidationErrorKind};

fn spec_with_graph(graph: &str) -> AgentSpec {
    AgentSpec::from_json(&format!(
        r#"{{ "spec_version": 1, "name": "echo",
              "model": {{ "provider": "ollama", "id": "llama3.2" }},
              "graph": {graph} }}"#
    ))
    .unwrap()
}

/// Errors other than reachability errors: the tests here each check one error at a deliberately
/// incomplete graph; reachability is checked by `tests/reachability.rs`.
fn errors_except_reachability(spec: &AgentSpec) -> Vec<ValidationError> {
    let errors: Vec<ValidationError> = spec
        .validate()
        .unwrap_err()
        .into_iter()
        .filter(|error| {
            !matches!(
                error.kind,
                ValidationErrorKind::FinishUnreachable
                    | ValidationErrorKind::UnreachableNode { .. }
                    | ValidationErrorKind::DeadEnd { .. }
            )
        })
        .collect();
    assert!(!errors.is_empty(), "only reachability errors");
    errors
}

#[test]
fn reports_edge_to_unknown_node() {
    let spec = spec_with_graph(
        r#"{ "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
             "edges": [ { "from": "start", "to": "missing" },
                        { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } } ] }"#,
    );

    let errors = errors_except_reachability(&spec);

    assert_eq!(
        errors,
        vec![ValidationError {
            path: "/graph/edges/0/to".into(),
            kind: ValidationErrorKind::UnknownNode {
                id: "missing".into()
            },
        }]
    );
}

#[test]
fn reports_edge_from_unknown_node() {
    let spec = spec_with_graph(
        r#"{ "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
             "edges": [ { "from": "start", "to": "call_llm" }, { "from": "ghost", "to": "finish" } ] }"#,
    );

    let errors = errors_except_reachability(&spec);

    assert_eq!(
        errors,
        vec![ValidationError {
            path: "/graph/edges/1/from".into(),
            kind: ValidationErrorKind::UnknownNode { id: "ghost".into() },
        }]
    );
}

#[test]
fn reports_reserved_node_ids() {
    let spec = spec_with_graph(
        r#"{ "nodes": [ { "id": "start", "kind": "llm_request" }, { "id": "finish", "kind": "llm_request" } ] }"#,
    );

    let errors = errors_except_reachability(&spec);

    assert_eq!(
        errors,
        vec![
            ValidationError {
                path: "/graph/nodes/0/id".into(),
                kind: ValidationErrorKind::ReservedNodeId { id: "start".into() },
            },
            ValidationError {
                path: "/graph/nodes/1/id".into(),
                kind: ValidationErrorKind::ReservedNodeId {
                    id: "finish".into()
                },
            },
        ]
    );
}

#[test]
fn reports_node_ids_that_are_not_snake_case_identifiers() {
    let spec = spec_with_graph(
        r#"{ "nodes": [
            { "id": "call_llm2", "kind": "llm_request" },
            { "id": "CallLlm", "kind": "llm_request" },
            { "id": "2nd", "kind": "llm_request" },
            { "id": "with-dash", "kind": "llm_request" },
            { "id": "", "kind": "llm_request" }
        ] }"#,
    );

    let paths: Vec<String> = spec
        .validate()
        .unwrap_err()
        .into_iter()
        .filter(|error| matches!(error.kind, ValidationErrorKind::InvalidId { .. }))
        .map(|error| error.path)
        .collect();

    assert_eq!(
        paths,
        [
            "/graph/nodes/1/id",
            "/graph/nodes/2/id",
            "/graph/nodes/3/id",
            "/graph/nodes/4/id"
        ]
    );
}

#[test]
fn reports_duplicate_node_ids() {
    let spec = spec_with_graph(
        r#"{ "nodes": [
            { "id": "call_llm", "kind": "llm_request" },
            { "id": "other", "kind": "llm_request" },
            { "id": "call_llm", "kind": "llm_request" }
        ] }"#,
    );

    assert_eq!(
        errors_except_reachability(&spec),
        vec![ValidationError {
            path: "/graph/nodes/2/id".into(),
            kind: ValidationErrorKind::DuplicateNodeId {
                id: "call_llm".into()
            },
        }]
    );
}

#[test]
fn reports_node_kinds_missing_from_the_catalog() {
    let spec = spec_with_graph(r#"{ "nodes": [ { "id": "call_llm", "kind": "llm_reqest" } ] }"#);

    assert_eq!(
        errors_except_reachability(&spec),
        vec![ValidationError {
            path: "/graph/nodes/0/kind".into(),
            kind: ValidationErrorKind::UnknownNodeKind {
                kind: "llm_reqest".into()
            },
        }]
    );
}

#[test]
fn reports_condition_kinds_missing_from_the_catalog() {
    let spec = spec_with_graph(
        r#"{ "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
             "edges": [ { "from": "call_llm", "to": "finish", "when": { "kind": "on_asistant_message" } } ] }"#,
    );

    assert_eq!(
        errors_except_reachability(&spec),
        vec![ValidationError {
            path: "/graph/edges/0/when/kind".into(),
            kind: ValidationErrorKind::UnknownConditionKind {
                kind: "on_asistant_message".into()
            },
        }]
    );
}

#[test]
fn reports_edges_whose_value_type_does_not_fit_the_target() {
    let spec = spec_with_graph(
        r#"{ "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
             "edges": [ { "from": "start", "to": "call_llm" }, { "from": "call_llm", "to": "finish" } ] }"#,
    );

    assert_eq!(
        errors_except_reachability(&spec),
        vec![ValidationError {
            path: "/graph/edges/1".into(),
            kind: ValidationErrorKind::TypeMismatch {
                expected: DataType::String,
                found: DataType::Message,
            },
        }]
    );
}

#[test]
fn reports_conditions_that_do_not_accept_the_edge_value() {
    let spec = spec_with_graph(
        r#"{ "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
             "edges": [ { "from": "start", "to": "call_llm", "when": { "kind": "on_assistant_message" } } ] }"#,
    );

    assert_eq!(
        errors_except_reachability(&spec),
        vec![ValidationError {
            path: "/graph/edges/0/when".into(),
            kind: ValidationErrorKind::ConditionInputMismatch {
                kind: "on_assistant_message".into(),
                expected: DataType::Message,
                found: DataType::String,
            },
        }]
    );
}

#[test]
fn reports_names_that_cannot_become_crate_names() {
    let spec = AgentSpec::from_json(
        r#"{ "spec_version": 1, "name": "Echo Bot", "model": { "provider": "ollama", "id": "m" } }"#,
    )
    .unwrap();

    assert_eq!(
        errors_except_reachability(&spec),
        vec![ValidationError {
            path: "/name".into(),
            kind: ValidationErrorKind::InvalidId {
                id: "Echo Bot".into()
            },
        }]
    );
}

#[test]
fn reports_invalid_and_duplicate_tool_names() {
    let spec = AgentSpec::from_json(
        r#"{ "spec_version": 1, "name": "calc", "model": { "provider": "ollama", "id": "m" },
             "tools": [
               { "name": "Add", "description": "d", "args": {}, "code": "" },
               { "name": "sub", "description": "d", "args": {}, "code": "" },
               { "name": "sub", "description": "d", "args": {}, "code": "" }
             ] }"#,
    )
    .unwrap();

    assert_eq!(
        errors_except_reachability(&spec),
        vec![
            ValidationError {
                path: "/tools/0/name".into(),
                kind: ValidationErrorKind::InvalidId { id: "Add".into() },
            },
            ValidationError {
                path: "/tools/2/name".into(),
                kind: ValidationErrorKind::DuplicateToolName {
                    name: "sub".into(),
                    other_origin: ToolOrigin::Own,
                    other_pointer: "/tools/1/name".into()
                },
            },
        ]
    );
}

#[test]
fn reports_agent_signature_with_unknown_type() {
    let spec = AgentSpec::from_json(
        r#"{ "spec_version": 1, "name": "echo",
             "model": { "provider": "ollama", "id": "llama3.2" },
             "types": { "Answer": { "type": "object" } },
             "input": "Question", "output": "Answer",
             "graph": { "edges": [ { "from": "start", "to": "finish" } ] } }"#,
    )
    .unwrap();

    let errors = errors_except_reachability(&spec);

    assert!(
        errors.contains(&ValidationError {
            path: "/input".into(),
            kind: ValidationErrorKind::UnknownType {
                name: "Question".into()
            },
        }),
        "{errors:?}"
    );
}

#[test]
fn reports_type_names_that_are_not_pascal_case() {
    let spec = AgentSpec::from_json(
        r#"{ "spec_version": 1, "name": "echo",
             "model": { "provider": "ollama", "id": "llama3.2" },
             "types": { "Answer": { "type": "object" }, "my/answer": { "type": "object" } } }"#,
    )
    .unwrap();

    let errors = errors_except_reachability(&spec);

    assert_eq!(
        errors,
        vec![ValidationError {
            path: "/types/my~1answer".into(),
            kind: ValidationErrorKind::InvalidTypeName {
                name: "my/answer".into()
            },
        }]
    );
}

#[test]
fn checks_edges_against_the_declared_agent_signature() {
    let spec = AgentSpec::from_json(
        r#"{ "spec_version": 1, "name": "echo",
             "model": { "provider": "ollama", "id": "llama3.2" },
             "types": { "Question": { "type": "object" } },
             "input": "Question",
             "graph": { "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
                        "edges": [ { "from": "start", "to": "call_llm" },
                                   { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } } ] } }"#,
    )
    .unwrap();

    let errors = errors_except_reachability(&spec);

    assert_eq!(
        errors,
        vec![ValidationError {
            path: "/graph/edges/0".into(),
            kind: ValidationErrorKind::TypeMismatch {
                expected: DataType::String,
                found: DataType::Named("Question".into()),
            },
        }]
    );
}

#[test]
fn accepts_named_types_flowing_from_start_to_finish() {
    let spec = AgentSpec::from_json(
        r#"{ "spec_version": 1, "name": "echo",
             "model": { "provider": "ollama", "id": "llama3.2" },
             "types": { "Answer": { "type": "object" } },
             "input": "Answer", "output": "Answer",
             "graph": { "edges": [ { "from": "start", "to": "finish" } ] } }"#,
    )
    .unwrap();

    assert_eq!(spec.validate(), Ok(()));
}

#[test]
fn reports_nodes_with_invalid_params() {
    let spec = spec_with_graph(
        r#"{ "nodes": [ { "id": "triage", "kind": "llm_request_structured" } ],
             "edges": [ { "from": "start", "to": "triage" } ] }"#,
    );

    let errors = errors_except_reachability(&spec);

    assert!(
        matches!(
            &errors[..],
            [ValidationError { path, kind: ValidationErrorKind::InvalidParams { kind, .. } }]
                if path == "/graph/nodes/0/params" && kind == "llm_request_structured"
        ),
        "{errors:?}"
    );
}

#[test]
fn reports_nodes_that_produce_undeclared_types() {
    let spec = spec_with_graph(
        r#"{ "nodes": [ { "id": "triage", "kind": "llm_request_structured", "params": { "type": "Ticket" } } ],
             "edges": [ { "from": "start", "to": "triage" } ] }"#,
    );

    let errors = errors_except_reachability(&spec);

    assert_eq!(
        errors,
        vec![ValidationError {
            path: "/graph/nodes/0/params".into(),
            kind: ValidationErrorKind::UnknownType {
                name: "Ticket".into()
            },
        }]
    );
}

/// ADR-0080: every place that names a provider, with its pointer – for the check in the CLI.
#[test]
fn lists_every_provider_with_its_place() {
    let spec = AgentSpec::from_json(
        &serde_json::json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "olama", "id": "m" },
            "knowledge": { "sources": ["docs"], "embedding_model": { "provider": "ollama", "id": "e" } },
            "subgraphs": { "zweit": {
                "model": { "provider": "openai", "id": "x" },
                "graph": { "nodes": [ { "id": "wechsel", "kind": "set_model",
                                        "params": { "provider": "anthropic", "id": "y", "value": "string" } } ],
                           "edges": [ { "from": "start", "to": "wechsel" }, { "from": "wechsel", "to": "finish" } ] } } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    let uses: Vec<(String, &str)> = spec.provider_uses().into_iter().collect();

    assert_eq!(
        uses,
        [
            ("/model/provider".to_owned(), "olama"),
            ("/subgraphs/zweit/model/provider".to_owned(), "openai"),
            (
                "/subgraphs/zweit/graph/nodes/0/params/provider".to_owned(),
                "anthropic"
            ),
            ("/knowledge/embedding_model/provider".to_owned(), "ollama"),
        ]
    );
}
