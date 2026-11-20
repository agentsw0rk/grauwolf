//! The node and condition kinds outside the catalogs are known to the validation, so an editor
//! can offer catalog + `STRUCTURAL_NODE_KINDS` + `STRUCTURAL_CONDITION_KINDS` as its palette.

use grauwolf_spec::{
    AgentSpec, AgentSpecExt, STRUCTURAL_CONDITION_KINDS, STRUCTURAL_NODE_KINDS, ValidationErrorKind,
};
use serde_json::json;

#[test]
fn the_structural_kinds_are_never_unknown_kinds() {
    for structural in STRUCTURAL_NODE_KINDS {
        let spec = json!({
            "spec_version": 1, "name": "s", "model": { "provider": "ollama", "id": "m" },
            "graph": {
                "nodes": [{ "id": "n", "kind": structural.kind, "params": {} }],
                "edges": [
                    { "from": "start", "to": "n" },
                    { "from": "n", "to": "finish", "when": { "kind": STRUCTURAL_CONDITION_KINDS[0].kind, "params": {} } }
                ]
            }
        });
        let errors = AgentSpec::from_json(&spec.to_string())
            .unwrap()
            .validate()
            .err()
            .unwrap_or_default();

        assert!(
            !errors.iter().any(|error| matches!(
                error.kind,
                ValidationErrorKind::UnknownNodeKind { .. }
                    | ValidationErrorKind::UnknownConditionKind { .. }
            )),
            "{}: {errors:?}",
            structural.kind
        );
        assert!(structural.description.ends_with('.'));
    }
}
