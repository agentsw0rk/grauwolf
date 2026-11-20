//! Tool packages (ADR-0107): OCI references, selection and renaming.

use super::Rules;
use super::{ValidationError, ValidationErrorKind, escape_pointer_token, is_snake_case_identifier};
use crate::ToolOrigin;

impl Rules<'_> {
    /// Tool packages (ADR-0107): one source, a valid reference, snake_case names that are
    /// unique among all tools. Fetching is not part of validation, so the ABI version of a
    /// package and the names of unselected tools are checked when it is resolved.
    pub(super) fn check_tool_packages(&self, errors: &mut Vec<ValidationError>) {
        let mut push = |path: String, kind| errors.push(ValidationError { path, kind });
        for (index, package) in self.tool_packages.iter().enumerate() {
            let path = format!("/tool_packages/{index}");
            match (&package.oci, &package.git) {
                (Some(reference), None) => {
                    if let Err(reason) = crate::OciReference::parse(reference) {
                        push(
                            format!("{path}/oci"),
                            ValidationErrorKind::InvalidOciReference { reason },
                        );
                    }
                }
                (None, Some(_)) => {
                    if let Err(problem) = crate::GitSource::from_entry(package) {
                        push(
                            format!("{path}/{}", problem.field),
                            ValidationErrorKind::InvalidGitSource {
                                reason: problem.reason,
                            },
                        );
                    }
                }
                _ => push(path.clone(), ValidationErrorKind::ToolPackageSource),
            }
            if package.git.is_none() {
                for (field, used) in [
                    ("path", package.path.is_some()),
                    ("rev", package.rev.is_some()),
                    ("tag", package.tag.is_some()),
                    ("branch", package.branch.is_some()),
                ] {
                    if used && package.oci.is_some() {
                        push(
                            format!("{path}/{field}"),
                            ValidationErrorKind::InvalidGitSource {
                                reason: format!("`{field}` belongs to `git` sources"),
                            },
                        );
                    }
                }
            }
            for (tool_index, tool) in package.tools.iter().flatten().enumerate() {
                if !is_snake_case_identifier(tool) {
                    push(
                        format!("{path}/tools/{tool_index}"),
                        ValidationErrorKind::InvalidId { id: tool.clone() },
                    );
                }
            }
            for (from, to) in &package.renames {
                let at = format!("{path}/as/{}", escape_pointer_token(from));
                let selected = package
                    .tools
                    .as_ref()
                    .is_none_or(|tools| tools.contains(from));
                if !selected {
                    push(at, ValidationErrorKind::UnknownTool { name: from.clone() });
                } else if !is_snake_case_identifier(to) {
                    push(at, ValidationErrorKind::InvalidId { id: to.clone() });
                }
            }
        }
        self.check_tool_collisions(&self.tool_namespace(), &[ToolOrigin::Package], errors);
    }
}
