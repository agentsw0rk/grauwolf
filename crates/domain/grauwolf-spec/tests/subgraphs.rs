//! Validation of subgraphs (ADR-0034).

use grauwolf_graph::DataType;
use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ValidationError, ValidationErrorKind};
use serde_json::{Value, json};

/// Spec with `subgraphs` and a main graph; tool `add` and type `Answer` are declared.
fn spec(subgraphs: Value, graph: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "types": { "Answer": { "type": "object" } },
            "tools": [ { "name": "add", "description": "Addiert", "args": { "type": "object" }, "code": "Ok(String::new())" } ],
            "subgraphs": subgraphs,
            "graph": graph
        })
        .to_string(),
    )
    .unwrap()
}

/// A valid subgraph `solve` (string → string) with a tool subset.
fn solve() -> Value {
    json!({
        "input": "string", "output": "string", "tools": ["add"],
        "graph": {
            "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
            "edges": [ { "from": "start", "to": "call_llm" },
                       { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
        }
    })
}

/// Main graph that includes `name` as a subgraph.
fn calling(name: &str) -> Value {
    json!({
        "nodes": [ { "id": "solve", "kind": "subgraph", "params": { "subgraph": name } } ],
        "edges": [ { "from": "start", "to": "solve" }, { "from": "solve", "to": "finish" } ]
    })
}

fn errors(spec: &AgentSpec) -> Vec<ValidationError> {
    spec.validate().err().unwrap_or_default()
}

#[test]
fn accepts_a_spec_that_calls_a_valid_subgraph() {
    assert_eq!(
        spec(json!({ "solve": solve() }), calling("solve")).validate(),
        Ok(())
    );
}

#[test]
fn checks_subgraph_graphs_with_their_own_paths_and_signature() {
    let mut broken = solve();
    broken["input"] = json!("Answer");
    broken["graph"]["edges"][1]["to"] = json!("missing");

    let errors = errors(&spec(json!({ "solve": broken }), calling("solve")));

    assert!(
        errors.contains(&ValidationError {
            path: "/subgraphs/solve/graph/edges/1/to".into(),
            kind: ValidationErrorKind::UnknownNode {
                id: "missing".into()
            },
        }),
        "{errors:?}"
    );
    assert!(
        errors.contains(&ValidationError {
            path: "/subgraphs/solve/graph/edges/0".into(),
            kind: ValidationErrorKind::TypeMismatch {
                expected: DataType::String,
                found: DataType::Named("Answer".into()),
            },
        }),
        "{errors:?}"
    );
}

#[test]
fn types_subgraph_nodes_by_the_signature_of_their_subgraph() {
    let mut typed = solve();
    typed["output"] = json!("Answer");
    typed["graph"] = json!({ "edges": [] });

    let errors = errors(&spec(json!({ "solve": typed }), calling("solve")));

    assert!(
        errors.contains(&ValidationError {
            path: "/graph/edges/1".into(),
            kind: ValidationErrorKind::TypeMismatch {
                expected: DataType::String,
                found: DataType::Named("Answer".into()),
            },
        }),
        "{errors:?}"
    );
}

#[test]
fn reports_unknown_subgraphs_tools_and_names() {
    let mut with_unknown_tool = solve();
    with_unknown_tool["tools"] = json!(["add", "mul"]);

    let errors = errors(&spec(
        json!({ "solve": with_unknown_tool, "Bad-Name": solve() }),
        calling("missing"),
    ));

    for expected in [
        ValidationError {
            path: "/graph/nodes/0/params".into(),
            kind: ValidationErrorKind::UnknownSubgraph {
                name: "missing".into(),
            },
        },
        ValidationError {
            path: "/subgraphs/solve/tools/1".into(),
            kind: ValidationErrorKind::UnknownTool { name: "mul".into() },
        },
        ValidationError {
            path: "/subgraphs/Bad-Name".into(),
            kind: ValidationErrorKind::InvalidId {
                id: "Bad-Name".into(),
            },
        },
    ] {
        assert!(
            errors.contains(&expected),
            "{expected:?} missing in {errors:?}"
        );
    }
}

#[test]
fn reports_subgraphs_that_include_themselves() {
    let looping = json!({
        "graph": {
            "nodes": [ { "id": "again", "kind": "subgraph", "params": { "subgraph": "a" } } ],
            "edges": [ { "from": "start", "to": "again" }, { "from": "again", "to": "finish" } ]
        }
    });

    let errors = errors(&spec(json!({ "a": looping }), calling("a")));

    assert!(
        errors.contains(&ValidationError {
            path: "/subgraphs/a/graph/nodes/0/params".into(),
            kind: ValidationErrorKind::SubgraphCycle { name: "a".into() },
        }),
        "{errors:?}"
    );
}

#[test]
fn lists_every_provider_the_spec_uses() {
    let spec = AgentSpec::from_json(
        &serde_json::json!({
            "spec_version": 1, "name": "mixed",
            "model": { "provider": "ollama", "id": "klein" },
            "subgraphs": { "answer": {
                "model": { "provider": "anthropic", "id": "gross" },
                "graph": { "edges": [ { "from": "start", "to": "finish" } ] }
            } },
            "graph": {
                "nodes": [
                    { "id": "switch", "kind": "set_model", "params": { "provider": "openai", "id": "mittel", "value": "string" } },
                    { "id": "answer", "kind": "subgraph", "params": { "subgraph": "answer" } }
                ],
                "edges": [ { "from": "start", "to": "switch" }, { "from": "switch", "to": "answer" }, { "from": "answer", "to": "finish" } ]
            }
        })
        .to_string(),
    )
    .unwrap();

    assert_eq!(spec.providers(), ["anthropic", "ollama", "openai"]);
    assert_eq!(spec.validate(), Ok(()));
}
