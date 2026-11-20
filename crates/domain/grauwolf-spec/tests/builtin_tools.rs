//! Built-in tools in specs (F03.13, F03.14, ADR-0046).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, PushPolicy, ToolOrigin, ValidationError, ValidationErrorKind};
use serde_json::{Value, json};

fn spec(builtin_tools: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "filer",
            "model": { "provider": "ollama", "id": "m" },
            "tools": [ { "name": "read_file", "description": "eigenes", "args": { "type": "object" }, "code": "Ok(String::new())" } ],
            "builtin_tools": builtin_tools,
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap()
}

#[test]
fn accepts_known_builtin_tools_with_a_root_for_file_tools() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "filer",
            "model": { "provider": "ollama", "id": "m" },
            "builtin_tools": { "tools": ["list_directory", "write_file", "ask_user"], "root": "workspace" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    let builtin = spec.builtin_tools.as_ref().unwrap();
    assert_eq!(builtin.tools, ["list_directory", "write_file", "ask_user"]);
    assert_eq!(builtin.root.as_deref(), Some("workspace"));
    assert_eq!(spec.validate(), Ok(()));
}

#[test]
fn rejects_unknown_tools_name_collisions_and_file_tools_without_root() {
    let errors = spec(json!({ "tools": ["shell", "read_file", "say_to_user"] }))
        .validate()
        .unwrap_err();

    assert_eq!(
        errors,
        [
            ValidationError {
                path: "/builtin_tools/tools/0".into(),
                kind: ValidationErrorKind::UnknownTool {
                    name: "shell".into()
                },
            },
            ValidationError {
                path: "/builtin_tools/tools/1".into(),
                kind: ValidationErrorKind::DuplicateToolName {
                    name: "read_file".into(),
                    other_origin: ToolOrigin::Own,
                    other_pointer: "/tools/0/name".into()
                },
            },
            ValidationError {
                path: "/builtin_tools/root".into(),
                kind: ValidationErrorKind::MissingBuiltinRoot,
            },
        ]
    );
}

/// Regex search (addendum ADR-0046): a file tool, so it needs a root.
#[test]
fn knows_search_files_as_a_file_tool() {
    let with_root = |root: Option<&str>| {
        let mut builtin = json!({ "tools": ["search_files"] });
        if let Some(root) = root {
            builtin["root"] = json!(root);
        }
        AgentSpec::from_json(
            &json!({
                "spec_version": 1, "name": "finder",
                "model": { "provider": "ollama", "id": "m" },
                "builtin_tools": builtin,
                "strategy": { "kind": "single_run" }
            })
            .to_string(),
        )
        .unwrap()
    };

    assert_eq!(with_root(Some("workspace")).validate(), Ok(()));
    assert!(
        with_root(None)
            .validate()
            .unwrap_err()
            .iter()
            .any(|error| error.kind == ValidationErrorKind::MissingBuiltinRoot),
    );
}

/// Jira, Confluence and git tools (ADR-0104).
fn atlassian_git_spec(builtin_tools: Value) -> Result<AgentSpec, grauwolf_spec::SpecError> {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "triage",
            "model": { "provider": "ollama", "id": "m" },
            "builtin_tools": builtin_tools,
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
}

fn error_kinds(spec: &AgentSpec) -> Vec<(String, ValidationErrorKind)> {
    spec.validate()
        .unwrap_err()
        .into_iter()
        .map(|error| (error.path, error.kind))
        .collect()
}

#[test]
fn accepts_jira_confluence_and_git_sections_with_their_tools() {
    let spec = atlassian_git_spec(json!({
        "tools": ["jira_search_issues", "confluence_get_page", "git_checkout", "git_push"],
        "jira": { "base_url": "https://example.atlassian.net" },
        "confluence": { "base_url": "https://example.atlassian.net/wiki" },
        "git": { "repo_url": "https://github.com/org/repo.git", "directory": "repo",
                 "push_policy": "any", "use_token": true },
        "root": "work"
    }))
    .unwrap();

    assert_eq!(spec.validate(), Ok(()));
    let builtin = spec.builtin_tools.as_ref().unwrap();
    assert_eq!(
        builtin.jira.as_ref().unwrap().base_url,
        "https://example.atlassian.net"
    );
    let git = builtin.git.as_ref().unwrap();
    assert_eq!(git.push_policy, PushPolicy::Any);
    assert!(git.use_token);
}

#[test]
fn git_section_defaults_to_new_branches_without_token_in_the_directory_repo() {
    let spec = atlassian_git_spec(json!({
        "tools": ["git_checkout"],
        "git": { "repo_url": "https://github.com/org/repo.git" }
    }))
    .unwrap();

    let git = spec.builtin_tools.as_ref().unwrap().git.clone().unwrap();
    assert_eq!(git.push_policy, PushPolicy::NewBranches);
    assert!(!git.use_token);
    assert_eq!(git.directory_or_default(), "repo");
    assert_eq!(spec.validate(), Ok(()));
}

#[test]
fn a_tool_of_a_group_needs_the_section_of_that_group() {
    let spec = atlassian_git_spec(json!({
        "tools": ["jira_get_issue", "confluence_get_page", "git_list_branches"]
    }))
    .unwrap();

    assert_eq!(
        error_kinds(&spec),
        [
            (
                "/builtin_tools/jira".to_owned(),
                ValidationErrorKind::MissingBuiltinSection {
                    group: "jira".into()
                }
            ),
            (
                "/builtin_tools/confluence".to_owned(),
                ValidationErrorKind::MissingBuiltinSection {
                    group: "confluence".into()
                }
            ),
            (
                "/builtin_tools/git".to_owned(),
                ValidationErrorKind::MissingBuiltinSection {
                    group: "git".into()
                }
            ),
        ]
    );
}

#[test]
fn urls_must_be_https_without_user_info() {
    let spec = atlassian_git_spec(json!({
        "tools": ["jira_get_issue", "git_checkout"],
        "jira": { "base_url": "http://example.atlassian.net" },
        "git": { "repo_url": "https://user:secret@github.com/org/repo.git" }
    }))
    .unwrap();

    let errors = error_kinds(&spec);
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert_eq!(errors[0].0, "/builtin_tools/jira/base_url");
    assert_eq!(errors[1].0, "/builtin_tools/git/repo_url");
    assert!(
        matches!(&errors[1].1, ValidationErrorKind::InvalidUrl { reason } if reason.contains("user info")),
        "{errors:?}"
    );
    // The secret must not end up in the message.
    assert!(!format!("{errors:?}").contains("secret"));
}

#[test]
fn the_git_directory_must_stay_inside_the_root() {
    for directory in ["/tmp/repo", "../repo", "a/../../b"] {
        let spec = atlassian_git_spec(json!({
            "tools": ["git_checkout"],
            "git": { "repo_url": "https://github.com/org/repo.git", "directory": directory }
        }))
        .unwrap();

        let errors = error_kinds(&spec);
        assert_eq!(errors.len(), 1, "{directory}: {errors:?}");
        assert_eq!(errors[0].0, "/builtin_tools/git/directory");
    }
}

#[test]
fn rejects_an_unknown_push_policy() {
    assert!(
        atlassian_git_spec(json!({
            "tools": ["git_checkout"],
            "git": { "repo_url": "https://github.com/org/repo.git", "push_policy": "force" }
        }))
        .is_err()
    );
}

#[test]
fn group_tool_names_must_not_collide_with_custom_tools() {
    let mut spec = spec(json!({
        "tools": ["git_checkout"],
        "git": { "repo_url": "https://github.com/org/repo.git" }
    }));
    spec.tools[0].name = "git_checkout".into();

    assert!(error_kinds(&spec).contains(&(
        "/builtin_tools/tools/0".to_owned(),
        ValidationErrorKind::DuplicateToolName {
            name: "git_checkout".into(),
            other_origin: ToolOrigin::Own,
            other_pointer: "/tools/0/name".into()
        }
    )));
}

#[test]
fn the_fetch_tool_needs_its_section_with_valid_hosts() {
    let missing = atlassian_git_spec(json!({ "tools": ["fetch"] })).unwrap();
    let good = atlassian_git_spec(json!({
        "tools": ["fetch", "think"],
        "fetch": { "allowed_hosts": ["example.com", "*.example.org", "127.0.0.1"], "allow_http": true }
    }))
    .unwrap();
    let bad = atlassian_git_spec(json!({
        "tools": ["fetch"],
        "fetch": { "allowed_hosts": ["https://example.com/page", "*.", "ok.example.com"] }
    }))
    .unwrap();
    let empty = atlassian_git_spec(json!({ "tools": ["fetch"], "fetch": { "allowed_hosts": [] } }))
        .unwrap();

    assert_eq!(
        error_kinds(&missing),
        [(
            "/builtin_tools/fetch".to_owned(),
            ValidationErrorKind::MissingBuiltinSection {
                group: "fetch".into()
            }
        )]
    );
    assert_eq!(good.validate(), Ok(()));
    assert_eq!(
        error_kinds(&bad),
        [
            (
                "/builtin_tools/fetch/allowed_hosts/0".to_owned(),
                ValidationErrorKind::InvalidAllowedHost {
                    host: "https://example.com/page".into()
                }
            ),
            (
                "/builtin_tools/fetch/allowed_hosts/1".to_owned(),
                ValidationErrorKind::InvalidAllowedHost { host: "*.".into() }
            ),
        ]
    );
    assert_eq!(
        error_kinds(&empty),
        [(
            "/builtin_tools/fetch/allowed_hosts".to_owned(),
            ValidationErrorKind::EmptyAllowedHosts
        )]
    );
}

#[test]
fn key_value_tools_need_a_root_and_todo_tools_none() {
    let without_root =
        atlassian_git_spec(json!({ "tools": ["store_value", "list_keys"] })).unwrap();
    let with_root = atlassian_git_spec(json!({
        "tools": ["store_value", "retrieve_value", "list_keys", "add_task", "complete_task", "remove_task", "list_tasks"],
        "root": "data"
    }))
    .unwrap();
    let todo_only = atlassian_git_spec(json!({ "tools": ["add_task", "list_tasks"] })).unwrap();

    assert_eq!(
        error_kinds(&without_root),
        [(
            "/builtin_tools/root".to_owned(),
            ValidationErrorKind::MissingBuiltinRoot
        )]
    );
    assert_eq!(with_root.validate(), Ok(()));
    assert_eq!(todo_only.validate(), Ok(()));
}

fn command_spec(command: Value) -> AgentSpec {
    atlassian_git_spec(json!({ "tools": ["run_command"], "root": "work", "command": command }))
        .unwrap()
}

fn command_errors(command: Value) -> Vec<(String, ValidationErrorKind)> {
    error_kinds(&command_spec(command))
}

#[test]
fn accepts_the_command_tool_in_allowlist_mode() {
    let spec = command_spec(json!({
        "allowed": ["git", "/usr/bin/python3"],
        "timeout_secs": 30, "max_output_bytes": 4096, "env": ["PATH", "LANG"]
    }));

    assert_eq!(spec.validate(), Ok(()));
    assert_eq!(spec.warnings(), Vec::new());
    let command = spec.builtin_tools.unwrap().command.unwrap();
    assert_eq!(command.allowed, ["git", "/usr/bin/python3"]);
    assert!(!command.shell);
    assert!(command.approve_each_call);
    assert_eq!(command.timeout_secs, Some(30));
}

#[test]
fn the_shell_with_approval_per_call_makes_run_command_need_approval() {
    let shell = command_spec(json!({ "allowed": [], "shell": true }));
    let open_shell =
        command_spec(json!({ "allowed": [], "shell": true, "approve_each_call": false }));
    let allowlist = command_spec(json!({ "allowed": ["ls"] }));

    assert_eq!(shell.approval_tools(), ["run_command"]);
    assert!(open_shell.approval_tools().is_empty());
    assert!(allowlist.approval_tools().is_empty());
}

#[test]
fn the_command_tool_needs_its_section_and_a_root() {
    let no_section =
        atlassian_git_spec(json!({ "tools": ["run_command"], "root": "work" })).unwrap();
    let no_root = atlassian_git_spec(json!({
        "tools": ["run_command"], "command": { "allowed": ["ls"] }
    }))
    .unwrap();

    assert_eq!(
        error_kinds(&no_section),
        [(
            "/builtin_tools/command".to_owned(),
            ValidationErrorKind::MissingBuiltinSection {
                group: "command".into()
            }
        )]
    );
    assert_eq!(
        error_kinds(&no_root),
        [(
            "/builtin_tools/root".to_owned(),
            ValidationErrorKind::MissingBuiltinRoot
        )]
    );
}

#[test]
fn allowlist_mode_needs_programs_and_refuses_the_wildcard() {
    assert_eq!(
        command_errors(json!({ "allowed": [] })),
        [(
            "/builtin_tools/command/allowed".to_owned(),
            ValidationErrorKind::EmptyAllowedCommands
        )]
    );
    assert_eq!(
        command_errors(json!({ "allowed": ["ls", "*"] })),
        [(
            "/builtin_tools/command/allowed/1".to_owned(),
            ValidationErrorKind::WildcardWithoutShell
        )]
    );
}

#[test]
fn programs_are_bare_names_or_absolute_paths() {
    assert_eq!(
        command_errors(json!({ "allowed": ["bin/tool", "ls -l", "", "./x"] })),
        ["bin/tool", "ls -l", "", "./x"]
            .iter()
            .enumerate()
            .map(|(index, program)| (
                format!("/builtin_tools/command/allowed/{index}"),
                ValidationErrorKind::InvalidAllowedProgram {
                    program: (*program).into()
                }
            ))
            .collect::<Vec<_>>()
    );
}

#[test]
fn shell_mode_needs_the_wildcard_and_the_acknowledgement() {
    let shell =
        command_spec(json!({ "allowed": ["*"], "shell": true, "i_understand_shell": true }));
    assert_eq!(shell.validate(), Ok(()));

    assert_eq!(
        command_errors(json!({ "allowed": ["ls"], "shell": true, "i_understand_shell": true })),
        [(
            "/builtin_tools/command/allowed".to_owned(),
            ValidationErrorKind::ShellNeedsWildcard
        )]
    );
    assert_eq!(
        command_errors(json!({ "allowed": ["*"], "shell": true })),
        [(
            "/builtin_tools/command/i_understand_shell".to_owned(),
            ValidationErrorKind::ShellNotAcknowledged
        )]
    );
}

#[test]
fn shell_mode_gets_a_warning() {
    let spec = command_spec(json!({ "allowed": ["*"], "shell": true, "i_understand_shell": true }));

    let warnings = spec.warnings();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].pointer, "/builtin_tools/command/shell");
    assert!(warnings[0].message.contains("shell"));
}

#[test]
fn command_limits_must_be_in_range() {
    let field = |name: &str| {
        (
            format!("/builtin_tools/command/{name}"),
            ValidationErrorKind::CommandLimitOutOfRange { field: name.into() },
        )
    };
    assert_eq!(
        command_errors(json!({ "allowed": ["ls"], "timeout_secs": 0, "max_output_bytes": 0 })),
        [field("timeout_secs"), field("max_output_bytes")]
    );
    assert_eq!(
        command_errors(
            json!({ "allowed": ["ls"], "timeout_secs": 3601, "max_output_bytes": 10_485_761 })
        ),
        [field("timeout_secs"), field("max_output_bytes")]
    );
}
