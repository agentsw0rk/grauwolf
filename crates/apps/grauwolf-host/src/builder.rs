//! Checker for the builder agent that compiles tool code with `grauwolf-toolbuild`
//! (F13.9, ADR-0035).

use std::path::PathBuf;

use futures_util::future::BoxFuture;
use grauwolf_builder_agent::{SpecChecker, SpecIssue};
use grauwolf_spec::AgentSpec;
use grauwolf_toolbuild::{BuildError, ToolBuilder};

/// Builds all tools of a spec; compile errors become issues with path and line.
#[derive(Debug, Clone)]
pub struct ToolBuildChecker {
    builder: ToolBuilder,
}

impl ToolBuildChecker {
    /// Checker with a tool cache in the directory `cache`.
    pub fn new(cache: impl Into<PathBuf>) -> Self {
        Self {
            builder: ToolBuilder::new(cache.into()),
        }
    }

    /// Checker with a fully configured `builder`, e.g. with an OS wrapper (ADR-0030).
    pub fn from_builder(builder: ToolBuilder) -> Self {
        Self { builder }
    }

    /// Builds the tools synchronously (cargo processes) and collects the issues.
    fn issues(&self, spec: &AgentSpec) -> Vec<SpecIssue> {
        if spec.tools.is_empty() {
            return Vec::new();
        }
        if let Err(error) = self.builder.prepare() {
            return vec![SpecIssue {
                path: "/tools".into(),
                message: format!("tool dependencies not prepared: {error}"),
            }];
        }
        spec.tools
            .iter()
            .enumerate()
            .flat_map(|(index, tool)| match self.builder.build(tool) {
                Ok(_) => Vec::new(),
                Err(error) => issues_of(index, error),
            })
            .collect()
    }
}

/// Issues for a build error of the tool `index`.
fn issues_of(index: usize, error: BuildError) -> Vec<SpecIssue> {
    let path = format!("/tools/{index}/code");
    match error {
        BuildError::Compile { diagnostics, .. } if !diagnostics.is_empty() => diagnostics
            .into_iter()
            .map(|diagnostic| SpecIssue {
                path: path.clone(),
                message: match (diagnostic.line, diagnostic.column) {
                    (Some(line), Some(column)) => format!(
                        "line {line}, column {column}: {}: {}",
                        diagnostic.level, diagnostic.message
                    ),
                    _ => format!("{}: {}", diagnostic.level, diagnostic.message),
                },
            })
            .collect(),
        other => vec![SpecIssue {
            path,
            message: other.to_string(),
        }],
    }
}

impl SpecChecker for ToolBuildChecker {
    fn check<'a>(&'a self, spec: &'a AgentSpec) -> BoxFuture<'a, Vec<SpecIssue>> {
        Box::pin(async move {
            let checker = self.clone();
            let spec = spec.clone();
            // cargo blocks; do not wait on a worker of the runtime.
            tokio::task::spawn_blocking(move || checker.issues(&spec))
                .await
                .unwrap_or_else(|error| {
                    vec![SpecIssue {
                        path: "/tools".into(),
                        message: format!("tool build aborted: {error}"),
                    }]
                })
        })
    }
}
