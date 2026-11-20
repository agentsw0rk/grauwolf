//! Skill sources (ADR-0103): one source per entry, a usable directory or git source and valid
//! names. What is inside the folders is checked when they are loaded.

use super::Rules;
use super::{ValidationError, ValidationErrorKind};
use crate::ToolOrigin;

/// `true` if `name` is a usable skill name: lowercase letters, digits, `-` and `_`, starting with
/// a letter or digit.
pub fn is_skill_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn relative_folder_problem(path: &str) -> bool {
    path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

impl Rules<'_> {
    pub(super) fn check_skills(&self, errors: &mut Vec<ValidationError>) {
        // The skill tool names are reserved as soon as there are skills.
        self.check_tool_collisions(&self.tool_namespace(), &[ToolOrigin::Skill], errors);
        let mut push = |path: String, kind| errors.push(ValidationError { path, kind });
        for (index, source) in self.skills.iter().enumerate() {
            let path = format!("/skills/{index}");
            match (&source.dir, &source.git) {
                (Some(dir), None) => {
                    if relative_folder_problem(dir) {
                        push(
                            format!("{path}/dir"),
                            ValidationErrorKind::InvalidGitSource {
                                reason:
                                    "must be a relative directory without `..`, empty or `.` parts"
                                        .to_owned(),
                            },
                        );
                    }
                    for (field, used) in [
                        ("path", source.path.is_some()),
                        ("rev", source.rev.is_some()),
                        ("tag", source.tag.is_some()),
                        ("branch", source.branch.is_some()),
                    ] {
                        if used {
                            push(
                                format!("{path}/{field}"),
                                ValidationErrorKind::InvalidGitSource {
                                    reason: format!("`{field}` belongs to `git` sources"),
                                },
                            );
                        }
                    }
                }
                (None, Some(_)) => {
                    if let Err(problem) = crate::GitSource::from_skill_source(source) {
                        push(
                            format!("{path}/{}", problem.field),
                            ValidationErrorKind::InvalidGitSource {
                                reason: problem.reason,
                            },
                        );
                    }
                }
                _ => push(path.clone(), ValidationErrorKind::SkillSource),
            }
            for (name_index, name) in source.names.iter().flatten().enumerate() {
                if !is_skill_name(name) {
                    push(
                        format!("{path}/names/{name_index}"),
                        ValidationErrorKind::InvalidId { id: name.clone() },
                    );
                }
            }
        }
    }
}
