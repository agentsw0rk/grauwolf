//! Normalization of structured values against the schema subset (ADR-0031).

use grauwolf_agents::structured::normalize;
use serde_json::json;

#[test]
fn keeps_required_fields_and_drops_unknown_ones() {
    let schema = json!({
        "type": "object",
        "properties": { "title": { "type": "string" } },
        "required": ["title"]
    });

    assert_eq!(
        normalize(&schema, &json!({ "title": "Bug", "extra": 1 })),
        Ok(json!({ "title": "Bug" }))
    );
    assert_eq!(
        normalize(&schema, &json!({})).unwrap_err().to_string(),
        "/title: missing required field"
    );
}

#[test]
fn checks_primitives_like_serde_does() {
    let schema = json!({
        "type": "object",
        "properties": {
            "priority": { "type": "integer" },
            "estimate": { "type": "number" },
            "urgent": { "type": "boolean" }
        },
        "required": ["priority", "estimate", "urgent"]
    });

    assert_eq!(
        normalize(
            &schema,
            &json!({ "priority": 2, "estimate": 3, "urgent": true })
        ),
        Ok(json!({ "priority": 2, "estimate": 3.0, "urgent": true }))
    );
    assert_eq!(
        normalize(
            &schema,
            &json!({ "priority": 1.5, "estimate": 3, "urgent": true })
        )
        .unwrap_err()
        .to_string(),
        "/priority: expected integer"
    );
    assert_eq!(
        normalize(
            &schema,
            &json!({ "priority": 1, "estimate": 3, "urgent": "yes" })
        )
        .unwrap_err()
        .to_string(),
        "/urgent: expected boolean"
    );
}

#[test]
fn drops_null_in_optional_fields_and_normalizes_array_items() {
    let schema = json!({
        "type": "object",
        "properties": {
            "tags": { "type": "array", "items": { "type": "string" } },
            "scores": { "type": "array", "items": { "type": "number" } },
            "note": { "type": "string" }
        },
        "required": ["tags"]
    });

    assert_eq!(
        normalize(
            &schema,
            &json!({ "tags": ["ui"], "scores": [1], "note": null })
        ),
        Ok(json!({ "tags": ["ui"], "scores": [1.0] }))
    );
    assert_eq!(
        normalize(&schema, &json!({ "tags": ["ui", 3] }))
            .unwrap_err()
            .to_string(),
        "/tags/1: expected string"
    );
}
