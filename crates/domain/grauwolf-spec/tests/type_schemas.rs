//! Type schemas in the subset shared by interpreter and generator (ADR-0031, ADR-0080).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ValidationErrorKind};
use serde_json::{Value, json};

fn errors(types: Value) -> Vec<(String, ValidationErrorKind)> {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "types": types,
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    spec.validate()
        .err()
        .unwrap_or_default()
        .into_iter()
        .map(|error| (error.path, error.kind))
        .collect()
}

#[test]
fn accepts_the_common_subset_of_schemas() {
    let found = errors(json!({
        "Ticket": {
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "priority": { "type": "integer" },
                "score": { "type": "number" },
                "urgent": { "type": "boolean" },
                "tags": { "type": "array", "items": { "type": "string" } },
                "grid": { "type": "array", "items": { "type": "array", "items": { "type": "number" } } }
            },
            "required": ["title"]
        }
    }));

    assert_eq!(found, Vec::new());
}

#[test]
fn reports_schemas_outside_the_common_subset_at_their_place() {
    let found = errors(json!({
        "Liste": { "type": "array", "items": { "type": "string" } },
        "Ticket": {
            "type": "object",
            "properties": {
                "owner": { "type": "object", "properties": {} },
                "note": { "description": "no type" },
                "tags": { "type": "array" },
                "Title": { "type": "string" }
            }
        }
    }));

    let paths: Vec<&str> = found.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/types/Liste",
            "/types/Ticket/properties/Title",
            "/types/Ticket/properties/note",
            "/types/Ticket/properties/owner",
            "/types/Ticket/properties/tags",
        ]
    );
    assert!(
        found
            .iter()
            .all(|(_, kind)| matches!(kind, ValidationErrorKind::UnsupportedSchema { .. }))
    );
}
