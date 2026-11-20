//! SPIFFE for `fetch`, the git tools, stdio MCP servers and `run_command` (ADR-0110, stage 1).

use grauwolf_spec::{AgentSpec, AgentSpecExt, ValidationErrorKind};
use serde_json::{Value, json};

const ID: &str = "spiffe://agentswork.de/ns/git/sa/git";

fn spec(builtin_tools: Value, mcp_servers: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "builtin_tools": builtin_tools,
            "mcp_servers": mcp_servers,
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap()
}

fn errors(spec: &AgentSpec) -> Vec<(String, ValidationErrorKind)> {
    spec.validate()
        .err()
        .unwrap_or_default()
        .into_iter()
        .map(|e| (e.path, e.kind))
        .collect()
}

fn fetch(auth: Value) -> Vec<(String, ValidationErrorKind)> {
    errors(&spec(
        json!({ "tools": ["fetch"], "fetch": {
            "allowed_hosts": ["git.svc", "*.internal", "example.com"], "auth": auth } }),
        json!({}),
    ))
}

#[test]
fn fetch_auth_accepts_allowed_hosts_and_patterns_in_both_modes() {
    assert_eq!(
        fetch(json!({
            "git.svc": { "spiffe": { "id": ID } },
            "*.internal": { "spiffe": { "mode": "jwt", "audience": "internal" } }
        })),
        []
    );
}

#[test]
fn fetch_auth_for_a_host_that_is_not_allowed_is_an_error() {
    assert_eq!(
        fetch(json!({ "other.svc": { "spiffe": { "id": ID } } })),
        [(
            "/builtin_tools/fetch/auth/other.svc".to_owned(),
            ValidationErrorKind::AuthHostNotAllowed {
                host: "other.svc".into()
            }
        )]
    );
}

#[test]
fn fetch_auth_follows_the_spiffe_rules_of_adr_0109() {
    assert_eq!(
        fetch(json!({ "git.svc": { "spiffe": { "mode": "jwt" } } })),
        [(
            "/builtin_tools/fetch/auth/git.svc/spiffe".to_owned(),
            ValidationErrorKind::SpiffeAudience
        )]
    );
    assert_eq!(
        fetch(json!({ "git.svc": { "spiffe": {} } })),
        [(
            "/builtin_tools/fetch/auth/git.svc/spiffe".to_owned(),
            ValidationErrorKind::SpiffeMatch
        )]
    );
}

fn git(extra: Value) -> Vec<(String, ValidationErrorKind)> {
    let mut section = json!({ "repo_url": "https://git.svc:8443/team/repo.git" });
    section
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    errors(&spec(
        json!({ "tools": ["git_checkout"], "root": "w", "git": section }),
        json!({}),
    ))
}

#[test]
fn git_auth_names_the_host_of_the_repository() {
    assert_eq!(
        git(json!({ "auth": { "git.svc": { "spiffe": { "id": ID } } } })),
        []
    );
    assert_eq!(
        git(json!({ "auth": { "*.svc": { "spiffe": { "id": ID } } } })),
        []
    );
    assert_eq!(
        git(json!({ "auth": { "other.svc": { "spiffe": { "id": ID } } } })),
        [(
            "/builtin_tools/git/auth/other.svc".to_owned(),
            ValidationErrorKind::AuthHostNotRepository {
                host: "other.svc".into()
            }
        )]
    );
}

#[test]
fn git_auth_excludes_use_token() {
    assert_eq!(
        git(json!({ "use_token": true, "auth": { "git.svc": { "spiffe": { "id": ID } } } })),
        [(
            "/builtin_tools/git/auth".to_owned(),
            ValidationErrorKind::GitTokenWithAuth
        )]
    );
}

#[test]
fn workload_api_is_for_stdio_mcp_servers_only() {
    let ok = spec(
        json!({ "tools": ["think"] }),
        json!({ "local": { "command": "tool", "tools": ["a"], "workload_api": true } }),
    );
    assert_eq!(errors(&ok), []);
    let remote = spec(
        json!({ "tools": ["think"] }),
        json!({ "remote": { "url": "https://m.svc/mcp", "tools": ["a"], "workload_api": true } }),
    );
    assert_eq!(
        errors(&remote),
        [(
            "/mcp_servers/remote/workload_api".to_owned(),
            ValidationErrorKind::WorkloadApiWithoutStdio
        )]
    );
}

fn command(section: Value) -> Vec<(String, ValidationErrorKind)> {
    errors(&spec(
        json!({ "tools": ["run_command"], "root": "w", "command": section }),
        json!({}),
    ))
}

#[test]
fn run_command_workload_api_is_refused_in_shell_mode() {
    assert_eq!(
        command(json!({ "allowed": ["git-spiffe"], "workload_api": true })),
        []
    );
    assert_eq!(
        command(
            json!({ "allowed": ["*"], "shell": true, "i_understand_shell": true,
                        "workload_api": true })
        ),
        [(
            "/builtin_tools/command/workload_api".to_owned(),
            ValidationErrorKind::WorkloadApiWithShell
        )]
    );
}

#[test]
fn host_matching_follows_allowed_hosts() {
    assert!(grauwolf_spec::host_matches("*.svc", "git.svc"));
    assert!(!grauwolf_spec::host_matches("*.svc", "svc"));
    assert!(grauwolf_spec::host_matches("Git.SVC", "git.svc"));
    assert_eq!(
        grauwolf_spec::origin_host("https://u@Git.svc:8443/a").as_deref(),
        Some("git.svc")
    );
}
