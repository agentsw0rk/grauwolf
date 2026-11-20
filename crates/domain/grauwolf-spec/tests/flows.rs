//! Multi-agent flows in the spec: `agent` and `for_each` nodes (ADR-0101).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ValidationError, ValidationErrorKind};
use serde_json::{Value, json};

fn errors(spec: Value) -> Vec<ValidationError> {
    AgentSpec::from_json(&spec.to_string())
        .unwrap()
        .validate()
        .err()
        .unwrap_or_default()
}

/// A flow of two agents; `agent` is the node parameter, `types` the declared types.
fn flow(node_params: Value) -> Value {
    json!({
        "spec_version": 1, "name": "flow",
        "model": { "provider": "ollama", "id": "m" },
        "types": {
            "Topic": { "type": "object", "properties": { "title": { "type": "string" } }, "required": ["title"] }
        },
        "agent_tools": {
            "researcher": { "spec": "researcher.json", "description": "Finds topics" }
        },
        "graph": {
            "nodes": [ { "id": "research", "kind": "agent", "params": node_params } ],
            "edges": [
                { "from": "start", "to": "research" },
                { "from": "research", "to": "finish" }
            ]
        }
    })
}

#[test]
fn an_agent_node_naming_a_sub_agent_is_valid() {
    assert_eq!(errors(flow(json!({ "agent": "researcher" }))), []);
}

#[test]
fn an_agent_node_must_name_an_entry_of_agent_tools() {
    let found = errors(flow(json!({ "agent": "ghost" })));

    assert_eq!(
        found,
        [ValidationError {
            path: "/graph/nodes/0/params/agent".into(),
            kind: ValidationErrorKind::UnknownAgent {
                name: "ghost".into()
            },
        }]
    );
}

#[test]
fn an_agent_node_declares_only_known_types() {
    let found = errors(flow(json!({ "agent": "researcher", "input": "Nope" })));

    assert!(
        found.iter().any(|error| error.path == "/graph/nodes/0/params"
            && matches!(&error.kind, ValidationErrorKind::UnknownType { name } if name == "Nope")),
        "{found:?}"
    );
}

/// A spec whose main graph is `start → each → finish` with text in and out; `write` is a
/// subgraph `Topic → Article`.
fn each(params: Value) -> Value {
    json!({
        "spec_version": 1, "name": "flow",
        "model": { "provider": "ollama", "id": "m" },
        "types": {
            "Topic": { "type": "object", "properties": { "title": { "type": "string" } }, "required": ["title"] },
            "Article": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }
        },
        "agent_tools": { "writer": { "spec": "writer.json", "description": "Writes" } },
        "subgraphs": {
            "write": {
                "input": "Topic", "output": "Article",
                "graph": {
                    "nodes": [
                        { "id": "pick", "kind": "get_field", "params": { "type": "Topic", "field": "title" } },
                        { "id": "ask", "kind": "llm_request_structured", "params": { "type": "Article" } }
                    ],
                    "edges": [
                        { "from": "start", "to": "pick" },
                        { "from": "pick", "to": "ask" },
                        { "from": "ask", "to": "finish" }
                    ]
                }
            }
        },
        "graph": {
            "nodes": [ { "id": "each", "kind": "for_each", "params": params } ],
            "edges": [ { "from": "start", "to": "each" }, { "from": "each", "to": "finish" } ]
        }
    })
}

fn text_each(body: Value) -> Value {
    each(json!({ "input": "string", "output": "string", "body": body }))
}

/// The errors at the `each` node (the `write` subgraph is not well-formed in every test).
fn node_errors(spec: Value) -> Vec<ValidationError> {
    errors(spec)
        .into_iter()
        .filter(|error| error.path.starts_with("/graph/nodes/0"))
        .collect()
}

fn invalid(reason: &str) -> ValidationError {
    ValidationError {
        path: "/graph/nodes/0/params".into(),
        kind: ValidationErrorKind::InvalidParams {
            kind: "for_each".into(),
            reason: reason.into(),
        },
    }
}

#[test]
fn a_for_each_node_with_an_agent_or_a_subgraph_body_is_valid() {
    assert_eq!(
        errors(each(
            json!({ "input": "string", "output": "string", "body": { "agent": "writer" },
                           "max_parallel": 16, "on_error": "skip" })
        )),
        []
    );
    assert_eq!(errors(text_each(json!({ "subgraph": "write" }))), []);
}

#[test]
fn the_body_must_name_a_known_agent_or_subgraph() {
    assert_eq!(
        node_errors(text_each(json!({ "agent": "ghost" }))),
        [ValidationError {
            path: "/graph/nodes/0/params/body/agent".into(),
            kind: ValidationErrorKind::UnknownAgent {
                name: "ghost".into()
            },
        }]
    );
    assert_eq!(
        node_errors(text_each(json!({ "subgraph": "ghost" }))),
        [ValidationError {
            path: "/graph/nodes/0/params/body/subgraph".into(),
            kind: ValidationErrorKind::UnknownSubgraph {
                name: "ghost".into()
            },
        }]
    );
}

#[test]
fn max_parallel_is_from_1_to_16() {
    for bad in [0, 17] {
        let spec = each(json!({ "input": "string", "output": "string",
                                "body": { "agent": "writer" }, "max_parallel": bad }));
        assert_eq!(
            node_errors(spec),
            [invalid("\"max_parallel\" must be from 1 to 16")]
        );
    }
}

#[test]
fn on_error_is_fail_or_skip() {
    let spec = each(json!({ "input": "string", "output": "string",
                            "body": { "agent": "writer" }, "on_error": "ignore" }));

    let found = node_errors(spec);

    assert_eq!(found.len(), 1);
    assert!(
        matches!(&found[0].kind, ValidationErrorKind::InvalidParams { reason, .. } if reason.contains("ignore")),
        "{found:?}"
    );
}

#[test]
fn the_input_is_a_list_or_text_and_the_output_a_list_or_text() {
    let spec = each(json!({ "input": "Topic", "output": "string", "body": { "agent": "writer" } }));
    assert_eq!(
        node_errors(spec),
        [invalid("\"input\" must be array<T> or string")]
    );
    let spec =
        each(json!({ "input": "string", "output": "Article", "body": { "agent": "writer" } }));
    assert_eq!(
        node_errors(spec),
        [invalid("\"output\" must be array<T> or string")]
    );
}

#[test]
fn the_element_types_of_lists_must_be_declared() {
    let spec =
        each(json!({ "input": "array<Nope>", "output": "string", "body": { "agent": "writer" } }));

    assert_eq!(
        node_errors(spec),
        [ValidationError {
            path: "/graph/nodes/0/params/input".into(),
            kind: ValidationErrorKind::UnknownType {
                name: "Nope".into()
            },
        }]
    );
}

#[test]
fn a_subgraph_body_must_take_and_return_the_element_types() {
    let spec = each(json!({ "input": "array<Article>", "output": "array<Topic>",
                            "body": { "subgraph": "write" } }));

    let found = node_errors(spec);

    assert_eq!(
        found,
        [
            invalid("the subgraph 'write' takes Topic, but the elements of the input are Article"),
            invalid(
                "the subgraph 'write' returns Article, but the elements of the output are Topic"
            ),
        ]
    );
}

#[test]
fn the_list_types_of_the_node_are_checked_on_its_edges() {
    let spec = each(
        json!({ "input": "string", "output": "array<string>", "body": { "agent": "writer" } }),
    );

    let found = errors(spec);

    assert!(
        found.iter().any(|error| error.path == "/graph/edges/1"
            && matches!(&error.kind, ValidationErrorKind::TypeMismatch { expected, found }
                if expected.to_string() == "string" && found.to_string() == "array<string>")),
        "{found:?}"
    );
}

#[test]
fn for_each_nodes_nest_but_a_body_may_not_include_itself() {
    let mut spec = text_each(json!({ "subgraph": "inner" }));
    spec["subgraphs"]["inner"] = json!({
        "input": "string", "output": "string",
        "graph": {
            "nodes": [ { "id": "again", "kind": "for_each", "params": {
                "input": "string", "output": "string", "body": { "subgraph": "write_text" } } } ],
            "edges": [ { "from": "start", "to": "again" }, { "from": "again", "to": "finish" } ]
        }
    });
    spec["subgraphs"]["write_text"] = json!({
        "input": "string", "output": "string",
        "graph": {
            "nodes": [ { "id": "ask", "kind": "agent", "params": { "agent": "writer" } } ],
            "edges": [ { "from": "start", "to": "ask" }, { "from": "ask", "to": "finish" } ]
        }
    });
    assert_eq!(errors(spec.clone()), []);

    spec["subgraphs"]["write_text"]["graph"]["nodes"][0] = json!({
        "id": "ask", "kind": "for_each",
        "params": { "input": "string", "output": "string", "body": { "subgraph": "inner" } }
    });
    let found = errors(spec);

    assert!(
        found
            .iter()
            .any(|error| matches!(&error.kind, ValidationErrorKind::SubgraphCycle { .. })),
        "{found:?}"
    );
}

#[test]
fn the_diagram_links_a_for_each_node_to_the_subgraph_of_its_body() {
    let spec =
        AgentSpec::from_json(&text_each(json!({ "subgraph": "write" })).to_string()).unwrap();

    let diagram = spec.to_mermaid();

    assert!(diagram.contains("    each -.-> sg_write\n"), "{diagram}");
}

fn signatures(
    entries: &[(&str, &str, &str)],
) -> std::collections::BTreeMap<String, (String, String)> {
    entries
        .iter()
        .map(|(name, input, output)| ((*name).into(), ((*input).into(), (*output).into())))
        .collect()
}

#[test]
fn declared_agent_types_must_match_the_specs_of_the_sub_agents() {
    let spec =
        AgentSpec::from_json(&flow(json!({ "agent": "researcher", "input": "Topic" })).to_string())
            .unwrap();

    let same = spec.check_sub_agent_types(&signatures(&[("researcher", "Topic", "string")]));
    let other = spec.check_sub_agent_types(&signatures(&[("researcher", "string", "Topic")]));

    assert_eq!(same, []);
    assert_eq!(
        other,
        [
            ValidationError {
                path: "/graph/nodes/0/params/input".into(),
                kind: ValidationErrorKind::SubAgentTypeMismatch {
                    agent: "researcher".into(),
                    field: "input".into(),
                    declared: "Topic".into(),
                    actual: "string".into(),
                },
            },
            ValidationError {
                path: "/graph/nodes/0/params/output".into(),
                kind: ValidationErrorKind::SubAgentTypeMismatch {
                    agent: "researcher".into(),
                    field: "output".into(),
                    declared: "string".into(),
                    actual: "Topic".into(),
                },
            },
        ]
    );
}

#[test]
fn the_elements_of_for_each_lists_are_the_types_of_an_agent_body() {
    let spec = AgentSpec::from_json(
        &each(json!({ "input": "array<Topic>", "output": "array<Article>", "body": { "agent": "writer" } })).to_string(),
    )
    .unwrap();

    let found = spec.check_sub_agent_types(&signatures(&[("writer", "Topic", "string")]));

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, "/graph/nodes/0/params/output");
}
