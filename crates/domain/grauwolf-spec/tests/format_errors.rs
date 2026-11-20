//! Format errors with JSON pointer and English message (ADR-0080).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, SpecError};
use serde_json::{Value, json};

fn error(spec: Value) -> String {
    match AgentSpec::from_json(&spec.to_string()) {
        Err(error @ SpecError::Format { .. }) => error.to_string(),
        other => panic!("Formatfehler erwartet: {other:?}"),
    }
}

fn base() -> Value {
    json!({
        "spec_version": 1, "name": "agent",
        "model": { "provider": "ollama", "id": "m" },
        "strategy": { "kind": "single_run" }
    })
}

#[test]
fn reports_format_errors_with_the_place_in_the_spec() {
    let mut typo = base();
    typo["sytem_prompt"] = json!("Hello");
    let mut missing = base();
    missing["tools"] = json!([ { "name": "add", "args": {}, "code": "Ok(String::new())" } ]);
    let mut wrong_type = base();
    wrong_type["model"]["id"] = json!(5);

    let typo = error(typo);
    assert!(typo.starts_with("/sytem_prompt: unknown field"), "{typo}");
    assert!(typo.contains("system_prompt"), "{typo}");
    assert_eq!(
        error(missing),
        "/tools/0/description: missing required field"
    );
    let wrong_type = error(wrong_type);
    assert!(
        wrong_type.starts_with("/model/id: wrong type"),
        "{wrong_type}"
    );
}

#[test]
fn keeps_syntax_errors_with_their_line() {
    let error = AgentSpec::from_json("{ kaputt").unwrap_err();

    assert!(matches!(error, SpecError::Json(_)), "{error:?}");
}
