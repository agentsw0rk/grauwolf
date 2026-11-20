//! Reachability (F13.5): `finish` reachable, every node reachable, no dead ends.

use std::collections::HashSet;

use super::{FINISH, GraphScope, START, ValidationError, ValidationErrorKind};

/// Checks that `finish` and all nodes are reachable from `start` via edges (F13.5);
/// conditions count as possible.
pub(super) fn check_reachability(scope: &GraphScope<'_>, errors: &mut Vec<ValidationError>) {
    let mut reachable: HashSet<&str> = HashSet::from([START]);
    let mut pending = vec![START];
    while let Some(from) = pending.pop() {
        for edge in scope.graph.edges.iter().filter(|edge| edge.from == from) {
            if reachable.insert(edge.to.as_str()) {
                pending.push(edge.to.as_str());
            }
        }
    }
    if !reachable.contains(FINISH) {
        errors.push(ValidationError {
            path: scope.path.clone(),
            kind: ValidationErrorKind::FinishUnreachable,
        });
    }
    // Backwards from `finish`: nodes from which a path leads to the end.
    let mut finishing: HashSet<&str> = HashSet::from([FINISH]);
    let mut pending = vec![FINISH];
    while let Some(to) = pending.pop() {
        for edge in scope.graph.edges.iter().filter(|edge| edge.to == to) {
            if finishing.insert(edge.from.as_str()) {
                pending.push(edge.from.as_str());
            }
        }
    }
    if reachable.contains(FINISH) {
        errors.extend(
            scope
                .graph
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, node)| {
                    reachable.contains(node.id.as_str()) && !finishing.contains(node.id.as_str())
                })
                .map(|(index, node)| ValidationError {
                    path: format!("{}/nodes/{index}", scope.path),
                    kind: ValidationErrorKind::DeadEnd {
                        id: node.id.clone(),
                    },
                }),
        );
    }
    errors.extend(
        scope
            .graph
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| !reachable.contains(node.id.as_str()))
            .map(|(index, node)| ValidationError {
                path: format!("{}/nodes/{index}", scope.path),
                kind: ValidationErrorKind::UnreachableNode {
                    id: node.id.clone(),
                },
            }),
    );
}
