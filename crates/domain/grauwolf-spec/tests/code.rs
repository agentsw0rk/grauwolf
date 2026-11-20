//! Code in graphs: node `code`, condition `on_code`, edge `transform` (ADR-0102).

use grauwolf_graph::DataType;
use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, CodePieceKind, ToolOrigin, ValidationError, ValidationErrorKind};
use serde_json::{Value, json};

fn load(spec: Value) -> AgentSpec {
    AgentSpec::from_json(&spec.to_string()).unwrap()
}

fn errors(spec: Value) -> Vec<ValidationError> {
    load(spec).validate().err().unwrap_or_default()
}

fn error(path: &str, kind: ValidationErrorKind) -> ValidationError {
    ValidationError {
        path: path.into(),
        kind,
    }
}

fn named(name: &str) -> DataType {
    DataType::Named(name.into())
}

/// start → score (code: string → Score) → [on_code Score] → format (transform Score → Report) → finish
fn spec() -> Value {
    json!({
        "spec_version": 1, "name": "scored",
        "model": { "provider": "ollama", "id": "m" },
        "types": {
            "Score": { "type": "object", "properties": { "points": { "type": "integer" } }, "required": ["points"] },
            "Report": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }
        },
        "input": "string", "output": "Report",
        "graph": {
            "nodes": [
                { "id": "score", "kind": "code", "params": {
                    "input": "string", "output": "Score",
                    "code": "Ok(Score { points: input.len() as i64 })" } },
                { "id": "report", "kind": "code", "params": {
                    "input": "Report", "output": "Report", "code": "Ok(input)" } }
            ],
            "edges": [
                { "from": "start", "to": "score" },
                { "from": "score", "to": "report",
                  "when": { "kind": "on_code", "params": { "input": "Score", "code": "value.points > 3" } },
                  "transform": { "output": "Report", "code": "Report { text: value.points.to_string() }" } },
                { "from": "report", "to": "finish" }
            ]
        }
    })
}

#[test]
fn code_nodes_conditions_and_transformations_are_valid() {
    assert_eq!(errors(spec()), []);
}

#[test]
fn a_spec_with_code_survives_a_json_round_trip() {
    let loaded = load(spec());
    assert_eq!(AgentSpec::from_json(&loaded.to_json()).unwrap(), loaded);
}

#[test]
fn code_pieces_have_stable_names_pointers_and_types() {
    let pieces = load(spec()).code_pieces();

    let summary: Vec<_> = pieces
        .iter()
        .map(|piece| {
            (
                piece.name.as_str(),
                piece.kind,
                piece.pointer.as_str(),
                piece.input.as_str(),
                piece.output.as_str(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (
                "code_main_score",
                CodePieceKind::Node,
                "/graph/nodes/0/params/code",
                "string",
                "Score"
            ),
            (
                "code_main_report",
                CodePieceKind::Node,
                "/graph/nodes/1/params/code",
                "Report",
                "Report"
            ),
            (
                "condition_main_score_report",
                CodePieceKind::Condition,
                "/graph/edges/1/when/params/code",
                "Score",
                "bool"
            ),
            (
                "transform_main_score_report",
                CodePieceKind::Transform,
                "/graph/edges/1/transform/code",
                "Score",
                "Report"
            ),
        ]
    );
    assert_eq!(
        pieces[3].types.keys().collect::<Vec<_>>(),
        ["Report", "Score"]
    );
}

#[test]
fn code_pieces_of_subgraphs_and_parallel_edges_get_distinct_names() {
    let mut value = spec();
    value["subgraphs"] = json!({
        "inner": {
            "input": "string", "output": "string",
            "graph": {
                "nodes": [ { "id": "echo", "kind": "code", "params": { "input": "string", "output": "string", "code": "Ok(input)" } } ],
                "edges": [ { "from": "start", "to": "echo" }, { "from": "echo", "to": "finish" } ]
            }
        }
    });
    value["graph"]["edges"].as_array_mut().unwrap().push(json!({
        "from": "score", "to": "report",
        "transform": { "output": "Report", "code": "Report { text: String::new() }" }
    }));
    value["graph"]["edges"].as_array_mut().unwrap().push(json!({
        "from": "score", "to": "report",
        "transform": { "output": "Report", "code": "Report { text: String::new() }" }
    }));

    let names: Vec<_> = load(value)
        .code_pieces()
        .into_iter()
        .map(|piece| piece.name)
        .collect();

    assert!(
        names.contains(&"code_sub_inner_echo".to_owned()),
        "{names:?}"
    );
    assert!(
        names.contains(&"transform_main_score_report_2".to_owned()),
        "{names:?}"
    );
    assert!(
        names.contains(&"transform_main_score_report_3".to_owned()),
        "{names:?}"
    );
}

#[test]
fn a_code_node_needs_code() {
    let mut value = spec();
    value["graph"]["nodes"][0]["params"]["code"] = json!("  ");

    assert_eq!(
        errors(value),
        [error(
            "/graph/nodes/0/params/code",
            ValidationErrorKind::MissingCode
        )]
    );
}

#[test]
fn a_code_node_without_code_is_missing_code_as_well() {
    let mut value = spec();
    value["graph"]["nodes"][0]["params"]
        .as_object_mut()
        .unwrap()
        .remove("code");

    assert_eq!(
        errors(value),
        [error(
            "/graph/nodes/0/params/code",
            ValidationErrorKind::MissingCode
        )]
    );
}

#[test]
fn code_nodes_name_only_known_types() {
    let mut value = spec();
    value["graph"]["nodes"][0]["params"]["output"] = json!("Nope");

    let found = errors(value);

    assert!(
        found.contains(&error(
            "/graph/nodes/0/params/output",
            ValidationErrorKind::UnknownType {
                name: "Nope".into()
            }
        )),
        "{found:?}"
    );
}

#[test]
fn code_nodes_cannot_use_the_message_types_of_the_catalog() {
    let mut value = spec();
    value["graph"]["nodes"][0]["params"]["input"] = json!("message");

    assert!(
        errors(value)
            .iter()
            .any(|e| e.path == "/graph/nodes/0/params/input"
                && matches!(&e.kind, ValidationErrorKind::UnknownType { name } if name == "message"))
    );
}

#[test]
fn code_nodes_accept_array_types() {
    let mut value = spec();
    value["graph"]["nodes"][0]["params"]["output"] = json!("array<Score>");
    value["graph"]["nodes"][0]["params"]["code"] = json!("Ok(Vec::new())");

    let found = errors(value);

    // Only the edge to `report` no longer fits; the node itself is fine.
    assert!(
        found.iter().all(|e| e.path.starts_with("/graph/edges/")),
        "{found:?}"
    );
}

#[test]
fn code_node_parameters_are_checked() {
    let mut value = spec();
    value["graph"]["nodes"][0]["params"] =
        json!({ "input": "string", "code": "Ok(1)", "extra": 1 });

    let found = errors(value);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].path, "/graph/nodes/0/params");
    assert!(
        matches!(&found[0].kind, ValidationErrorKind::InvalidParams { kind, .. } if kind == "code")
    );
}

#[test]
fn the_edges_of_a_code_node_are_type_checked() {
    let mut value = spec();
    value["graph"]["edges"][0] = json!({ "from": "start", "to": "report" });

    assert!(errors(value).contains(&error(
        "/graph/edges/0",
        ValidationErrorKind::TypeMismatch {
            expected: named("Report"),
            found: DataType::String
        }
    )));
}

#[test]
fn on_code_needs_the_type_that_the_source_delivers() {
    let mut value = spec();
    value["graph"]["edges"][1]["when"]["params"]["input"] = json!("Report");

    assert_eq!(
        errors(value),
        [error(
            "/graph/edges/1/when",
            ValidationErrorKind::ConditionInputMismatch {
                kind: "on_code".into(),
                expected: named("Report"),
                found: named("Score"),
            }
        )]
    );
}

#[test]
fn on_code_needs_code() {
    let mut value = spec();
    value["graph"]["edges"][1]["when"]["params"]["code"] = json!("");

    assert_eq!(
        errors(value),
        [error(
            "/graph/edges/1/when/params/code",
            ValidationErrorKind::MissingCode
        )]
    );
}

#[test]
fn on_code_names_only_known_types() {
    let mut value = spec();
    value["graph"]["edges"][1]["when"]["params"]["input"] = json!("Nope");

    let found = errors(value);

    assert!(
        found.contains(&error(
            "/graph/edges/1/when/params/input",
            ValidationErrorKind::UnknownType {
                name: "Nope".into()
            }
        )),
        "{found:?}"
    );
}

#[test]
fn the_output_of_a_transformation_must_fit_the_target() {
    let mut value = spec();
    value["graph"]["edges"][1]["transform"]["output"] = json!("Score");

    assert_eq!(
        errors(value),
        [error(
            "/graph/edges/1",
            ValidationErrorKind::TypeMismatch {
                expected: named("Report"),
                found: named("Score")
            }
        )]
    );
}

#[test]
fn a_transformation_fixes_a_type_mismatch_between_the_nodes() {
    let mut value = spec();
    value["graph"]["edges"][1]
        .as_object_mut()
        .unwrap()
        .remove("when");

    assert_eq!(errors(value), []);
}

#[test]
fn a_declared_transformation_input_must_be_what_the_edge_delivers() {
    let mut value = spec();
    value["graph"]["edges"][1]["transform"]["input"] = json!("Report");

    assert_eq!(
        errors(value),
        [error(
            "/graph/edges/1/transform/input",
            ValidationErrorKind::TransformInputMismatch {
                expected: named("Report"),
                found: named("Score"),
            }
        )]
    );
}

#[test]
fn a_transformation_needs_code_and_known_types() {
    let mut value = spec();
    value["graph"]["edges"][1]["transform"] = json!({ "output": "Nope", "code": "" });

    let found = errors(value);

    assert!(
        found.contains(&error(
            "/graph/edges/1/transform/code",
            ValidationErrorKind::MissingCode
        )),
        "{found:?}"
    );
    assert!(
        found.contains(&error(
            "/graph/edges/1/transform/output",
            ValidationErrorKind::UnknownType {
                name: "Nope".into()
            }
        )),
        "{found:?}"
    );
}

#[test]
fn a_transformation_cannot_read_a_message() {
    let value = json!({
        "spec_version": 1, "name": "scored",
        "model": { "provider": "ollama", "id": "m" },
        "graph": {
            "nodes": [ { "id": "ask", "kind": "llm_request" } ],
            "edges": [
                { "from": "start", "to": "ask" },
                { "from": "ask", "to": "finish", "transform": { "output": "string", "code": "String::new()" } }
            ]
        }
    });

    assert!(
        errors(value).iter().any(|e| e.path == "/graph/edges/1/transform"
            && matches!(&e.kind, ValidationErrorKind::UnknownType { name } if name == "message"))
    );
}

#[test]
fn code_in_a_subgraph_is_reported_with_the_subgraph_pointer() {
    let mut value = spec();
    value["subgraphs"] = json!({
        "inner": {
            "graph": {
                "nodes": [ { "id": "echo", "kind": "code", "params": { "input": "string", "output": "string", "code": "" } } ],
                "edges": [ { "from": "start", "to": "echo" }, { "from": "echo", "to": "finish" } ]
            }
        }
    });

    assert_eq!(
        errors(value),
        [error(
            "/subgraphs/inner/graph/nodes/0/params/code",
            ValidationErrorKind::MissingCode
        )]
    );
}

#[test]
fn a_code_piece_must_not_share_its_name_with_a_tool() {
    let mut value = spec();
    value["mcp_servers"] =
        json!({ "scores": { "command": "scores", "tools": ["code_main_score"] } });

    assert_eq!(
        errors(value),
        [error(
            "/graph/nodes/0/params/code",
            ValidationErrorKind::DuplicateToolName {
                name: "code_main_score".into(),
                other_origin: ToolOrigin::Mcp,
                other_pointer: "/mcp_servers/scores/tools/0".into()
            }
        )]
    );
}
