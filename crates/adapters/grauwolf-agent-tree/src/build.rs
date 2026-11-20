//! Building the code of a spec (ADR-0102).

use std::path::Path;

use grauwolf_spec::{AgentSpec, AgentSpecExt};
use grauwolf_toolbuild::{BuildError, ToolBuilder};
use grauwolf_tools::ToolArtifact;

/// Builds the tool code of the spec `file` with `builder`, then the code of its graphs
/// (`code` nodes, `on_code` conditions and edge transformations, ADR-0102): one artifact per
/// tool and per piece, the latter named like the piece (`AgentSpecExt::code_pieces`).
pub fn build_tools(
    file: &Path,
    spec: &AgentSpec,
    builder: &ToolBuilder,
) -> Result<Vec<ToolArtifact>, String> {
    builder
        .prepare()
        .map_err(|error| format!("tool dependencies not prepared: {error}"))?;
    let tools = spec.tools.iter().enumerate().map(|(index, tool)| {
        builder
            .build(tool)
            .map_err(|error| build_error(file, &format!("/tools/{index}/code"), error))
    });
    let pieces = spec.code_pieces().into_iter().map(|piece| {
        builder
            .build_code(&piece)
            .map_err(|error| build_error(file, &piece.pointer, error))
    });
    tools.chain(pieces).collect()
}

/// Compile errors with position point to the place in the spec like validation errors:
/// `<file>:<pointer>:<line>:<column>: <level>: <message>`, e.g. `/tools/<i>/code` or
/// `/graph/nodes/<i>/params/code` (F13.9, ADR-0102).
fn build_error(file: &Path, pointer: &str, error: BuildError) -> String {
    match &error {
        BuildError::Compile { diagnostics, .. }
            if diagnostics
                .iter()
                .any(|diagnostic| diagnostic.line.is_some()) =>
        {
            diagnostics
                .iter()
                .map(|diagnostic| match (diagnostic.line, diagnostic.column) {
                    (Some(line), Some(column)) => format!(
                        "{}:{pointer}:{line}:{column}: {}: {}",
                        file.display(),
                        diagnostic.level,
                        diagnostic.message
                    ),
                    _ => format!(
                        "{}:{pointer}: {}: {}",
                        file.display(),
                        diagnostic.level,
                        diagnostic.message
                    ),
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        BuildError::ForbiddenCode { .. } | BuildError::InvalidCode { .. } => {
            format!("{}:{pointer}: {error}", file.display())
        }
        _ => format!("{}: {error}", file.display()),
    }
}
