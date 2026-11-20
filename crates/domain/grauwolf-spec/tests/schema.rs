//! JSON schema of the DSL (F13.2).

use grauwolf_spec::AgentSpec;

#[test]
fn describes_the_spec_format_as_json_schema() {
    let schema = AgentSpec::json_schema();

    assert_eq!(schema["type"], "object");
    for field in [
        "spec_version",
        "name",
        "model",
        "system_prompt",
        "tools",
        "graph",
    ] {
        assert!(
            schema["properties"].get(field).is_some(),
            "{field} is missing from the schema"
        );
    }
    assert_eq!(
        schema["additionalProperties"], false,
        "unknown fields must be forbidden"
    );
}
