//! Declarative parameter schemas of the catalog kinds (ADR-0018).
//!
//! Supports the subset of JSON Schema that parameters need: an object with
//! `properties`, `required` and no other fields; values of type `string`, `integer`
//! (optional `minimum`), `number` or `boolean`.

use serde_json::{Value, json};

/// Schema of a kind without parameters: only `null` or an empty object is allowed.
pub fn no_params() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

/// Checks `params` against `schema`; errors as `<pointer>: <reason>`, joined with `; `.
/// `null` counts as an empty object.
pub fn check_params(schema: &Value, params: &Value) -> Result<(), String> {
    let empty = serde_json::Map::new();
    let object = match params {
        Value::Null => &empty,
        Value::Object(object) => object,
        _ => return Err("expected an object".into()),
    };
    let properties = schema["properties"].as_object();
    let mut errors = Vec::new();
    for (name, value) in object {
        let pointer = format!("/{}", name.replace('~', "~0").replace('/', "~1"));
        match properties.and_then(|properties| properties.get(name)) {
            None => errors.push((pointer, "unknown parameter".to_owned())),
            Some(property) => {
                if let Err(reason) = check_value(property, value) {
                    errors.push((pointer, reason));
                }
            }
        }
    }
    for required in schema["required"].as_array().into_iter().flatten() {
        if let Some(name) = required.as_str().filter(|name| !object.contains_key(*name)) {
            errors.push((format!("/{name}"), "missing required parameter".to_owned()));
        }
    }
    errors.sort();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors
            .into_iter()
            .map(|(pointer, reason)| format!("{pointer}: {reason}"))
            .collect::<Vec<_>>()
            .join("; "))
    }
}

fn check_value(schema: &Value, value: &Value) -> Result<(), String> {
    let expected = schema["type"].as_str().unwrap_or_default();
    let fits = match expected {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        _ => true,
    };
    if !fits {
        return Err(format!("expected {expected}"));
    }
    if let (Some(minimum), Some(number)) = (schema["minimum"].as_f64(), value.as_f64())
        && number < minimum
    {
        return Err(format!("must be at least {minimum}"));
    }
    Ok(())
}
