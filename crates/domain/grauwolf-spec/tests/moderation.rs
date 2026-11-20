//! Moderation in the spec (ADR-0066).

use grauwolf_spec::AgentSpec;
use grauwolf_spec::AgentSpecExt;
use serde_json::json;

#[test]
fn counts_the_provider_of_a_moderation_node() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "guarded",
            "model": { "provider": "ollama", "id": "m" },
            "types": { "Moderation": {
                "type": "object",
                "properties": {
                    "harmful": { "type": "boolean" },
                    "categories": { "type": "array", "items": { "type": "string" } },
                    "text": { "type": "string" },
                    "message": { "type": "string" }
                },
                "required": ["harmful", "categories", "text", "message"]
            } },
            "graph": {
                "nodes": [
                    { "id": "moderate", "kind": "llm_moderate",
                      "params": { "type": "Moderation", "provider": "openai", "id": "omni-moderation-latest" } },
                    { "id": "pass", "kind": "get_field", "params": { "type": "Moderation", "field": "text" } }
                ],
                "edges": [
                    { "from": "start", "to": "moderate" },
                    { "from": "moderate", "to": "pass" },
                    { "from": "pass", "to": "finish" }
                ]
            }
        })
        .to_string(),
    )
    .unwrap();

    assert_eq!(spec.providers(), ["ollama", "openai"]);
}
