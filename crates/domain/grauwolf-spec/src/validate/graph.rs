//! Graph structure: scopes, node IDs and kinds, condition kinds, edge endpoints.

use std::collections::HashSet;

use serde_json::Value;

use grauwolf_graph::params::check_params;
use grauwolf_graph::{CatalogError, Catalogs, DataType, NodeCatalog};

use super::Rules;
use super::{
    FINISH, GraphScope, START, ValidationError, ValidationErrorKind, check_decisions,
    check_reachability, escape_pointer_token, is_snake_case_identifier,
};
use crate::{
    AGENT_KIND, CODE_KIND, FOR_EACH_KIND, GOAP_KIND, ON_CODE_KIND, PARALLEL_KIND, PLAN_KIND,
    SUBGRAPH_KIND,
};

impl Rules<'_> {
    /// The main graph and all subgraphs, each with its signature and path (ADR-0034).
    pub(super) fn graph_scopes(&self) -> Vec<GraphScope<'_>> {
        let main = GraphScope {
            graph: &self.graph,
            input: &self.input,
            output: &self.output,
            path: "/graph".into(),
        };
        let subgraphs = self.subgraphs.iter().map(|(name, subgraph)| GraphScope {
            graph: &subgraph.graph,
            input: &subgraph.input,
            output: &subgraph.output,
            path: format!("/subgraphs/{}/graph", escape_pointer_token(name)),
        });
        std::iter::once(main).chain(subgraphs).collect()
    }

    /// Checks a graph: IDs, kinds, parameters, conditions, edge types and ends.
    pub(super) fn check_graph(
        &self,
        scope: &GraphScope<'_>,
        catalogs: &Catalogs,
        errors: &mut Vec<ValidationError>,
    ) {
        let mut seen = HashSet::new();
        for (index, node) in scope.graph.nodes.iter().enumerate() {
            let path = format!("{}/nodes/{index}/id", scope.path);
            let id = node.id.clone();
            let kind = if !is_snake_case_identifier(&node.id) {
                Some(ValidationErrorKind::InvalidId { id })
            } else if [START, FINISH].contains(&node.id.as_str()) {
                Some(ValidationErrorKind::ReservedNodeId { id })
            } else if !seen.insert(node.id.as_str()) {
                Some(ValidationErrorKind::DuplicateNodeId { id })
            } else {
                None
            };
            errors.extend(kind.map(|kind| ValidationError { path, kind }));
        }
        self.check_node_kinds(scope, &catalogs.nodes, errors);
        self.check_condition_kinds(scope, catalogs, errors);
        self.check_transforms(scope, errors);
        self.check_edge_types(scope, catalogs, errors);
        self.check_edge_endpoints(scope, errors);
        check_decisions(scope, errors);
        check_reachability(scope, errors);
    }

    /// Node kinds must be in the catalog or be `subgraph`; their parameters must yield a
    /// signature that names only declared types and subgraphs.
    pub(super) fn check_node_kinds(
        &self,
        scope: &GraphScope<'_>,
        catalog: &NodeCatalog,
        errors: &mut Vec<ValidationError>,
    ) {
        for (index, node) in scope.graph.nodes.iter().enumerate() {
            let params_path = format!("{}/nodes/{index}/params", scope.path);
            if node.kind == SUBGRAPH_KIND {
                if let Err(kind) = self.subgraph_of(node) {
                    errors.push(ValidationError {
                        path: params_path,
                        kind,
                    });
                }
                continue;
            }
            if node.kind == PARALLEL_KIND {
                if let Err(kind) = self.parallel_of(node) {
                    errors.push(ValidationError {
                        path: params_path,
                        kind,
                    });
                }
                continue;
            }
            if node.kind == FOR_EACH_KIND {
                errors.extend(
                    self.for_each_errors(node)
                        .into_iter()
                        .map(|(suffix, kind)| ValidationError {
                            path: format!("{params_path}{suffix}"),
                            kind,
                        }),
                );
                continue;
            }
            if node.kind == CODE_KIND {
                errors.extend(self.code_node_errors(node.params.as_ref()).into_iter().map(
                    |(suffix, kind)| ValidationError {
                        path: format!("{params_path}{suffix}"),
                        kind,
                    },
                ));
                continue;
            }
            if node.kind == PLAN_KIND {
                if let Err(kind) = self.plan_of(node) {
                    errors.push(ValidationError {
                        path: params_path,
                        kind,
                    });
                }
                continue;
            }
            if node.kind == GOAP_KIND {
                if let Err(kind) = self.goap_of(node) {
                    errors.push(ValidationError {
                        path: params_path,
                        kind,
                    });
                }
                continue;
            }
            let Some(descriptor) = catalog.get(&node.kind) else {
                errors.push(ValidationError {
                    path: format!("{}/nodes/{index}/kind", scope.path),
                    kind: ValidationErrorKind::UnknownNodeKind {
                        kind: node.kind.clone(),
                    },
                });
                continue;
            };
            let params = node.params.as_ref().unwrap_or(&Value::Null);
            if let Err(reason) = check_params(&descriptor.params_schema(), params) {
                errors.push(ValidationError {
                    path: params_path,
                    kind: ValidationErrorKind::InvalidParams {
                        kind: node.kind.clone(),
                        reason,
                    },
                });
                continue;
            }
            match descriptor.signature(params) {
                Ok((input, output)) => {
                    let mut named: Vec<String> = [input, output]
                        .into_iter()
                        .filter_map(|data_type| match element_type(data_type) {
                            DataType::Named(name) if !self.types.contains_key(&name) => Some(name),
                            _ => None,
                        })
                        .collect();
                    named.dedup();
                    errors.extend(named.into_iter().map(|name| ValidationError {
                        path: params_path.clone(),
                        kind: ValidationErrorKind::UnknownType { name },
                    }));
                    if node.kind == AGENT_KIND {
                        errors.extend(self.check_agent_name(params, &params_path));
                    }
                }
                Err(CatalogError::InvalidParams { kind, reason }) => errors.push(ValidationError {
                    path: params_path,
                    kind: ValidationErrorKind::InvalidParams { kind, reason },
                }),
                Err(CatalogError::InputTypeMismatch { .. }) => {}
            }
        }
    }

    pub(super) fn check_condition_kinds(
        &self,
        scope: &GraphScope<'_>,
        catalogs: &Catalogs,
        errors: &mut Vec<ValidationError>,
    ) {
        for (index, edge) in scope.graph.edges.iter().enumerate() {
            let Some(condition) = &edge.when else {
                continue;
            };
            if condition.kind == ON_CODE_KIND {
                errors.extend(
                    self.on_code_errors(condition.params.as_ref())
                        .into_iter()
                        .map(|(suffix, kind)| ValidationError {
                            path: format!("{}/edges/{index}/when/params{suffix}", scope.path),
                            kind,
                        }),
                );
                continue;
            }
            let Some(descriptor) = catalogs.conditions.get(&condition.kind) else {
                continue;
            };
            let params = condition.params.as_ref().unwrap_or(&Value::Null);
            if let Err(reason) = check_params(&descriptor.params_schema(), params) {
                errors.push(ValidationError {
                    path: format!("{}/edges/{index}/when/params", scope.path),
                    kind: ValidationErrorKind::InvalidParams {
                        kind: condition.kind.clone(),
                        reason,
                    },
                });
            }
        }
        errors.extend(
            scope
                .graph
                .edges
                .iter()
                .enumerate()
                .filter_map(|(index, edge)| {
                    let condition = edge.when.as_ref()?;
                    (condition.kind != ON_CODE_KIND).then_some(())?;
                    catalogs
                        .conditions
                        .get(&condition.kind)
                        .is_none()
                        .then(|| ValidationError {
                            path: format!("{}/edges/{index}/when/kind", scope.path),
                            kind: ValidationErrorKind::UnknownConditionKind {
                                kind: condition.kind.clone(),
                            },
                        })
                }),
        );
    }

    pub(super) fn check_edge_endpoints(
        &self,
        scope: &GraphScope<'_>,
        errors: &mut Vec<ValidationError>,
    ) {
        let known: HashSet<&str> = scope
            .graph
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .chain([START, FINISH])
            .collect();

        errors.extend(
            scope
                .graph
                .edges
                .iter()
                .enumerate()
                .flat_map(|(index, edge)| [(index, "from", &edge.from), (index, "to", &edge.to)])
                .filter(|(_, _, id)| !known.contains(id.as_str()))
                .map(|(index, end, id)| ValidationError {
                    path: format!("{}/edges/{index}/{end}", scope.path),
                    kind: ValidationErrorKind::UnknownNode { id: id.clone() },
                }),
        );
    }
}

/// The innermost element type of `array<…>`; other types as they are.
pub(super) fn element_type(data_type: DataType) -> DataType {
    match data_type {
        DataType::Array(element) => element_type(*element),
        other => other,
    }
}
