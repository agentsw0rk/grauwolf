//! Code in graphs (ADR-0102): the `code` node, the `on_code` condition and the edge
//! transformation – parameters, types and the code pieces they make.

use std::collections::{BTreeMap, HashMap};

use grauwolf_graph::Catalogs;
use serde_json::Value;

use super::schema::data_type;
use super::{GraphScope, Rules, ValidationErrorKind};
use crate::{
    AgentSpec, CODE_KIND, CodeNodeParams, CodePiece, CodePieceKind, ON_CODE_KIND, OnCodeParams,
};

/// An error at `suffix` below the place that the caller names.
pub(super) type Located = (String, ValidationErrorKind);

impl Rules<'_> {
    /// Problems with the types and the code of a code piece: unknown types and empty code.
    /// `types` are `(suffix, type name)`, the code is at `/code`.
    fn code_errors(&self, types: &[(&str, &str)], code: &str) -> Vec<Located> {
        let mut errors: Vec<Located> = types
            .iter()
            .filter(|(_, name)| !self.is_known_node_type(name))
            .map(|(suffix, name)| {
                (
                    (*suffix).to_owned(),
                    ValidationErrorKind::UnknownType {
                        name: (*name).to_owned(),
                    },
                )
            })
            .collect();
        if code.trim().is_empty() {
            errors.push(("/code".into(), ValidationErrorKind::MissingCode));
        }
        errors
    }

    /// Errors of the parameters of a `code` node, relative to its `params`.
    pub(super) fn code_node_errors(&self, params: Option<&Value>) -> Vec<Located> {
        match CodeNodeParams::parse(params) {
            Ok(parsed) => self.code_errors(
                &[("/input", &parsed.input), ("/output", &parsed.output)],
                &parsed.code,
            ),
            Err(reason) => vec![(
                String::new(),
                ValidationErrorKind::InvalidParams {
                    kind: CODE_KIND.into(),
                    reason,
                },
            )],
        }
    }

    /// Errors of the parameters of an `on_code` condition, relative to its `params`.
    pub(super) fn on_code_errors(&self, params: Option<&Value>) -> Vec<Located> {
        match OnCodeParams::parse(params) {
            Ok(parsed) => self.code_errors(&[("/input", &parsed.input)], &parsed.code),
            Err(reason) => vec![(
                String::new(),
                ValidationErrorKind::InvalidParams {
                    kind: ON_CODE_KIND.into(),
                    reason,
                },
            )],
        }
    }

    /// Static errors of the transformations of a graph: unknown types and missing code.
    pub(super) fn check_transforms(
        &self,
        scope: &GraphScope<'_>,
        errors: &mut Vec<super::ValidationError>,
    ) {
        for (index, edge) in scope.graph.edges.iter().enumerate() {
            let Some(transform) = &edge.transform else {
                continue;
            };
            let mut types = vec![("/output", transform.output.as_str())];
            types.extend(transform.input.as_deref().map(|input| ("/input", input)));
            errors.extend(self.code_errors(&types, &transform.code).into_iter().map(
                |(suffix, kind)| super::ValidationError {
                    path: format!("{}/edges/{index}/transform{suffix}", scope.path),
                    kind,
                },
            ));
        }
    }
}

impl Rules<'_> {
    /// Code pieces are built and shipped next to the tools under their names (ADR-0102), so a
    /// piece must not be named like a tool: reported at the piece's code.
    pub(super) fn check_code_piece_names(&self, errors: &mut Vec<super::ValidationError>) {
        let mut namespace = self.tool_namespace();
        for piece in code_pieces(self.0) {
            namespace.push(piece.name, crate::ToolOrigin::CodePiece, piece.pointer);
        }
        self.check_tool_collisions(&namespace, &[crate::ToolOrigin::CodePiece], errors);
    }
}

/// The code pieces of a validated spec in graph order: nodes, conditions and transformations of
/// the main graph, then of the subgraphs (ADR-0102). Pieces whose parameters or types are
/// invalid are left out; the validation reports them.
pub(crate) fn code_pieces(spec: &AgentSpec) -> Vec<CodePiece> {
    let rules = Rules(spec);
    let catalogs = Catalogs::builtin();
    let labels = std::iter::once("main".to_owned())
        .chain(spec.subgraphs.keys().map(|name| format!("sub_{name}")));
    let mut pieces = Vec::new();
    let mut taken: HashMap<String, usize> = HashMap::new();
    let mut unique = |base: String| {
        let count = taken.entry(base.clone()).or_insert(0);
        *count += 1;
        if *count == 1 {
            base
        } else {
            format!("{base}_{count}")
        }
    };
    for (scope, label) in rules.graph_scopes().into_iter().zip(labels) {
        for (index, node) in scope.graph.nodes.iter().enumerate() {
            if node.kind != CODE_KIND {
                continue;
            }
            let Ok(params) = CodeNodeParams::parse(node.params.as_ref()) else {
                continue;
            };
            if !rules.code_node_errors(node.params.as_ref()).is_empty() {
                continue;
            }
            pieces.push(rules.piece(
                unique(format!("code_{label}_{}", node.id)),
                CodePieceKind::Node,
                format!("{}/nodes/{index}/params/code", scope.path),
                (params.input, params.output),
                params.code,
            ));
        }
        for (index, edge) in scope.graph.edges.iter().enumerate() {
            let ends = format!("{label}_{}_{}", edge.from, edge.to);
            let mut delivered = rules
                .node_types(&scope, &edge.from, &catalogs.nodes)
                .map(|(_, output)| output);
            if let Some(when) = edge.when.as_ref().filter(|when| when.kind == ON_CODE_KIND) {
                if let Ok(params) = OnCodeParams::parse(when.params.as_ref())
                    && rules.on_code_errors(when.params.as_ref()).is_empty()
                {
                    pieces.push(rules.piece(
                        unique(format!("condition_{ends}")),
                        CodePieceKind::Condition,
                        format!("{}/edges/{index}/when/params/code", scope.path),
                        (params.input.clone(), "bool".into()),
                        params.code,
                    ));
                    delivered = Some(data_type(&params.input));
                }
            } else if let Some(when) = &edge.when {
                delivered = delivered.and_then(|input| {
                    catalogs
                        .conditions
                        .get(&when.kind)?
                        .signature(when.params.as_ref().unwrap_or(&Value::Null), &input)
                        .ok()
                });
            }
            let Some(transform) = &edge.transform else {
                continue;
            };
            let input = transform
                .input
                .clone()
                .or_else(|| delivered.map(|data_type| data_type.to_string()));
            let Some(input) = input else { continue };
            let mut types = vec![("/output", transform.output.as_str()), ("/input", &input)];
            types.dedup();
            if !rules.code_errors(&types, &transform.code).is_empty() {
                continue;
            }
            pieces.push(rules.piece(
                unique(format!("transform_{ends}")),
                CodePieceKind::Transform,
                format!("{}/edges/{index}/transform/code", scope.path),
                (input, transform.output.clone()),
                transform.code.clone(),
            ));
        }
    }
    pieces
}

impl Rules<'_> {
    fn piece(
        &self,
        name: String,
        kind: CodePieceKind,
        pointer: String,
        (input, output): (String, String),
        code: String,
    ) -> CodePiece {
        let types: BTreeMap<String, Value> = [&input, &output]
            .into_iter()
            .filter_map(|name| {
                let mut name = name.as_str();
                while let Some(element) = name
                    .strip_prefix("array<")
                    .and_then(|rest| rest.strip_suffix('>'))
                {
                    name = element;
                }
                Some((name.to_owned(), self.types.get(name)?.clone()))
            })
            .collect();
        CodePiece {
            name,
            kind,
            pointer,
            input,
            output,
            code,
            types,
        }
    }
}
