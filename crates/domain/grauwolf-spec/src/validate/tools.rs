//! Tool names: the tool namespace shared by inline tools, MCP, knowledge, memory and so on
//! (ADR-0025, ADR-0117). Every collision error comes from [`Rules::check_tool_collisions`].

use super::Rules;
use super::{ValidationError, ValidationErrorKind, is_snake_case_identifier};
use crate::{ToolNamespace, ToolOrigin};

impl Rules<'_> {
    /// The tool namespace of the spec without code pieces (they only ever come last).
    pub(super) fn tool_namespace(&self) -> ToolNamespace {
        self.0.tool_namespace()
    }

    /// Reports every tool of `origins` whose name an earlier declaration already has: at the later
    /// declaration, naming the earlier tool (ADR-0117). Names that are reported as invalid ids
    /// (and unknown built-in tools) are left out, so each place gets one error.
    pub(super) fn check_tool_collisions(
        &self,
        namespace: &ToolNamespace,
        origins: &[ToolOrigin],
        errors: &mut Vec<ValidationError>,
    ) {
        for collision in namespace.duplicates() {
            let later = collision.later;
            if !origins.contains(&later.origin) || self.is_reported_elsewhere(later) {
                continue;
            }
            errors.push(ValidationError {
                path: later.pointer.clone(),
                kind: ValidationErrorKind::DuplicateToolName {
                    name: later.name.clone(),
                    other_origin: collision.earlier.origin,
                    other_pointer: collision.earlier.pointer.clone(),
                },
            });
        }
    }

    /// `true` if another error already says what is wrong with this entry's name.
    fn is_reported_elsewhere(&self, entry: &crate::ToolEntry) -> bool {
        match entry.origin {
            ToolOrigin::Mcp | ToolOrigin::McpResource | ToolOrigin::Skill => false,
            ToolOrigin::Builtin => !super::builtin_tools::is_known_builtin(&entry.name),
            _ => !is_snake_case_identifier(&entry.name),
        }
    }

    /// Tool names form their own namespace (ADR-0025) and become Rust identifiers.
    pub(super) fn check_tool_names(&self, errors: &mut Vec<ValidationError>) {
        for (index, tool) in self.tools.iter().enumerate() {
            if !is_snake_case_identifier(&tool.name) {
                errors.push(ValidationError {
                    path: format!("/tools/{index}/name"),
                    kind: ValidationErrorKind::InvalidId {
                        id: tool.name.clone(),
                    },
                });
            }
        }
        self.check_tool_collisions(&self.tool_namespace(), &[ToolOrigin::Own], errors);
    }

    /// An additional tool under `path` needs a snake_case name; that it occurs only once among
    /// all tools is checked with the namespace.
    pub(super) fn check_extra_tool_name(
        &self,
        path: &str,
        name: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        if !is_snake_case_identifier(name) {
            errors.push(ValidationError {
                path: path.into(),
                kind: ValidationErrorKind::InvalidId { id: name.into() },
            });
        }
    }
}
