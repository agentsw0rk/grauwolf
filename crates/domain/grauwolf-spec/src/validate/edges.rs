//! Edge type checking: output of the source, condition, input of the target (ADR-0022).

use serde_json::Value;

use grauwolf_graph::{CatalogError, Catalogs, DataType, NodeCatalog};

use super::Rules;
use super::schema::data_type;
use super::{FINISH, GraphScope, START, ValidationError, ValidationErrorKind};
use crate::{
    CODE_KIND, CodeNodeParams, EdgeSpec, FOR_EACH_KIND, ForEachParams, GOAP_KIND, ON_CODE_KIND,
    OnCodeParams, PARALLEL_KIND, PLAN_KIND, SUBGRAPH_KIND,
};

impl Rules<'_> {
    /// Checks that the value of each edge (after its condition) matches the input type of the
    ///
    /// target. Edges with unknown ends or kinds are skipped; they have errors of their own.
    pub(super) fn check_edge_types(
        &self,
        scope: &GraphScope<'_>,
        catalogs: &Catalogs,
        errors: &mut Vec<ValidationError>,
    ) {
        for (index, edge) in scope.graph.edges.iter().enumerate() {
            let mut found = match self.edge_value_type(scope, edge, catalogs) {
                Ok(Some(found)) => found,
                Ok(None) => continue,
                Err(kind) => {
                    errors.push(ValidationError {
                        path: format!("{}/edges/{index}/when", scope.path),
                        kind,
                    });
                    continue;
                }
            };
            if let Some(transform) = &edge.transform {
                if let Some(declared) = &transform.input {
                    let expected = data_type(declared);
                    if expected != found {
                        errors.push(ValidationError {
                            path: format!("{}/edges/{index}/transform/input", scope.path),
                            kind: ValidationErrorKind::TransformInputMismatch { expected, found },
                        });
                    }
                } else if !self.is_known_node_type(&found.to_string()) {
                    errors.push(ValidationError {
                        path: format!("{}/edges/{index}/transform", scope.path),
                        kind: ValidationErrorKind::UnknownType {
                            name: found.to_string(),
                        },
                    });
                }
                found = data_type(&transform.output);
            }
            let Some((expected, _)) = self.node_types(scope, &edge.to, &catalogs.nodes) else {
                continue;
            };
            if found != expected {
                errors.push(ValidationError {
                    path: format!("{}/edges/{index}", scope.path),
                    kind: ValidationErrorKind::TypeMismatch { expected, found },
                });
            }
        }
    }

    /// Type of the value that `edge` passes to its target; `None` if it cannot be determined.
    pub(super) fn edge_value_type(
        &self,
        scope: &GraphScope<'_>,
        edge: &EdgeSpec,
        catalogs: &Catalogs,
    ) -> Result<Option<DataType>, ValidationErrorKind> {
        let Some((_, output)) = self.node_types(scope, &edge.from, &catalogs.nodes) else {
            return Ok(None);
        };
        let Some(condition) = &edge.when else {
            return Ok(Some(output));
        };
        if condition.kind == ON_CODE_KIND {
            let Ok(params) = OnCodeParams::parse(condition.params.as_ref()) else {
                return Ok(None);
            };
            let expected = data_type(&params.input);
            return if expected == output {
                Ok(Some(output))
            } else {
                Err(ValidationErrorKind::ConditionInputMismatch {
                    kind: ON_CODE_KIND.into(),
                    expected,
                    found: output,
                })
            };
        }
        let Some(descriptor) = catalogs.conditions.get(&condition.kind) else {
            return Ok(None);
        };
        match descriptor.signature(condition.params.as_ref().unwrap_or(&Value::Null), &output) {
            Ok(value_type) => Ok(Some(value_type)),
            Err(CatalogError::InputTypeMismatch {
                kind,
                expected,
                found,
            }) => Err(ValidationErrorKind::ConditionInputMismatch {
                kind,
                expected,
                found,
            }),
            Err(CatalogError::InvalidParams { .. }) => Ok(None),
        }
    }

    /// (input, output type) of the node `id`; `start` passes the graph's input on,
    /// `finish` expects its output, a subgraph has the signature of its subgraph.
    pub(super) fn node_types(
        &self,
        scope: &GraphScope<'_>,
        id: &str,
        nodes: &NodeCatalog,
    ) -> Option<(DataType, DataType)> {
        if id == START {
            let input = data_type(scope.input);
            return Some((input.clone(), input));
        }
        if id == FINISH {
            let output = data_type(scope.output);
            return Some((output.clone(), output));
        }
        let node = scope.graph.nodes.iter().find(|node| node.id == id)?;
        if node.kind == SUBGRAPH_KIND {
            let (_, subgraph) = self.subgraph_of(node).ok()?;
            return Some((data_type(&subgraph.input), data_type(&subgraph.output)));
        }
        if node.kind == CODE_KIND {
            let params = CodeNodeParams::parse(node.params.as_ref()).ok()?;
            return Some((data_type(&params.input), data_type(&params.output)));
        }
        if node.kind == PLAN_KIND || node.kind == GOAP_KIND {
            return Some((DataType::String, DataType::String));
        }
        if node.kind == FOR_EACH_KIND {
            let params = ForEachParams::parse(node.params.as_ref()).ok()?;
            return Some((data_type(&params.input), data_type(&params.output)));
        }
        if node.kind == PARALLEL_KIND {
            let branch = self.parallel_of(node).ok()?;
            return Some((data_type(&branch.input), data_type(&branch.output)));
        }
        nodes
            .get(&node.kind)?
            .signature(node.params.as_ref().unwrap_or(&Value::Null))
            .ok()
    }
}
