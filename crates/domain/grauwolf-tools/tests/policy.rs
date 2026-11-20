//! Policy for tool packages (ADR-0107 stage 4): parsing and allowlists.

use grauwolf_tools::{PolicyError, ToolPolicy};

fn parse(body: &str) -> Result<ToolPolicy, PolicyError> {
    ToolPolicy::parse(&format!("[tool_packages]\n{body}"))
}

#[test]
fn an_empty_file_allows_everything_and_requires_nothing() {
    let policy = ToolPolicy::parse("").unwrap();
    assert!(policy.check_oci("ghcr.io", "anyone/tool").is_ok());
    assert!(policy.check_git("https://example.org/a/b.git").is_ok());
    assert!(!policy.require_signatures);
}

#[test]
fn parses_every_setting() {
    let policy = parse(
        r#"
allowed_registries = ["ghcr.io/acme", "localhost:5000"]
allowed_git_hosts = ["github.com/acme"]
require_signatures = true
oci_keys = ["keys/acme.pub"]
git_allowed_signers = "keys/allowed_signers"
"#,
    )
    .unwrap();
    assert_eq!(
        policy.allowed_registries,
        Some(vec!["ghcr.io/acme".into(), "localhost:5000".into()])
    );
    assert_eq!(
        policy.allowed_git_hosts,
        Some(vec!["github.com/acme".into()])
    );
    assert!(policy.require_signatures);
    assert_eq!(policy.oci_keys, vec!["keys/acme.pub"]);
    assert_eq!(
        policy.git_allowed_signers.as_deref(),
        Some("keys/allowed_signers")
    );
}

#[test]
fn registries_match_a_registry_or_a_prefix_at_segment_boundaries() {
    let policy = parse(r#"allowed_registries = ["ghcr.io/acme", "localhost:5000"]"#).unwrap();
    assert!(policy.check_oci("ghcr.io", "acme/tools/add").is_ok());
    assert!(policy.check_oci("ghcr.io", "acme").is_ok());
    assert!(policy.check_oci("localhost:5000", "any/repo").is_ok());
    let error = policy
        .check_oci("ghcr.io", "acmex/tools")
        .unwrap_err()
        .to_string();
    assert!(error.contains("ghcr.io/acmex/tools"), "{error}");
    assert!(error.contains("allowed_registries"), "{error}");
    assert!(policy.check_oci("localhost:50001", "a").is_err());
    assert!(policy.check_oci("docker.io", "acme/x").is_err());
}

#[test]
fn a_missing_list_allows_any_and_an_empty_list_allows_none() {
    let any = parse("require_signatures = false").unwrap();
    assert!(any.check_oci("ghcr.io", "a/b").is_ok());
    let none = parse("allowed_registries = []\nallowed_git_hosts = []").unwrap();
    assert!(none.check_oci("ghcr.io", "a/b").is_err());
    assert!(none.check_git("https://github.com/a/b").is_err());
}

#[test]
fn git_hosts_match_host_and_path_prefixes_ignoring_scheme_and_dot_git() {
    let policy = parse(r#"allowed_git_hosts = ["github.com/acme", "git.internal"]"#).unwrap();
    assert!(
        policy
            .check_git("https://github.com/acme/tools.git")
            .is_ok()
    );
    assert!(policy.check_git("https://GitHub.com/acme/tools").is_ok());
    assert!(policy.check_git("https://git.internal/x/y.git").is_ok());
    assert!(
        policy
            .check_git("https://github.com/acmex/tools.git")
            .is_err()
    );
    assert!(
        policy
            .check_git("https://github.com/other/tools.git")
            .is_err()
    );
    assert!(
        policy
            .check_git("https://git.internal.evil.org/x.git")
            .is_err()
    );
}

#[test]
fn unknown_keys_are_an_error() {
    assert!(matches!(
        parse("allow_registries = []"),
        Err(PolicyError::Invalid(_))
    ));
    assert!(matches!(
        ToolPolicy::parse("[other]\na = 1"),
        Err(PolicyError::Invalid(_))
    ));
}

#[test]
fn require_signatures_without_a_key_is_an_error() {
    assert_eq!(parse("require_signatures = true"), Err(PolicyError::NoKeys));
    assert!(parse("require_signatures = true\noci_keys = [\"a.pub\"]").is_ok());
    assert!(parse("require_signatures = true\ngit_allowed_signers = \"s\"").is_ok());
}

#[test]
fn a_credentials_table_next_to_the_policy_is_not_an_error() {
    let policy = ToolPolicy::parse(
        "[tool_packages]\nrequire_signatures = false\n[credentials.variables]\nGIT_TOKEN = \"file:x\"\n",
    )
    .unwrap();
    assert!(!policy.require_signatures);
}
