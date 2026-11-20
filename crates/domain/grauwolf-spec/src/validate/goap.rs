//! GOAP nodes (ADR-0070): state, actions and goals.

use std::collections::{BTreeMap, HashSet};

use serde_json::Value;

use super::Rules;
use super::ValidationErrorKind;
use super::schema::check_keys;
use crate::{GOAP_KIND, NodeSpec, STRING_TYPE};

/// Type of a fact, fixed by its initial value in `state` and the list under `values`.
enum Kind<'a> {
    Bool,
    /// Text from a fixed list.
    Enum(&'a [Value]),
    Int,
    Number,
    /// Text without a list.
    Text,
}

impl Kind<'_> {
    fn is_numeric(&self) -> bool {
        matches!(self, Self::Int | Self::Number)
    }

    /// Does the JSON value fit the fact type?
    fn fits(&self, value: &Value) -> bool {
        match self {
            Self::Bool => value.is_boolean(),
            Self::Enum(allowed) => allowed.contains(value),
            Self::Int => value.as_i64().is_some(),
            Self::Number => value.is_number(),
            Self::Text => value.is_string(),
        }
    }

    fn expected(&self, fact: &str, value: &Value) -> String {
        match self {
            Self::Enum(_) if value.is_string() => {
                format!("value {value} is not in `values.{fact}`")
            }
            Self::Bool => "expected true or false".into(),
            Self::Enum(_) => format!("expected a value from `values.{fact}`"),
            Self::Int => "expected an integer".into(),
            Self::Number => "expected a number".into(),
            Self::Text => "expected a text".into(),
        }
    }
}

type Facts<'a> = BTreeMap<&'a str, Kind<'a>>;

/// Which of the two lists a set of conditions or effects is.
#[derive(Clone, Copy, PartialEq)]
enum Use {
    Condition,
    Effect,
}

/// Facts with their kinds from `state` and `values` (enumerations: addendum ADR-0070).
fn facts_of(params: &Value) -> Result<Facts<'_>, String> {
    let state = params["state"]
        .as_object()
        .filter(|state| {
            state
                .values()
                .all(|fact| fact.is_boolean() || fact.is_number() || fact.is_string())
        })
        .ok_or("`state` needs facts as `{ \"name\": true|false|number|\"text\" }`")?;
    let values = match &params["values"] {
        Value::Null => None,
        Value::Object(values) => Some(values),
        _ => return Err("`values` needs `{ \"fact\": [\"value\", …] }`".into()),
    };
    for (fact, allowed) in values.into_iter().flatten() {
        match state.get(fact) {
            Some(Value::String(_)) => {}
            Some(_) => {
                return Err(format!("values.{fact}: only text facts have a value list"));
            }
            None => return Err(format!("values.{fact}: fact is not in `state`")),
        }
        let listed = allowed
            .as_array()
            .filter(|allowed| !allowed.is_empty() && allowed.iter().all(Value::is_string));
        if listed.is_none() {
            return Err(format!(
                "values.{fact}: expected a list with at least one text"
            ));
        }
    }
    let mut facts = Facts::new();
    for (fact, value) in state {
        let kind = match value {
            Value::Bool(_) => Kind::Bool,
            Value::Number(number) if number.is_i64() => Kind::Int,
            Value::Number(_) => Kind::Number,
            _ => match values
                .and_then(|values| values.get(fact))
                .and_then(Value::as_array)
            {
                Some(allowed) => {
                    if !allowed.contains(value) {
                        return Err(format!(
                            "state.{fact}: value {value} is not in `values.{fact}`"
                        ));
                    }
                    Kind::Enum(allowed)
                }
                None => Kind::Text,
            },
        };
        facts.insert(fact.as_str(), kind);
    }
    Ok(facts)
}

/// Checks `{ fact: value | { op: value } }` (conditions) or `{ fact: value | { "add": n } }`
/// (effects) against the facts.
fn check_facts(value: &Value, what: &str, facts: &Facts, usage: Use) -> Result<(), String> {
    let Some(object) = value.as_object() else {
        return if value.is_null() {
            Ok(())
        } else {
            Err(format!("{what}: expected an object with facts"))
        };
    };
    for (fact, wanted) in object {
        let Some(kind) = facts.get(fact.as_str()) else {
            return Err(format!("{what}: fact '{fact}' is not in `state`"));
        };
        let (operator, operand) = match wanted {
            Value::Object(operator) => {
                let mut entries = operator.iter();
                let (Some((name, operand)), None) = (entries.next(), entries.next()) else {
                    return Err(match usage {
                        Use::Condition => format!(
                            "{what}.{fact}: expected exactly one operator (eq, ne, gt, lt, ge, le)"
                        ),
                        Use::Effect => {
                            format!("{what}.{fact}: expected a value or exactly `{{ \"add\": n }}`")
                        }
                    });
                };
                (Some(name.as_str()), operand)
            }
            plain => (None, plain),
        };
        match (usage, operator) {
            (_, None) | (Use::Condition, Some("eq" | "ne")) => {}
            (Use::Condition, Some("gt" | "lt" | "ge" | "le")) if kind.is_numeric() => {}
            (Use::Condition, Some("gt" | "lt" | "ge" | "le")) => {
                return Err(format!(
                    "{what}.{fact}: gt, lt, ge and le need an integer or number fact"
                ));
            }
            (Use::Condition, Some(other)) => {
                return Err(format!(
                    "{what}.{fact}: unknown operator '{other}' (eq, ne, gt, lt, ge, le)"
                ));
            }
            (Use::Effect, Some("add")) if kind.is_numeric() => {}
            (Use::Effect, Some("add")) => {
                return Err(format!(
                    "{what}.{fact}: add needs an integer or number fact"
                ));
            }
            (Use::Effect, Some(other)) => {
                return Err(format!(
                    "{what}.{fact}: unknown effect '{other}' (expected a value or `{{ \"add\": n }}`)"
                ));
            }
        }
        // An integer literal may stand for a number fact, but `add` on an integer needs an integer.
        if !kind.fits(operand) {
            return Err(format!("{what}.{fact}: {}", kind.expected(fact, operand)));
        }
    }
    Ok(())
}

/// Checks the `{{…}}` placeholders of an input template: `task` or a fact of `state`.
fn check_template(template: &str, what: &str, facts: &Facts) -> Result<(), String> {
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start..];
        let end = after
            .find("}}")
            .ok_or_else(|| format!("{what}: placeholder `{{{{` is not closed by `}}}}`"))?;
        let name = after[2..end].trim();
        if name != "task" && !facts.contains_key(name) {
            return Err(format!(
                "{what}: unknown placeholder '{{{{{name}}}}}' (expected `task` or a fact of `state`)"
            ));
        }
        rest = &after[end + 2..];
    }
    Ok(())
}

/// A free-text fact named by `value` (`input_fact`, `output_fact`).
fn check_text_fact(value: &Value, what: &str, facts: &Facts) -> Result<(), String> {
    let name = value
        .as_str()
        .ok_or_else(|| format!("{what}: expected the name of a free-text fact"))?;
    match facts.get(name) {
        Some(Kind::Text) => Ok(()),
        Some(_) => Err(format!("{what}: fact '{name}' is not a free-text fact")),
        None => Err(format!("{what}: fact '{name}' is not in `state`")),
    }
}

/// An integer ≥ `least` in `params[key]`, if the key is there.
fn check_limit(params: &Value, key: &str, least: u64) -> Result<(), String> {
    match params.get(key) {
        Some(value) if value.as_u64().is_none_or(|value| value < least) => {
            Err(format!("{key}: expected an integer ≥ {least}"))
        }
        _ => Ok(()),
    }
}

impl Rules<'_> {
    /// Checks a `goap` node (ADR-0070): known facts, operators and values that fit the fact
    /// types, executors (subgraphs with input `string`, or agents from `agent_tools`), unique
    /// action names, costs ≥ 0, priorities ≥ 0, at least one action and one goal.
    pub(super) fn goap_of(&self, node: &NodeSpec) -> Result<(), ValidationErrorKind> {
        let invalid = |reason: String| ValidationErrorKind::InvalidParams {
            kind: GOAP_KIND.into(),
            reason,
        };
        let params = node.params.as_ref().unwrap_or(&Value::Null);
        let Some(object) = params.as_object() else {
            return Err(invalid(
                "missing parameters `state`, `actions` and `goals`".into(),
            ));
        };
        if let Some(extra) = object.keys().find(|key| {
            ![
                "state",
                "values",
                "actions",
                "goals",
                "max_iterations",
                "max_search_nodes",
                "max_plan_depth",
                "input_fact",
                "output_fact",
            ]
            .contains(&key.as_str())
        }) {
            return Err(invalid(format!("/{extra}: unknown parameter")));
        }
        let facts = facts_of(params).map_err(invalid)?;
        for key in ["input_fact", "output_fact"] {
            if let Some(fact) = params.get(key) {
                check_text_fact(fact, key, &facts).map_err(invalid)?;
            }
        }
        let actions = params["actions"]
            .as_array()
            .filter(|actions| !actions.is_empty());
        let actions =
            actions.ok_or_else(|| invalid("`actions` needs at least one action".into()))?;
        let mut names = HashSet::new();
        for (index, action) in actions.iter().enumerate() {
            let what = format!("actions[{index}]");
            self.check_goap_action(action, &what, &facts, &mut names)?;
        }
        let goals = params["goals"].as_array().filter(|goals| !goals.is_empty());
        let goals = goals.ok_or_else(|| invalid("`goals` needs at least one goal".into()))?;
        for (index, goal) in goals.iter().enumerate() {
            let what = format!("goals[{index}]");
            check_keys(goal, &what, &["name", "facts", "priority"]).map_err(invalid)?;
            goal["name"]
                .as_str()
                .ok_or_else(|| invalid(format!("{what}.name is missing")))?;
            check_facts(
                &goal["facts"],
                &format!("{what}.facts"),
                &facts,
                Use::Condition,
            )
            .map_err(invalid)?;
            check_limit(goal, "priority", 0).map_err(|error| invalid(format!("{what}.{error}")))?;
        }
        for (key, least) in [
            ("max_iterations", 1),
            ("max_search_nodes", 1),
            ("max_plan_depth", 1),
        ] {
            check_limit(params, key, least).map_err(invalid)?;
        }
        Ok(())
    }

    fn check_goap_action<'a>(
        &self,
        action: &'a Value,
        what: &str,
        facts: &Facts,
        names: &mut HashSet<&'a str>,
    ) -> Result<(), ValidationErrorKind> {
        let invalid = |reason: String| ValidationErrorKind::InvalidParams {
            kind: GOAP_KIND.into(),
            reason,
        };
        check_keys(
            action,
            what,
            &[
                "name",
                "subgraph",
                "agent",
                "pre",
                "effects",
                "on_success",
                "on_failure",
                "cost",
                "description",
                "input",
                "output_fact",
                "success_contains",
            ],
        )
        .map_err(invalid)?;
        let name = action["name"]
            .as_str()
            .ok_or_else(|| invalid(format!("{what}.name is missing")))?;
        if !names.insert(name) {
            return Err(invalid(format!("{what}: action '{name}' already exists")));
        }
        match (action.get("subgraph"), action.get("agent")) {
            (Some(_), Some(_)) => {
                return Err(invalid(format!(
                    "{what}: `subgraph` and `agent` exclude each other"
                )));
            }
            (None, None) => {
                return Err(invalid(format!("{what}: needs a `subgraph` or an `agent`")));
            }
            (Some(subgraph), None) => {
                let subgraph = subgraph
                    .as_str()
                    .ok_or_else(|| invalid(format!("{what}.subgraph is missing")))?;
                let spec = self.subgraphs.get(subgraph).ok_or_else(|| {
                    invalid(format!("{what}: subgraph '{subgraph}' does not exist"))
                })?;
                if spec.input != STRING_TYPE {
                    return Err(invalid(format!(
                        "{what}: subgraph '{subgraph}' receives the task as text and needs input string"
                    )));
                }
            }
            (None, Some(agent)) => {
                let agent = agent
                    .as_str()
                    .ok_or_else(|| invalid(format!("{what}.agent: expected a name")))?;
                if !self.agent_tools.contains_key(agent) {
                    return Err(ValidationErrorKind::UnknownAgent { name: agent.into() });
                }
            }
        }
        check_facts(
            &action["pre"],
            &format!("{what}.pre"),
            facts,
            Use::Condition,
        )
        .map_err(invalid)?;
        for key in ["effects", "on_success", "on_failure"] {
            check_facts(&action[key], &format!("{what}.{key}"), facts, Use::Effect)
                .map_err(invalid)?;
        }
        if let Some(template) = action.get("input") {
            let template = template
                .as_str()
                .ok_or_else(|| invalid(format!("{what}.input: expected a text")))?;
            check_template(template, &format!("{what}.input"), facts).map_err(invalid)?;
        }
        if let Some(fact) = action.get("output_fact") {
            check_text_fact(fact, &format!("{what}.output_fact"), facts).map_err(invalid)?;
        }
        if action
            .get("success_contains")
            .is_some_and(|text| !text.is_string())
        {
            return Err(invalid(format!("{what}.success_contains: expected a text")));
        }
        if let Some(cost) = action.get("cost")
            && cost.as_f64().is_none_or(|cost| cost < 0.0)
        {
            return Err(invalid(format!("{what}.cost: expected a number ≥ 0")));
        }
        Ok(())
    }
}

/// Subgraphs of the actions of a `goap` node (ADR-0070); actions with an agent have none.
pub(super) fn goap_subgraphs(node: &NodeSpec) -> Vec<&str> {
    node.params
        .as_ref()
        .and_then(|params| params.get("actions"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|action| action.get("subgraph").and_then(Value::as_str))
        .collect()
}
