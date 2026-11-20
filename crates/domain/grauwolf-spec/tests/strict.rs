//! Strict deserialization per ADR-0025.

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, SpecError};

#[test]
fn rejects_unknown_fields_at_every_level() {
    let cases = [
        (
            "root",
            r#"{ "spec_version": 1, "name": "echo", "model": { "provider": "ollama", "id": "m" }, "nmae": "x" }"#,
            "nmae",
        ),
        (
            "model",
            r#"{ "spec_version": 1, "name": "echo", "model": { "provider": "ollama", "id": "m", "temp": 1 } }"#,
            "temp",
        ),
        (
            "graph",
            r#"{ "spec_version": 1, "name": "echo", "model": { "provider": "ollama", "id": "m" }, "graph": { "nodez": [] } }"#,
            "nodez",
        ),
        (
            "tool",
            r#"{ "spec_version": 1, "name": "echo", "model": { "provider": "ollama", "id": "m" },
                 "tools": [ { "name": "t", "description": "d", "args": {}, "code": "", "langauge": "rust" } ] }"#,
            "langauge",
        ),
        (
            "edge",
            r#"{ "spec_version": 1, "name": "echo", "model": { "provider": "ollama", "id": "m" },
                      "graph": { "edges": [ { "from": "start", "to": "finish", "wehn": "on_tool_call" } ] } }"#,
            "wehn",
        ),
    ];

    for (level, json, field) in cases {
        let error = AgentSpec::from_json(json).expect_err(level).to_string();
        assert!(
            error.contains(field),
            "{level}: error does not name '{field}': {error}"
        );
    }
}

#[test]
fn rejects_unsupported_spec_version_before_parsing_the_rest() {
    // Version 2 could have a different format – so the rest must not be parsed as v1.
    let error =
        AgentSpec::from_json(r#"{ "spec_version": 2, "agent": { "name": "echo" } }"#).unwrap_err();

    assert!(
        matches!(error, SpecError::UnsupportedVersion { found: 2 }),
        "unexpected error: {error}"
    );
}
