use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ConditionSpec, EdgeSpec, NodeSpec, ToolSpec};
use serde_json::json;

/// Builds spec JSON with the required fields; `extra` are additional fields (without a leading comma).
fn spec_json(model: &str, extra: &str) -> String {
    let sep = if extra.is_empty() { "" } else { "," };
    format!(r#"{{ "spec_version": 1, "name": "echo", "model": {model}{sep} {extra} }}"#)
}

const OLLAMA: &str = r#"{ "provider": "ollama", "id": "llama3.2" }"#;

#[test]
fn parses_name_and_version_from_json() {
    let spec = AgentSpec::from_json(&spec_json(OLLAMA, "")).unwrap();

    assert_eq!(spec.spec_version, 1);
    assert_eq!(spec.name, "echo");
}

#[test]
fn parses_model_provider_and_id() {
    let spec = AgentSpec::from_json(&spec_json(
        r#"{ "provider": "openai", "id": "gpt-4o" }"#,
        "",
    ))
    .unwrap();

    assert_eq!(spec.model.provider, "openai");
    assert_eq!(spec.model.id, "gpt-4o");
}

#[test]
fn accepts_custom_provider_names() {
    let spec = AgentSpec::from_json(&spec_json(
        r#"{ "provider": "my-gateway", "id": "internal-model" }"#,
        "",
    ))
    .unwrap();

    assert_eq!(spec.model.provider, "my-gateway");
}

#[test]
fn parses_system_prompt() {
    let spec =
        AgentSpec::from_json(&spec_json(OLLAMA, r#""system_prompt": "You are helpful.""#)).unwrap();

    assert_eq!(spec.system_prompt.as_deref(), Some("You are helpful."));
}

#[test]
fn parses_node_kind_and_params() {
    let spec = AgentSpec::from_json(&spec_json(
        OLLAMA,
        r#""graph": { "nodes": [
            { "id": "call_llm", "kind": "llm_request" },
            { "id": "answer", "kind": "llm_request_structured", "params": { "type": "Answer" } }
        ] }"#,
    ))
    .unwrap();

    assert_eq!(
        spec.graph.nodes,
        vec![
            NodeSpec {
                id: "call_llm".into(),
                kind: "llm_request".into(),
                params: None,
            },
            NodeSpec {
                id: "answer".into(),
                kind: "llm_request_structured".into(),
                params: Some(json!({ "type": "Answer" })),
            },
        ]
    );
}

#[test]
fn parses_edges_between_start_node_and_finish() {
    let spec = AgentSpec::from_json(&spec_json(
        OLLAMA,
        r#""graph": {
            "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
            "edges": [ { "from": "start", "to": "call_llm" }, { "from": "call_llm", "to": "finish" } ]
        }"#,
    ))
    .unwrap();

    assert_eq!(
        spec.graph.edges,
        vec![
            EdgeSpec {
                from: "start".into(),
                transform: None,
                to: "call_llm".into(),
                when: None,
            },
            EdgeSpec {
                from: "call_llm".into(),
                transform: None,
                to: "finish".into(),
                when: None,
            },
        ]
    );
}

#[test]
fn parses_edge_condition_as_object() {
    let spec = AgentSpec::from_json(&spec_json(
        OLLAMA,
        r#""graph": {
            "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
            "edges": [ { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
        }"#,
    ))
    .unwrap();

    assert_eq!(
        spec.graph.edges[0].when,
        Some(ConditionSpec {
            kind: "on_assistant_message".into(),
            params: None,
        })
    );
}

#[test]
fn parses_tools_with_schema_and_code() {
    let spec = AgentSpec::from_json(&spec_json(
        OLLAMA,
        r#""tools": [ {
            "name": "add",
            "description": "Addiert zwei Zahlen",
            "args": { "type": "object", "properties": { "a": { "type": "number" } } },
            "code": "Ok(args.to_string())"
        } ]"#,
    ))
    .unwrap();

    assert_eq!(
        spec.tools,
        vec![ToolSpec {
            name: "add".into(),
            description: "Addiert zwei Zahlen".into(),
            args: json!({ "type": "object", "properties": { "a": { "type": "number" } } }),
            code: "Ok(args.to_string())".into(),
        }]
    );
}

#[test]
fn parses_named_types_and_agent_signature() {
    let spec = AgentSpec::from_json(&spec_json(
        OLLAMA,
        r#""types": { "Answer": { "type": "object", "properties": { "text": { "type": "string" } } } },
           "input": "string", "output": "Answer""#,
    ))
    .unwrap();

    assert_eq!(
        spec.types.get("Answer"),
        Some(&json!({ "type": "object", "properties": { "text": { "type": "string" } } }))
    );
    assert_eq!(spec.input, "string");
    assert_eq!(spec.output, "Answer");
}

#[test]
fn agent_signature_defaults_to_string() {
    let spec = AgentSpec::from_json(&spec_json(OLLAMA, "")).unwrap();

    assert!(spec.types.is_empty());
    assert_eq!(
        (spec.input.as_str(), spec.output.as_str()),
        ("string", "string")
    );
}

#[test]
fn parses_named_subgraphs_with_signature_tools_and_graph() {
    let spec = AgentSpec::from_json(&spec_json(
        OLLAMA,
        r#""subgraphs": { "solve": {
               "input": "string", "output": "string", "tools": ["add"],
               "graph": { "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
                          "edges": [ { "from": "start", "to": "call_llm" } ] } } },
           "graph": { "nodes": [ { "id": "solve", "kind": "subgraph", "params": { "subgraph": "solve" } } ] }"#,
    ))
    .unwrap();

    let solve = &spec.subgraphs["solve"];
    assert_eq!(
        (solve.input.as_str(), solve.output.as_str()),
        ("string", "string")
    );
    assert_eq!(solve.tools.as_deref(), Some(&["add".to_string()][..]));
    assert_eq!(solve.graph.nodes[0].kind, "llm_request");
    assert_eq!(spec.graph.nodes[0].kind, "subgraph");
}
