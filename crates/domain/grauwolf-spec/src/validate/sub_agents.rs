//! The declared types of `agent` nodes and `for_each` agent bodies against the signatures of the
//! sub-agents' own specs (ADR-0101). One spec cannot read the others, so the caller that can
//! (`spec validate`, the loader) hands the signatures in.

use std::collections::BTreeMap;

use grauwolf_graph::DataType;

use super::{GraphScope, Rules, ValidationError, ValidationErrorKind};
use crate::{AGENT_KIND, AgentSpec, FOR_EACH_KIND, ForEachBody, ForEachParams};

/// Errors for nodes whose declared types differ from `signatures` (agent name → (input, output)
/// of the sub-agent's spec); agents without a signature are not checked.
pub(crate) fn check_sub_agent_types(
    spec: &AgentSpec,
    signatures: &BTreeMap<String, (String, String)>,
) -> Vec<ValidationError> {
    let rules = Rules(spec);
    let mut errors = Vec::new();
    for scope in rules.graph_scopes() {
        check_scope(&scope, signatures, &mut errors);
    }
    errors
}

fn check_scope(
    scope: &GraphScope<'_>,
    signatures: &BTreeMap<String, (String, String)>,
    errors: &mut Vec<ValidationError>,
) {
    for (index, node) in scope.graph.nodes.iter().enumerate() {
        let path = format!("{}/nodes/{index}/params", scope.path);
        let params = node.params.as_ref();
        // (agent, declared input, declared output); `None` = nothing to compare.
        let declared: Option<(String, Option<String>, Option<String>)> = match node.kind.as_str() {
            AGENT_KIND => params.and_then(|params| {
                let name = params["agent"].as_str()?.to_owned();
                let type_of = |key: &str| Some(params[key].as_str().unwrap_or("string").to_owned());
                Some((name, type_of("input"), type_of("output")))
            }),
            FOR_EACH_KIND => ForEachParams::parse(params).ok().and_then(|parsed| {
                let ForEachBody::Agent(name) = parsed.body else {
                    return None;
                };
                let element = |list: &str| match DataType::from_name(list) {
                    DataType::Array(element) => Some(element.to_string()),
                    _ => None,
                };
                Some((name, element(&parsed.input), element(&parsed.output)))
            }),
            _ => None,
        };
        let Some((agent, input, output)) = declared else {
            continue;
        };
        let Some((actual_input, actual_output)) = signatures.get(&agent) else {
            continue;
        };
        for (field, declared, actual) in [
            ("input", input, actual_input),
            ("output", output, actual_output),
        ] {
            if let Some(declared) = declared.filter(|declared| declared != actual) {
                errors.push(ValidationError {
                    path: format!("{path}/{field}"),
                    kind: ValidationErrorKind::SubAgentTypeMismatch {
                        agent: agent.clone(),
                        field: field.into(),
                        declared,
                        actual: actual.clone(),
                    },
                });
            }
        }
    }
}
