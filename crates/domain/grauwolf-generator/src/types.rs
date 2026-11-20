//! Rust structs from the named types of the spec (ADR-0022).
//!
//! Supported is the subset of JSON Schema that maps unambiguously to Rust: objects with
//! `properties` and `required`, `string`, `integer`, `number`, `boolean` and `array` with
//! `items`. Fields not listed as required become `Option`.

use grauwolf_codegen::{Field, Scope, Struct};
use grauwolf_spec::{AgentSpec, STRING_TYPE};
use serde_json::Value;

use crate::GeneratorError;

/// Adds a struct to `scope` for every type of the spec (sorted by name).
///
/// Every struct implements `grauwolf_agents::Structured` with the schema from the spec,
/// so that both modes send the LLM the same schema (ADR-0031).
pub(crate) fn push_types(scope: &mut Scope, spec: &AgentSpec) -> Result<(), GeneratorError> {
    for (name, schema) in &spec.types {
        scope.push_struct(type_struct(name, schema)?);
        scope
            .new_impl(name)
            .impl_trait("grauwolf_agents::Structured")
            .new_fn("structured_output")
            .ret("grauwolf_llm::StructuredOutput")
            .line(format!(
                "grauwolf_llm::StructuredOutput {{ name: {name:?}.into(), \
                 schema: serde_json::from_str({:?}).expect(\"schema from the spec is valid JSON\") }}",
                schema.to_string()
            ));
    }
    Ok(())
}

/// Rust type for a type reference of the spec (`input`, `output`).
pub(crate) fn type_ref(name: &str) -> String {
    if name == STRING_TYPE {
        "String".to_owned()
    } else if let Some(element) = name
        .strip_prefix("array<")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        format!("Vec<{}>", type_ref(element))
    } else {
        name.to_owned()
    }
}

fn type_struct(name: &str, schema: &Value) -> Result<Struct, GeneratorError> {
    let path = format!("/types/{name}");
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err(unsupported(
            &path,
            "a type must be an object (\"type\": \"object\")",
        ));
    }
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();

    let mut item = Struct::new(name);
    item.vis("pub")
        .derive("Debug")
        .derive("Clone")
        .derive("PartialEq")
        .derive("serde::Serialize")
        .derive("serde::Deserialize")
        .derive("schemars::JsonSchema");
    if let Some(description) = schema.get("description").and_then(Value::as_str) {
        item.doc(description);
    }
    let properties = schema.get("properties").and_then(Value::as_object);
    for (property, property_schema) in properties.into_iter().flatten() {
        let property_path = format!(
            "{path}/properties/{}",
            property.replace('~', "~0").replace('/', "~1")
        );
        if !is_field_identifier(property) || NOT_RAW_IDENTIFIER.contains(&property.as_str()) {
            return Err(unsupported(
                &property_path,
                "field names consist of lowercase letters, digits and '_' and do not start with a digit",
            ));
        }
        let ty = field_type(property_schema, &property_path)?;
        let mut field = if required.contains(&property.as_str()) {
            Field::new(field_ident(property), ty)
        } else {
            let mut field = Field::new(field_ident(property), format!("Option<{ty}>"));
            field.annotation("#[serde(default, skip_serializing_if = \"Option::is_none\")]");
            field
        };
        field.vis("pub");
        if let Some(description) = property_schema.get("description").and_then(Value::as_str) {
            field.doc(description);
        }
        item.push_field(field);
    }
    Ok(item)
}

/// Rust type of a field; `path` is the JSON pointer to its schema.
fn field_type(schema: &Value, path: &str) -> Result<String, GeneratorError> {
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => Ok("String".into()),
        Some("integer") => Ok("i64".into()),
        Some("number") => Ok("f64".into()),
        Some("boolean") => Ok("bool".into()),
        Some("array") => {
            let items = schema
                .get("items")
                .ok_or_else(|| unsupported(path, "an array needs \"items\""))?;
            Ok(format!(
                "Vec<{}>",
                field_type(items, &format!("{path}/items"))?
            ))
        }
        _ => Err(unsupported(
            path,
            "supported are \"string\", \"integer\", \"number\", \"boolean\" and \"array\"",
        )),
    }
}

/// Rust keywords (2024); as field names they become raw identifiers (`r#type`), which serde
/// serializes without the `r#` again.
const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "do", "dyn",
    "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl", "in", "let",
    "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref", "return",
    "static", "struct", "trait", "true", "try", "type", "typeof", "unsafe", "unsized", "use",
    "virtual", "where", "while", "yield",
];

/// Names that are not allowed even as raw identifiers.
const NOT_RAW_IDENTIFIER: &[&str] = &["_", "crate", "self", "super"];

/// Rust identifier for the field `name`.
fn field_ident(name: &str) -> String {
    if KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_owned()
    }
}

/// `true` if `name` matches the pattern `[a-z_][a-z0-9_]*`.
fn is_field_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn unsupported(path: &str, reason: &str) -> GeneratorError {
    GeneratorError::UnsupportedType {
        path: path.to_owned(),
        reason: reason.to_owned(),
    }
}
