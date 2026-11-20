//! Warnings: findings that never fail the validation.

use grauwolf_graph::{Catalogs, DataType};

use super::Rules;
use super::{GraphScope, SpecWarning};

impl Rules<'_> {
    /// Shell mode of the command tool gives the model a shell (ADR-0099).
    pub(super) fn warn_shell_mode(&self, warnings: &mut Vec<SpecWarning>) {
        let shell = self
            .builtin_tools
            .as_ref()
            .and_then(|builtin| builtin.command.as_ref())
            .is_some_and(|command| command.shell);
        if shell {
            warnings.push(SpecWarning {
                pointer: "/builtin_tools/command/shell".into(),
                message: "shell mode gives the model a shell with the permissions of this \
                          process; prefer an allowlist of programs, and run it only in an \
                          isolated container"
                    .into(),
            });
        }
    }

    /// A node whose answer (a `message`) can carry tool calls and that routes a single call
    /// with `on_tool_call` must also route several calls: `on_tool_call` needs exactly one.
    pub(super) fn warn_missing_multi_call_edge(
        &self,
        scope: &GraphScope<'_>,
        catalogs: &Catalogs,
        warnings: &mut Vec<SpecWarning>,
    ) {
        let leaving = |id: &str, kind: &str| {
            scope.graph.edges.iter().any(|edge| {
                edge.from == id && edge.when.as_ref().is_some_and(|when| when.kind == kind)
            })
        };
        for (index, node) in scope.graph.nodes.iter().enumerate() {
            let answers_with_calls = self
                .node_types(scope, &node.id, &catalogs.nodes)
                .is_some_and(|(_, output)| output == DataType::Message);
            if answers_with_calls
                && leaving(&node.id, "on_tool_call")
                && !leaving(&node.id, "on_multiple_tool_calls")
            {
                warnings.push(SpecWarning {
                    pointer: format!("{}/nodes/{index}", scope.path),
                    message: "answers with several tool calls have no edge; add an \
                              `on_multiple_tool_calls` edge (see the strategies) – otherwise the \
                              run stops with `no matching edge`"
                        .into(),
                });
            }
        }
    }
}
