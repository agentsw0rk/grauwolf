//! Names and type declarations: spec name, type names and schemas, input/output types.

use grauwolf_graph::DataType;

use super::Rules;
use super::{ValidationError, ValidationErrorKind, escape_pointer_token, is_snake_case_identifier};
use crate::STRING_TYPE;

impl Rules<'_> {
    /// The name becomes the crate name of generated agents and must therefore be an identifier.
    pub(super) fn check_name(&self, errors: &mut Vec<ValidationError>) {
        if !is_snake_case_identifier(&self.name) {
            errors.push(ValidationError {
                path: "/name".into(),
                kind: ValidationErrorKind::InvalidId {
                    id: self.name.clone(),
                },
            });
        }
    }

    pub(super) fn check_type_names(&self, errors: &mut Vec<ValidationError>) {
        errors.extend(
            self.types
                .keys()
                .filter(|name| !is_pascal_case_identifier(name))
                .map(|name| ValidationError {
                    path: format!("/types/{}", escape_pointer_token(name)),
                    kind: ValidationErrorKind::InvalidTypeName { name: name.clone() },
                }),
        );
    }

    /// Type names become Rust structs in the generated code (ADR-0022).
    /// Every type schema: an object with fields of `string`, `integer`, `number`, `boolean` and
    /// `array` with `items` (ADR-0031, ADR-0080) – as in the generator.
    pub(super) fn check_type_schemas(&self, errors: &mut Vec<ValidationError>) {
        let mut unsupported = |path: String, reason: &str| {
            errors.push(ValidationError {
                path,
                kind: ValidationErrorKind::UnsupportedSchema {
                    reason: reason.into(),
                },
            });
        };
        for (name, schema) in &self.types {
            let path = format!("/types/{}", escape_pointer_token(name));
            if schema.get("type").and_then(serde_json::Value::as_str) != Some("object") {
                unsupported(path, "a type must be an object (\"type\": \"object\")");
                continue;
            }
            let properties = schema
                .get("properties")
                .and_then(serde_json::Value::as_object);
            for (field, field_schema) in properties.into_iter().flatten() {
                let field_path = format!("{path}/properties/{}", escape_pointer_token(field));
                if !is_field_name(field) {
                    unsupported(
                        field_path,
                        "field names consist of lowercase letters, digits and '_' and do not start with a digit",
                    );
                } else if let Err((at, reason)) = check_field_schema(field_schema, &field_path) {
                    unsupported(at, reason);
                }
            }
        }
    }

    /// `input` and `output` refer to `string` or a declared type (ADR-0022).
    pub(super) fn check_signature_types(&self, errors: &mut Vec<ValidationError>) {
        errors.extend(
            [("/input", &self.input), ("/output", &self.output)]
                .into_iter()
                .filter(|(_, name)| !self.is_known_type(name))
                .map(|(path, name)| ValidationError {
                    path: path.into(),
                    kind: ValidationErrorKind::UnknownType { name: name.clone() },
                }),
        );
    }

    pub(super) fn is_known_type(&self, name: &str) -> bool {
        name == STRING_TYPE || self.types.contains_key(name)
    }

    /// Like [`Rules::is_known_type`], and `array<T>` of a known type (ADR-0101).
    pub(super) fn is_known_node_type(&self, name: &str) -> bool {
        match name
            .strip_prefix("array<")
            .and_then(|rest| rest.strip_suffix('>'))
        {
            Some(element) => self.is_known_node_type(element),
            None => self.is_known_type(name),
        }
    }
}

/// `true` if `name` matches the pattern `[A-Z][A-Za-z0-9]*`.
pub(super) fn is_pascal_case_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_alphanumeric())
}

/// Field name of a type: lowercase letters, digits, `_`, not starting with a digit; not `_`,
/// `crate`, `self`, `super` (also invalid as raw identifiers).
pub(super) fn is_field_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !["_", "crate", "self", "super"].contains(&name)
}

/// Checks the schema of a field; on an error, pointer and reason.
pub(super) fn check_field_schema(
    schema: &serde_json::Value,
    path: &str,
) -> Result<(), (String, &'static str)> {
    match schema.get("type").and_then(serde_json::Value::as_str) {
        Some("string" | "integer" | "number" | "boolean") => Ok(()),
        Some("array") => match schema.get("items") {
            Some(items) => check_field_schema(items, &format!("{path}/items")),
            None => Err((path.to_owned(), "an array needs \"items\"")),
        },
        _ => Err((
            path.to_owned(),
            "supported are \"string\", \"integer\", \"number\", \"boolean\" and \"array\"",
        )),
    }
}

/// Reports keys of `value` that are not in `allowed` as `<what>.<key>`
/// (ADR-0080).
pub(super) fn check_keys(
    value: &serde_json::Value,
    what: &str,
    allowed: &[&str],
) -> Result<(), String> {
    match value
        .as_object()
        .and_then(|object| object.keys().find(|key| !allowed.contains(&key.as_str())))
    {
        Some(key) => Err(format!(
            "{what}.{key}: unknown key (allowed: {})",
            allowed.join(", ")
        )),
        None => Ok(()),
    }
}

/// Data type for a type reference of the spec.
pub(super) fn data_type(name: &str) -> DataType {
    if name == STRING_TYPE {
        DataType::String
    } else if name.starts_with("array<") {
        DataType::from_name(name)
    } else {
        DataType::Named(name.into())
    }
}

#[cfg(test)]
mod tests {
    use super::{data_type, is_field_name, is_pascal_case_identifier};
    use grauwolf_graph::DataType;

    #[test]
    fn field_names_are_lowercase_and_not_reserved() {
        assert!(is_field_name("name_2"));
        assert!(is_field_name("_private"));
        for bad in ["", "_", "self", "Name", "2nd", "a-b"] {
            assert!(!is_field_name(bad), "{bad}");
        }
    }

    #[test]
    fn type_names_are_pascal_case() {
        assert!(is_pascal_case_identifier("Answer2"));
        assert!(!is_pascal_case_identifier("answer"));
        assert!(!is_pascal_case_identifier("My_Type"));
    }

    #[test]
    fn string_is_builtin_and_other_names_are_nominal() {
        assert_eq!(data_type("string"), DataType::String);
        assert_eq!(data_type("Answer"), DataType::Named("Answer".into()));
    }
}
