//! Structured values by the named types of the spec (ADR-0022, ADR-0031).
//!
//! [`normalize`] reproduces in the interpreter what a serde roundtrip through a generated
//! struct would yield. This way the interpreter and generated crates return the same values
//! and reject the same ones.

use grauwolf_graph::NodeData;
use grauwolf_llm::StructuredOutput;
use grauwolf_prompt::Message;
use serde_json::{Map, Value};

/// Type that an LLM can deliver in structured form; generated structs implement it with the
/// schema from the spec (ADR-0031).
pub trait Structured: NodeData {
    /// Name and schema of the type for the request to the LLM.
    fn structured_output() -> StructuredOutput;
}

/// A structured LLM answer is unusable.
#[derive(Debug, thiserror::Error)]
pub enum AnswerError {
    /// The LLM did not return a text answer (e.g. a tool call).
    #[error("LLM answer for type {name} is not text")]
    NotText { name: String },
    /// The answer is not JSON.
    #[error("LLM answer for type {name} is not JSON: {source}")]
    Json {
        name: String,
        #[source]
        source: serde_json::Error,
    },
    /// The answer does not match the schema.
    #[error("LLM answer does not fit type {name}: {source}")]
    Structure {
        name: String,
        #[source]
        source: StructureError,
    },
}

/// Reads `answer` as a value for `output`; a surrounding Markdown code block is allowed.
pub fn parse_answer(answer: &Message, output: &StructuredOutput) -> Result<Value, AnswerError> {
    let name = || output.name.clone();
    let content = match answer {
        Message::Assistant(answer) if answer.tool_calls().next().is_none() => answer.text(),
        _ => return Err(AnswerError::NotText { name: name() }),
    };
    let value =
        serde_json::from_str(strip_code_fence(&content)).map_err(|source| AnswerError::Json {
            name: name(),
            source,
        })?;
    normalize(&output.schema, &value).map_err(|source| AnswerError::Structure {
        name: name(),
        source,
    })
}

/// Removes a Markdown code block (```json … ```) around `text`, if present.
fn strip_code_fence(text: &str) -> &str {
    let text = text.trim();
    match text.strip_prefix("```") {
        Some(rest) => {
            let body = rest.split_once('\n').map_or("", |(_, body)| body);
            body.trim_end().strip_suffix("```").unwrap_or(body).trim()
        }
        None => text,
    }
}

/// A value does not match the schema.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {reason}")]
pub struct StructureError {
    /// JSON pointer to the location in the value.
    pub path: String,
    /// What does not match.
    pub reason: String,
}

/// Checks `value` against `schema` and returns it in normalized form.
pub fn normalize(schema: &Value, value: &Value) -> Result<Value, StructureError> {
    normalize_at(schema, value, "")
}

fn normalize_at(schema: &Value, value: &Value, path: &str) -> Result<Value, StructureError> {
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => normalize_object(schema, value, path),
        Some("string") if value.is_string() => Ok(value.clone()),
        Some("integer") if value.is_i64() => Ok(value.clone()),
        Some("number") => match value.as_f64() {
            Some(number) => Ok(Value::from(number)),
            None => Err(error(path, "expected number")),
        },
        Some("boolean") if value.is_boolean() => Ok(value.clone()),
        Some("array") => {
            let items = value
                .as_array()
                .ok_or_else(|| error(path, "expected array"))?;
            let item_schema = schema.get("items").unwrap_or(&Value::Null);
            items
                .iter()
                .enumerate()
                .map(|(index, item)| normalize_at(item_schema, item, &format!("{path}/{index}")))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        Some(expected) => Err(error(path, format!("expected {expected}"))),
        None => Err(error(path, "schema without \"type\"")),
    }
}

fn normalize_object(schema: &Value, value: &Value, path: &str) -> Result<Value, StructureError> {
    let object = value
        .as_object()
        .ok_or_else(|| error(path, "expected object"))?;
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut normalized = Map::new();
    let properties = schema.get("properties").and_then(Value::as_object);
    for (name, property_schema) in properties.into_iter().flatten() {
        let property_path = format!("{path}/{}", name.replace('~', "~0").replace('/', "~1"));
        let is_required = required.contains(&name.as_str());
        match object.get(name) {
            // Optional fields become `Option`; serde reads `null` as `None` and drops it.
            Some(Value::Null) if !is_required => {}
            Some(property) => {
                normalized.insert(
                    name.clone(),
                    normalize_at(property_schema, property, &property_path)?,
                );
            }
            None if is_required => {
                return Err(error(&property_path, "missing required field"));
            }
            None => {}
        }
    }
    Ok(Value::Object(normalized))
}

fn error(path: &str, reason: impl Into<String>) -> StructureError {
    StructureError {
        path: path.to_owned(),
        reason: reason.into(),
    }
}
