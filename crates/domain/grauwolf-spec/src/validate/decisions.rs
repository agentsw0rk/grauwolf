//! Decisions (ADR-0100): `on_decision` edges and the coverage of `decide` answers.

use std::collections::HashSet;

use super::{GraphScope, ValidationError, ValidationErrorKind};
use crate::EdgeSpec;

/// Checks decisions (ADR-0100): `on_decision` only leaves `decide` nodes, `decide` nodes are
/// only left through `on_decision`, every answer exists and has exactly one edge.
pub(super) fn check_decisions(scope: &GraphScope<'_>, errors: &mut Vec<ValidationError>) {
    let edges = &scope.graph.edges;
    let is_decision_edge = |edge: &EdgeSpec| {
        edge.when
            .as_ref()
            .is_some_and(|when| when.kind == "on_decision")
    };
    let decides = |id: &str| {
        scope
            .graph
            .nodes
            .iter()
            .find(|node| node.id == id && node.kind == "decide")
    };
    for (index, edge) in edges.iter().enumerate() {
        if is_decision_edge(edge) && decides(&edge.from).is_none() {
            errors.push(ValidationError {
                path: format!("{}/edges/{index}/when", scope.path),
                kind: ValidationErrorKind::DecisionEdgeSource,
            });
        }
    }
    for (node_index, node) in scope.graph.nodes.iter().enumerate() {
        if node.kind != "decide" {
            continue;
        }
        let Some(answers) = node
            .params
            .as_ref()
            .and_then(grauwolf_graph::decision_answers)
        else {
            continue;
        };
        let mut covered: HashSet<&str> = HashSet::new();
        for (index, edge) in edges
            .iter()
            .enumerate()
            .filter(|(_, edge)| edge.from == node.id)
        {
            let edge_path = format!("{}/edges/{index}", scope.path);
            if !is_decision_edge(edge) {
                errors.push(ValidationError {
                    path: edge_path,
                    kind: ValidationErrorKind::DecisionEdgeCondition {
                        node: node.id.clone(),
                    },
                });
                continue;
            }
            let listed = edge
                .when
                .as_ref()
                .and_then(|when| when.params.as_ref())
                .and_then(|params| params["answers"].as_array())
                .into_iter()
                .flatten()
                .enumerate()
                .filter_map(|(position, answer)| Some((position, answer.as_str()?)));
            for (position, answer) in listed {
                let error = if !answers.iter().any(|known| known == answer) {
                    Some((
                        format!("{edge_path}/when/params/answers/{position}"),
                        ValidationErrorKind::UnknownDecisionAnswer {
                            node: node.id.clone(),
                            answer: answer.to_owned(),
                        },
                    ))
                } else if !covered.insert(answer) {
                    Some((
                        edge_path.clone(),
                        ValidationErrorKind::DecisionAnswerDuplicated {
                            node: node.id.clone(),
                            answer: answer.to_owned(),
                        },
                    ))
                } else {
                    None
                };
                errors.extend(error.map(|(path, kind)| ValidationError { path, kind }));
            }
        }
        errors.extend(
            answers
                .iter()
                .filter(|answer| !covered.contains(answer.as_str()))
                .map(|answer| ValidationError {
                    path: format!("{}/nodes/{node_index}", scope.path),
                    kind: ValidationErrorKind::DecisionAnswerUncovered {
                        node: node.id.clone(),
                        answer: answer.clone(),
                    },
                }),
        );
    }
}
