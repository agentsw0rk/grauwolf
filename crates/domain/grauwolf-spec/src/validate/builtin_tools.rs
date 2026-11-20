//! Built-in tools (ADR-0046, ADR-0098, ADR-0104): sections, roots, hosts and URLs.

use super::Rules;
use super::escape_pointer_token;
use super::sections::check_spiffe;
use super::{ValidationError, ValidationErrorKind};
use crate::{BuiltinGroup, ToolOrigin};

/// `true` if `name` is a tool of one of the built-in groups.
pub(super) fn is_known_builtin(name: &str) -> bool {
    BuiltinGroup::of_tool(name).is_some()
}

impl Rules<'_> {
    /// Built-in tools must be known, must not take a tool name twice, and
    /// file tools need a root.
    pub(super) fn check_builtin_tools(&self, errors: &mut Vec<ValidationError>) {
        let Some(builtin) = &self.builtin_tools else {
            return;
        };
        for (index, name) in builtin.tools.iter().enumerate() {
            if !is_known_builtin(name) {
                errors.push(ValidationError {
                    path: format!("/builtin_tools/tools/{index}"),
                    kind: ValidationErrorKind::UnknownTool { name: name.clone() },
                });
            }
        }
        self.check_tool_collisions(&self.tool_namespace(), &[ToolOrigin::Builtin], errors);
        let needs_root = builtin.groups().iter().any(|group| group.info().needs_root);
        if needs_root && builtin.root.is_none() {
            errors.push(ValidationError {
                path: "/builtin_tools/root".into(),
                kind: ValidationErrorKind::MissingBuiltinRoot,
            });
        }
        check_atlassian_git(builtin, errors);
        check_auth_tables(builtin, errors);
        if let Some(command) = &builtin.command {
            check_command(command, errors);
        }
    }
}

/// Sections of the Jira, Confluence and git tools (ADR-0104).
pub(super) fn check_atlassian_git(
    builtin: &crate::BuiltinToolsSpec,
    errors: &mut Vec<ValidationError>,
) {
    let mut push = |path: &str, kind| {
        errors.push(ValidationError {
            path: path.to_owned(),
            kind,
        });
    };
    for group in builtin.groups() {
        if let Some(section) = group.info().section
            && !builtin.has_section(section)
        {
            push(
                &format!("/builtin_tools/{section}"),
                ValidationErrorKind::MissingBuiltinSection {
                    group: section.to_owned(),
                },
            );
        }
    }
    for (group, section) in [("jira", &builtin.jira), ("confluence", &builtin.confluence)] {
        if let Some(reason) = section.as_ref().and_then(|s| url_problem(&s.base_url)) {
            push(
                &format!("/builtin_tools/{group}/base_url"),
                ValidationErrorKind::InvalidUrl { reason },
            );
        }
    }
    if let Some(fetch) = &builtin.fetch {
        if fetch.allowed_hosts.is_empty() {
            push(
                "/builtin_tools/fetch/allowed_hosts",
                ValidationErrorKind::EmptyAllowedHosts,
            );
        }
        for (index, host) in fetch.allowed_hosts.iter().enumerate() {
            let body = host.strip_prefix("*.").unwrap_or(host);
            if body.is_empty() || body.contains(['/', ':', ' ', '@', '*']) && !body.contains("::") {
                push(
                    &format!("/builtin_tools/fetch/allowed_hosts/{index}"),
                    ValidationErrorKind::InvalidAllowedHost { host: host.clone() },
                );
            }
        }
    }
    if let Some(git) = &builtin.git {
        if let Some(reason) = url_problem(&git.repo_url) {
            push(
                "/builtin_tools/git/repo_url",
                ValidationErrorKind::InvalidUrl { reason },
            );
        }
        let directory = std::path::Path::new(git.directory_or_default());
        if directory.is_absolute()
            || directory.components().any(|part| {
                !matches!(
                    part,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            })
        {
            push(
                "/builtin_tools/git/directory",
                ValidationErrorKind::DirectoryOutsideRoot,
            );
        }
    }
}

/// `auth` tables of `fetch` and the git tools, and `workload_api` (ADR-0110).
fn check_auth_tables(builtin: &crate::BuiltinToolsSpec, errors: &mut Vec<ValidationError>) {
    let mut push = |path: &str, kind| {
        errors.push(ValidationError {
            path: path.to_owned(),
            kind,
        });
    };
    let mut specs: Vec<(String, &crate::SpiffeAuthSpec)> = Vec::new();
    if let Some(fetch) = &builtin.fetch {
        for (host, auth) in &fetch.auth {
            let key = escape_pointer_token(host);
            let base = format!("/builtin_tools/fetch/auth/{key}");
            if !fetch
                .allowed_hosts
                .iter()
                .any(|entry| entry.trim().eq_ignore_ascii_case(host.trim()))
            {
                push(
                    &base,
                    ValidationErrorKind::AuthHostNotAllowed { host: host.clone() },
                );
            }
            specs.push((base, &auth.spiffe));
        }
    }
    if let Some(git) = &builtin.git {
        if git.use_token && !git.auth.is_empty() {
            push(
                "/builtin_tools/git/auth",
                ValidationErrorKind::GitTokenWithAuth,
            );
        }
        let repo_host = crate::origin_host(&git.repo_url);
        for (host, auth) in &git.auth {
            let base = format!("/builtin_tools/git/auth/{}", escape_pointer_token(host));
            if !repo_host
                .as_deref()
                .is_some_and(|repo| crate::host_matches(host, repo))
            {
                push(
                    &base,
                    ValidationErrorKind::AuthHostNotRepository { host: host.clone() },
                );
            }
            specs.push((base, &auth.spiffe));
        }
    }
    for (base, spiffe) in specs {
        check_spiffe(&base, spiffe, errors);
    }
}

/// The `command` section (ADR-0099).
fn check_command(command: &crate::CommandSpec, errors: &mut Vec<ValidationError>) {
    let mut push = |path: String, kind| errors.push(ValidationError { path, kind });
    let base = "/builtin_tools/command";
    if command.allowed.is_empty() {
        push(
            format!("{base}/allowed"),
            ValidationErrorKind::EmptyAllowedCommands,
        );
    }
    if command.shell && command.workload_api {
        push(
            format!("{base}/workload_api"),
            ValidationErrorKind::WorkloadApiWithShell,
        );
    }
    if command.shell {
        if command.allowed != ["*"] {
            push(
                format!("{base}/allowed"),
                ValidationErrorKind::ShellNeedsWildcard,
            );
        }
        if !command.i_understand_shell {
            push(
                format!("{base}/i_understand_shell"),
                ValidationErrorKind::ShellNotAcknowledged,
            );
        }
    } else {
        for (index, program) in command.allowed.iter().enumerate() {
            let kind = if program == "*" {
                ValidationErrorKind::WildcardWithoutShell
            } else if program.is_empty()
                || program.contains(char::is_whitespace)
                || (program.contains('/') && !program.starts_with('/'))
                || program.ends_with('/')
            {
                ValidationErrorKind::InvalidAllowedProgram {
                    program: program.clone(),
                }
            } else {
                continue;
            };
            push(format!("{base}/allowed/{index}"), kind);
        }
    }
    let limits = [
        ("timeout_secs", command.timeout_secs, 3600),
        ("max_output_bytes", command.max_output_bytes, 10_485_760),
    ];
    for (field, value, max) in limits {
        if value.is_some_and(|value| value == 0 || value > max) {
            push(
                format!("{base}/{field}"),
                ValidationErrorKind::CommandLimitOutOfRange {
                    field: field.into(),
                },
            );
        }
    }
}

/// Why `url` is not an `https://` URL with a host and without user info, if it is not.
pub(super) fn url_problem(url: &str) -> Option<String> {
    let Some(rest) = url.strip_prefix("https://") else {
        return Some("must start with https://".to_owned());
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return Some("must not contain user info (user:password@)".to_owned());
    }
    if authority.is_empty() {
        return Some("has no host".to_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::url_problem;

    #[test]
    fn accepts_an_https_url_with_a_host() {
        assert_eq!(url_problem("https://example.atlassian.net/wiki?x=1"), None);
    }

    #[test]
    fn rejects_plain_http_user_info_and_a_missing_host() {
        assert!(url_problem("http://example.com").is_some_and(|r| r.contains("https://")));
        assert!(
            url_problem("https://user:pw@example.com").is_some_and(|r| r.contains("user info"))
        );
        assert!(url_problem("https:///path").is_some_and(|r| r.contains("no host")));
    }
}
