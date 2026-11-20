//! Guest source of the code pieces of graphs (ADR-0102): the code of a `code` node, an `on_code`
//! condition or an edge transformation becomes a guest crate with tool ABI v1, like a tool.
//!
//! The guest has only `serde_json`, so the Rust types of the spec types are generated together
//! with their conversion from and to JSON; no derive macros are needed.

use std::collections::BTreeMap;

use grauwolf_spec::{CodePiece, CodePieceKind};
use serde_json::Value;

use crate::{ABI_GLUE, BuildError};

/// Source of `src/lib.rs`: the user's function first (so its lines are found at
/// `CODE_FIRST_LINE`), then types, conversions and the ABI glue.
pub(crate) fn piece_source(piece: &CodePiece) -> Result<String, BuildError> {
    let invalid = |reason: String| BuildError::InvalidCode {
        tool: piece.name.clone(),
        reason,
    };
    let input = rust_type(&piece.input);
    let output = rust_type(&piece.output);
    let (signature, call) = match piece.kind {
        CodePieceKind::Node => (
            "fn run(input: In) -> Result<Out, String> {",
            "let output = run(input)?;\n    Ok(ToJson::to_json(&output).to_string())",
        ),
        CodePieceKind::Condition => (
            "fn check(value: &In) -> bool {",
            "Ok(check(&input).to_string())",
        ),
        CodePieceKind::Transform => (
            "fn transform(value: In) -> Out {",
            "let output = transform(input);\n    Ok(ToJson::to_json(&output).to_string())",
        ),
    };
    Ok(format!(
        "//! Generated from the spec code piece `{name}` ({kind}) – do not edit by hand.\n\
         \n\
         use serde_json::Value;\n\
         \n\
         {signature}\n\
         {code}\n\
         }}\n\
         \n\
         #[allow(dead_code)]\n\
         type In = {input};\n\
         #[allow(dead_code)]\n\
         type Out = {output};\n\
         {types}\n\
         {RUNTIME}\n\
         fn tool(args: Value) -> Result<String, String> {{\n    \
             let input = <In as FromJson>::from_json(&args).map_err(|error| format!(\"invalid input: {{error}}\"))?;\n    \
             {call}\n\
         }}\n\
         {ABI_GLUE}",
        name = piece.name,
        kind = piece.kind.as_str(),
        code = piece.code,
        types = type_definitions(&piece.types).map_err(invalid)?,
    ))
}

/// Rust type of a type reference of the spec: `string`, a named type or `array<T>`.
fn rust_type(name: &str) -> String {
    if name == "string" {
        "String".to_owned()
    } else if name == "bool" {
        "bool".to_owned()
    } else if let Some(element) = name
        .strip_prefix("array<")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        format!("Vec<{}>", rust_type(element))
    } else {
        name.to_owned()
    }
}

/// Conversions of the basic types.
const RUNTIME: &str = r#"
#[allow(dead_code)]
trait FromJson: Sized {
    fn from_json(value: &Value) -> Result<Self, String>;
}

#[allow(dead_code)]
trait ToJson {
    fn to_json(&self) -> Value;
}

impl FromJson for String {
    fn from_json(value: &Value) -> Result<Self, String> {
        value.as_str().map(str::to_owned).ok_or_else(|| format!("expected a string, got {value}"))
    }
}

impl FromJson for i64 {
    fn from_json(value: &Value) -> Result<Self, String> {
        value.as_i64().ok_or_else(|| format!("expected an integer, got {value}"))
    }
}

impl FromJson for f64 {
    fn from_json(value: &Value) -> Result<Self, String> {
        value.as_f64().ok_or_else(|| format!("expected a number, got {value}"))
    }
}

impl FromJson for bool {
    fn from_json(value: &Value) -> Result<Self, String> {
        value.as_bool().ok_or_else(|| format!("expected a boolean, got {value}"))
    }
}

impl<T: FromJson> FromJson for Vec<T> {
    fn from_json(value: &Value) -> Result<Self, String> {
        let items = value.as_array().ok_or_else(|| format!("expected a list, got {value}"))?;
        items
            .iter()
            .enumerate()
            .map(|(index, item)| T::from_json(item).map_err(|error| format!("item {index}: {error}")))
            .collect()
    }
}

impl ToJson for String {
    fn to_json(&self) -> Value {
        Value::String(self.clone())
    }
}

impl ToJson for i64 {
    fn to_json(&self) -> Value {
        Value::from(*self)
    }
}

impl ToJson for f64 {
    fn to_json(&self) -> Value {
        serde_json::Number::from_f64(*self).map_or(Value::Null, Value::Number)
    }
}

impl ToJson for bool {
    fn to_json(&self) -> Value {
        Value::Bool(*self)
    }
}

impl<T: ToJson> ToJson for Vec<T> {
    fn to_json(&self) -> Value {
        Value::Array(self.iter().map(ToJson::to_json).collect())
    }
}
"#;

/// Rust keywords (2024); as field names they become raw identifiers.
const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "do", "dyn",
    "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl", "in", "let",
    "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref", "return",
    "static", "struct", "trait", "true", "try", "type", "typeof", "unsafe", "unsized", "use",
    "virtual", "where", "while", "yield",
];

/// A struct with its conversions for every named type.
fn type_definitions(types: &BTreeMap<String, Value>) -> Result<String, String> {
    types
        .iter()
        .map(|(name, schema)| type_definition(name, schema))
        .collect::<Result<Vec<_>, _>>()
        .map(|definitions| definitions.concat())
}

fn type_definition(name: &str, schema: &Value) -> Result<String, String> {
    let required: Vec<&str> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut fields = String::new();
    let mut reads = String::new();
    let mut writes = String::new();
    for (property, property_schema) in schema["properties"].as_object().into_iter().flatten() {
        let ty = field_type(property_schema)
            .map_err(|reason| format!("type {name}, field {property}: {reason}"))?;
        let ident = if KEYWORDS.contains(&property.as_str()) {
            format!("r#{property}")
        } else {
            property.clone()
        };
        let key = format!("{property:?}");
        if required.contains(&property.as_str()) {
            fields.push_str(&format!("    pub {ident}: {ty},\n"));
            reads.push_str(&format!(
                "            {ident}: match object.get({key}) {{\n                \
                 Some(value) if !value.is_null() => FromJson::from_json(value).map_err(|error| format!(\"field `{property}`: {{error}}\"))?,\n                \
                 _ => return Err(\"missing field `{property}`\".to_owned()),\n            }},\n"
            ));
            writes.push_str(&format!(
                "        object.insert({key}.to_owned(), self.{ident}.to_json());\n"
            ));
        } else {
            fields.push_str(&format!("    pub {ident}: Option<{ty}>,\n"));
            reads.push_str(&format!(
                "            {ident}: match object.get({key}) {{\n                \
                 Some(value) if !value.is_null() => Some(FromJson::from_json(value).map_err(|error| format!(\"field `{property}`: {{error}}\"))?),\n                \
                 _ => None,\n            }},\n"
            ));
            writes.push_str(&format!(
                "        if let Some(value) = &self.{ident} {{\n            object.insert({key}.to_owned(), value.to_json());\n        }}\n"
            ));
        }
    }
    Ok(format!(
        "\n#[allow(dead_code)]\n#[derive(Debug, Clone, PartialEq)]\npub struct {name} {{\n{fields}}}\n\n\
         impl FromJson for {name} {{\n    fn from_json(value: &Value) -> Result<Self, String> {{\n        \
         let object = value.as_object().ok_or_else(|| format!(\"expected an object for {name}, got {{value}}\"))?;\n        \
         Ok({name} {{\n{reads}        }})\n    }}\n}}\n\n\
         impl ToJson for {name} {{\n    fn to_json(&self) -> Value {{\n        \
         let mut object = serde_json::Map::new();\n{writes}        Value::Object(object)\n    }}\n}}\n"
    ))
}

/// Rust type of a field: `string`, `integer`, `number`, `boolean` or `array` of these.
fn field_type(schema: &Value) -> Result<String, String> {
    match schema["type"].as_str() {
        Some("string") => Ok("String".into()),
        Some("integer") => Ok("i64".into()),
        Some("number") => Ok("f64".into()),
        Some("boolean") => Ok("bool".into()),
        Some("array") => Ok(format!("Vec<{}>", field_type(&schema["items"])?)),
        _ => Err("supported are string, integer, number, boolean and array".into()),
    }
}
