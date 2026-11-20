//! Subgraphs and the nodes that call them (ADR-0034, ADR-0039, ADR-0045): definitions, plan, parallel, cycles.

use std::collections::HashSet;

use serde_json::Value;

use grauwolf_graph::DataType;
use grauwolf_graph::params::check_params;

use super::Rules;
use super::goap::goap_subgraphs;
use super::{ValidationError, ValidationErrorKind, escape_pointer_token, is_snake_case_identifier};
use crate::{
    FOR_EACH_KIND, ForEachBody, ForEachParams, GOAP_KIND, NodeSpec, PARALLEL_KIND, PLAN_KIND,
    STRING_TYPE, SUBGRAPH_KIND, SubgraphSpec, plan_params_schema,
};

impl Rules<'_> {
    /// Name and spec of the step subgraph of a `plan` node; checks parameters and signature.
    pub(super) fn plan_of<'s>(
        &'s self,
        node: &'s NodeSpec,
    ) -> Result<(&'s str, &'s SubgraphSpec), ValidationErrorKind> {
        let invalid = |reason: String| ValidationErrorKind::InvalidParams {
            kind: PLAN_KIND.into(),
            reason,
        };
        check_params(
            &plan_params_schema(),
            node.params.as_ref().unwrap_or(&Value::Null),
        )
        .map_err(invalid)?;
        let (name, subgraph) = self.subgraph_of(node)?;
        if subgraph.input != STRING_TYPE || subgraph.output != STRING_TYPE {
            return Err(invalid(format!(
                "subgraph '{name}' executes plan steps and must be string → string"
            )));
        }
        Ok((name, subgraph))
    }

    /// Name and spec of the subgraph that a node of kind `subgraph` includes.
    pub(super) fn subgraph_of<'s>(
        &'s self,
        node: &'s NodeSpec,
    ) -> Result<(&'s str, &'s SubgraphSpec), ValidationErrorKind> {
        let name = node
            .params
            .as_ref()
            .and_then(|params| params.get("subgraph"))
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationErrorKind::InvalidParams {
                kind: SUBGRAPH_KIND.into(),
                reason: "missing \"subgraph\" with the name of a subgraph".into(),
            })?;
        let subgraph = self
            .subgraphs
            .get(name)
            .ok_or_else(|| ValidationErrorKind::UnknownSubgraph { name: name.into() })?;
        Ok((name, subgraph))
    }

    /// Spec of a branch of a `parallel` node; checks parameters, branches and merge.
    pub(super) fn parallel_of<'s>(
        &'s self,
        node: &'s NodeSpec,
    ) -> Result<&'s SubgraphSpec, ValidationErrorKind> {
        let invalid = |reason: String| ValidationErrorKind::InvalidParams {
            kind: PARALLEL_KIND.into(),
            reason,
        };
        let params = node
            .params
            .as_ref()
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("missing parameters `subgraphs` and `merge`".into()))?;
        if let Some(extra) = params
            .keys()
            .find(|key| !["subgraphs", "merge", "index", "criteria"].contains(&key.as_str()))
        {
            return Err(invalid(format!("/{extra}: unknown parameter")));
        }
        let names: Vec<&str> = params
            .get("subgraphs")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if names.len() < 2 {
            return Err(invalid(
                "\"subgraphs\" needs at least two subgraph names".into(),
            ));
        }
        let mut branches = Vec::new();
        for name in &names {
            let subgraph =
                self.subgraphs
                    .get(*name)
                    .ok_or_else(|| ValidationErrorKind::UnknownSubgraph {
                        name: (*name).into(),
                    })?;
            branches.push(subgraph);
        }
        let first = branches[0];
        if branches
            .iter()
            .any(|branch| branch.input != first.input || branch.output != first.output)
        {
            return Err(invalid(
                "all branches need the same signature (input and output)".into(),
            ));
        }
        match params.get("merge").and_then(Value::as_str) {
            Some("concat") if first.output == STRING_TYPE => {}
            Some("concat") => {
                return Err(invalid(
                    "\"merge\": concat needs branches with output string".into(),
                ));
            }
            Some("index") => {
                let index = params.get("index").and_then(Value::as_u64);
                if !index.is_some_and(|index| (index as usize) < names.len()) {
                    return Err(invalid(format!(
                        "\"index\" must be a number from 0 to {}",
                        names.len() - 1
                    )));
                }
            }
            Some("llm_select") => {}
            _ => {
                return Err(invalid(
                    "\"merge\" must be \"concat\", \"index\" or \"llm_select\"".into(),
                ));
            }
        }
        Ok(first)
    }

    /// Checks the parameters of a `for_each` node; each error comes with its pointer relative
    /// to the node's `params` (ADR-0101).
    pub(super) fn for_each_errors(&self, node: &NodeSpec) -> Vec<(String, ValidationErrorKind)> {
        let invalid = |reason: String| {
            (
                String::new(),
                ValidationErrorKind::InvalidParams {
                    kind: FOR_EACH_KIND.into(),
                    reason,
                },
            )
        };
        let params = match ForEachParams::parse(node.params.as_ref()) {
            Ok(params) => params,
            Err(reason) => return vec![invalid(reason)],
        };
        let mut errors = Vec::new();
        let mut element = |field: &str, name: &str| -> Option<String> {
            let element = match DataType::from_name(name) {
                DataType::Array(element) => element.to_string(),
                DataType::String => return None,
                _ => {
                    errors.push(invalid(format!("\"{field}\" must be array<T> or string")));
                    return None;
                }
            };
            if element.starts_with("array<") || !self.is_known_type(&element) {
                errors.push((
                    format!("/{field}"),
                    ValidationErrorKind::UnknownType { name: element },
                ));
                return None;
            }
            Some(element)
        };
        let input = element("input", &params.input);
        let output = element("output", &params.output);
        match &params.body {
            ForEachBody::Agent(name) => {
                if !self.agent_tools.contains_key(name) {
                    errors.push((
                        "/body/agent".into(),
                        ValidationErrorKind::UnknownAgent { name: name.clone() },
                    ));
                }
            }
            ForEachBody::Subgraph(name) => match self.subgraphs.get(name) {
                None => errors.push((
                    "/body/subgraph".into(),
                    ValidationErrorKind::UnknownSubgraph { name: name.clone() },
                )),
                Some(subgraph) => {
                    if let Some(input) = input.filter(|input| *input != subgraph.input) {
                        errors.push(invalid(format!(
                            "the subgraph '{name}' takes {}, but the elements of the input are {input}",
                            subgraph.input
                        )));
                    }
                    if let Some(output) = output.filter(|output| *output != subgraph.output) {
                        errors.push(invalid(format!(
                            "the subgraph '{name}' returns {}, but the elements of the output are {output}",
                            subgraph.output
                        )));
                    }
                }
            },
        }
        errors
    }

    /// Names of the known subgraphs that `node` includes; empty for other nodes.
    pub(super) fn called_subgraphs<'s>(&'s self, node: &'s NodeSpec) -> Vec<&'s str> {
        if node.kind == PARALLEL_KIND {
            return node
                .params
                .as_ref()
                .and_then(|params| params.get("subgraphs"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|name| self.subgraphs.contains_key(*name))
                .collect();
        }
        if node.kind == FOR_EACH_KIND {
            return match ForEachParams::parse(node.params.as_ref()) {
                Ok(ForEachParams {
                    body: ForEachBody::Subgraph(name),
                    ..
                }) => self
                    .subgraphs
                    .get_key_value(&name)
                    .map(|(known, _)| known.as_str())
                    .into_iter()
                    .collect(),
                _ => Vec::new(),
            };
        }
        if node.kind == GOAP_KIND {
            return goap_subgraphs(node)
                .into_iter()
                .filter(|name| self.subgraphs.contains_key(*name))
                .collect();
        }
        if node.kind != SUBGRAPH_KIND && node.kind != PLAN_KIND {
            return Vec::new();
        }
        self.subgraph_of(node)
            .ok()
            .map(|(name, _)| name)
            .into_iter()
            .collect()
    }

    /// Subgraph names are snake_case identifiers (own namespace, ADR-0025); their tools
    /// must be in the spec.
    pub(super) fn check_subgraph_definitions(&self, errors: &mut Vec<ValidationError>) {
        let namespace = self.tool_namespace();
        for (name, subgraph) in &self.subgraphs {
            let path = format!("/subgraphs/{}", escape_pointer_token(name));
            if !is_snake_case_identifier(name) {
                errors.push(ValidationError {
                    path: path.clone(),
                    kind: ValidationErrorKind::InvalidId { id: name.clone() },
                });
            }
            for (field, type_name) in [("input", &subgraph.input), ("output", &subgraph.output)] {
                if !self.is_known_node_type(type_name) {
                    errors.push(ValidationError {
                        path: format!("{path}/{field}"),
                        kind: ValidationErrorKind::UnknownType {
                            name: type_name.clone(),
                        },
                    });
                }
            }
            for (index, tool) in subgraph.tools.iter().flatten().enumerate() {
                if !namespace.is_taken(tool) {
                    errors.push(ValidationError {
                        path: format!("{path}/tools/{index}"),
                        kind: ValidationErrorKind::UnknownTool { name: tool.clone() },
                    });
                }
            }
        }
    }

    /// A subgraph may not include itself, directly or through other subgraphs.
    pub(super) fn check_subgraph_cycles(&self, errors: &mut Vec<ValidationError>) {
        for (name, subgraph) in &self.subgraphs {
            for (index, node) in subgraph.graph.nodes.iter().enumerate() {
                let cyclic = self.called_subgraphs(node).into_iter().any(|called| {
                    called == name || self.reaches(called, name, &mut HashSet::new())
                });
                if cyclic {
                    errors.push(ValidationError {
                        path: format!(
                            "/subgraphs/{}/graph/nodes/{index}/params",
                            escape_pointer_token(name)
                        ),
                        kind: ValidationErrorKind::SubgraphCycle { name: name.clone() },
                    });
                }
            }
        }
    }

    /// `true` if subgraph `from` includes subgraph `target` (transitively).
    pub(super) fn reaches<'s>(
        &'s self,
        from: &'s str,
        target: &str,
        visited: &mut HashSet<&'s str>,
    ) -> bool {
        if !visited.insert(from) {
            return false;
        }
        let Some(subgraph) = self.subgraphs.get(from) else {
            return false;
        };
        subgraph.graph.nodes.iter().any(|node| {
            self.called_subgraphs(node)
                .into_iter()
                .any(|called| called == target || self.reaches(called, target, visited))
        })
    }
}
