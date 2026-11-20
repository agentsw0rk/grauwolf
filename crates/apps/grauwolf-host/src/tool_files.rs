//! Tool definitions in separate files (ADR-0073).

/// JSON schema of the spec format as written in files: extended by `tool_files` and
/// `code_file`, which [`crate::loader::load_spec`] resolves before validation (ADR-0073, ADR-0080).
pub fn spec_schema() -> serde_json::Value {
    let mut schema = grauwolf_spec::AgentSpec::json_schema();
    schema["properties"]["tool_files"] = serde_json::json!({
        "description": "Tool definitions in separate JSON files, relative to the spec; their `code_file` is relative to the tool file (ADR-0073).",
        "type": "array",
        "items": { "type": "string" }
    });
    let tool = &mut schema["$defs"]["ToolSpec"];
    tool["properties"]["code_file"] = serde_json::json!({
        "description": "File with the body of the tool function, relative to the spec or tool file; instead of `code` (ADR-0073).",
        "type": "string"
    });
    if let Some(required) = tool["required"].as_array_mut() {
        required.retain(|field| field != "code");
    }
    tool["oneOf"] = serde_json::json!([
        { "required": ["code"] },
        { "required": ["code_file"] }
    ]);
    // `transform` of an edge: code or a file with it (ADR-0102); the params of `code` nodes and
    // `on_code` conditions are free-form and checked by the validation.
    let transform = &mut schema["$defs"]["TransformSpec"];
    if transform.is_object() {
        transform["properties"]["code_file"] = serde_json::json!({
            "description": "File with the body of `fn transform(value: In) -> Out`, relative to the spec; instead of `code` (ADR-0102).",
            "type": "string"
        });
        if let Some(required) = transform["required"].as_array_mut() {
            required.retain(|field| field != "code");
        }
        transform["oneOf"] = serde_json::json!([
            { "required": ["code"] },
            { "required": ["code_file"] }
        ]);
    }
    schema
}
