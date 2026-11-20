//! Parameters of the `for_each` node (ADR-0101), shared by the validation, the interpreter and
//! the generator.

use serde::Deserialize;
use serde_json::Value;

/// Most elements that a `for_each` node may run at the same time.
pub const MAX_PARALLEL_LIMIT: usize = 16;

/// What runs for each element.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ForEachBody {
    /// A sub-agent from `agent_tools`.
    Agent(String),
    /// A subgraph of the spec, `T → U`.
    Subgraph(String),
}

/// What a failing element does to the node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForEachOnError {
    /// The node fails.
    #[default]
    Fail,
    /// The element is left out of the output.
    Skip,
}

/// The parameters of a `for_each` node.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForEachParams {
    /// Input type: `array<T>`, or `string` for text that holds a JSON array.
    #[serde(default = "default_list")]
    pub input: String,
    /// Output type: `array<U>`.
    #[serde(default = "default_list")]
    pub output: String,
    /// What runs for each element.
    pub body: ForEachBody,
    /// How many elements run at the same time (1 to 16).
    #[serde(default = "one")]
    pub max_parallel: usize,
    /// What a failing element does.
    #[serde(default)]
    pub on_error: ForEachOnError,
}

fn default_list() -> String {
    "array<string>".into()
}

fn one() -> usize {
    1
}

impl ForEachParams {
    /// Reads the parameters of a node; the error says what is wrong.
    pub fn parse(params: Option<&Value>) -> Result<Self, String> {
        let params = params.ok_or("missing parameters, at least \"body\"")?;
        let parsed: Self = serde_json::from_value(params.clone()).map_err(|error| {
            let reason = error.to_string();
            if reason.contains("unknown variant") && reason.contains("agent") {
                "\"body\" must be { \"agent\": \"<name>\" } or { \"subgraph\": \"<name>\" }".into()
            } else {
                reason
            }
        })?;
        if !(1..=MAX_PARALLEL_LIMIT).contains(&parsed.max_parallel) {
            return Err(format!(
                "\"max_parallel\" must be from 1 to {MAX_PARALLEL_LIMIT}"
            ));
        }
        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_run_one_element_at_a_time_and_fail_on_errors() {
        let params = ForEachParams::parse(Some(&json!({ "body": { "agent": "writer" } }))).unwrap();

        assert_eq!(params.body, ForEachBody::Agent("writer".into()));
        assert_eq!(params.max_parallel, 1);
        assert_eq!(params.on_error, ForEachOnError::Fail);
        assert_eq!(
            (params.input.as_str(), params.output.as_str()),
            ("array<string>", "array<string>")
        );
    }

    #[test]
    fn max_parallel_is_limited_to_sixteen() {
        for bad in [0, 17] {
            let error = ForEachParams::parse(Some(
                &json!({ "body": { "subgraph": "x" }, "max_parallel": bad }),
            ))
            .unwrap_err();
            assert!(error.contains("from 1 to 16"), "{error}");
        }
    }

    #[test]
    fn body_names_exactly_an_agent_or_a_subgraph() {
        let error = ForEachParams::parse(Some(&json!({ "body": { "tool": "x" } }))).unwrap_err();
        assert!(error.contains("{ \"agent\": \"<name>\" }"), "{error}");
        assert!(ForEachParams::parse(Some(&json!({}))).is_err());
        assert!(ForEachParams::parse(None).is_err());
    }
}
