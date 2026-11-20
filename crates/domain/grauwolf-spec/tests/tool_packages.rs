//! `tool_packages` in specs (ADR-0107).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, OciReference, ToolOrigin, ValidationErrorKind};
use serde_json::{Value, json};

fn spec(packages: Value) -> AgentSpec {
    let mut spec = json!({
        "spec_version": 1, "name": "agent",
        "model": { "provider": "ollama", "id": "m" },
        "strategy": { "kind": "single_run" },
        "tool_packages": packages
    });
    spec["tools"] = json!([{ "name": "local", "description": "d",
        "args": { "type": "object" }, "code": "Ok(String::new())" }]);
    AgentSpec::from_json(&spec.to_string()).unwrap()
}

fn errors(packages: Value) -> Vec<(String, ValidationErrorKind)> {
    match spec(packages).validate() {
        Ok(()) => Vec::new(),
        Err(errors) => errors.into_iter().map(|e| (e.path, e.kind)).collect(),
    }
}

#[test]
fn accepts_oci_packages_with_selection_and_renaming() {
    let packages = json!([
        { "oci": "ghcr.io/acme/tools/math:1.2" },
        { "oci": "localhost:5000/test/add@sha256:0000000000000000000000000000000000000000000000000000000000000000",
          "tools": ["add", "sub"], "as": { "add": "plus" } }
    ]);
    assert_eq!(errors(packages), vec![]);
}

const COMMIT: &str = "4e1a9c2";

#[test]
fn accepts_git_sources_with_one_of_rev_tag_branch() {
    let found = errors(json!([
        { "git": "https://github.com/acme/t.git", "path": "text", "rev": COMMIT, "tools": ["a"] },
        { "git": "https://github.com/acme/t.git", "tag": "v1.0" },
        { "git": "http://127.0.0.1:8080/t.git", "branch": "main" },
        { "git": "http://localhost/t.git", "rev": "4e1a9c2f4e1a9c2f4e1a9c2f4e1a9c2f4e1a9c2f" }
    ]));
    assert_eq!(found, vec![]);
}

#[test]
fn rejects_unusable_git_sources() {
    let cases = [
        (json!({ "git": "https://x/t.git" }), "git"),
        (
            json!({ "git": "https://x/t.git", "rev": COMMIT, "tag": "v1" }),
            "git",
        ),
        (
            json!({ "git": "https://x/t.git", "tag": "v1", "branch": "m" }),
            "git",
        ),
        (
            json!({ "git": "http://example.com/t.git", "rev": COMMIT }),
            "git",
        ),
        (json!({ "git": "ssh://x/t.git", "rev": COMMIT }), "git"),
        (
            json!({ "git": "https://user:pw@example.com/t.git", "rev": COMMIT }),
            "git",
        ),
        (json!({ "git": "https:///t.git", "rev": COMMIT }), "git"),
        (json!({ "git": "/srv/repo.git", "rev": COMMIT }), "git"),
        (
            json!({ "git": "file:///srv/repo.git", "rev": COMMIT }),
            "git",
        ),
        (json!({ "git": "https://x/t.git", "rev": "abc12" }), "rev"),
        (json!({ "git": "https://x/t.git", "rev": "XYZ1234" }), "rev"),
        (json!({ "git": "https://x/t.git", "tag": "a b" }), "tag"),
        (
            json!({ "git": "https://x/t.git", "branch": "-x" }),
            "branch",
        ),
        (
            json!({ "git": "https://x/t.git", "rev": COMMIT, "path": "../up" }),
            "path",
        ),
        (
            json!({ "git": "https://x/t.git", "rev": COMMIT, "path": "/abs" }),
            "path",
        ),
        (
            json!({ "git": "https://x/t.git", "rev": COMMIT, "path": "a//b" }),
            "path",
        ),
    ];
    for (entry, field) in cases {
        let found = errors(json!([entry.clone()]));
        let expected = format!("/tool_packages/0/{field}");
        assert!(
            matches!(&found[..], [(path, ValidationErrorKind::InvalidGitSource { .. })] if *path == expected),
            "{entry}: {found:?}"
        );
    }
}

#[test]
fn git_errors_never_repeat_the_url() {
    let found = errors(json!([{ "git": "https://user:secret@example.com/t.git", "rev": COMMIT }]));
    assert!(!format!("{found:?}").contains("secret"));
}

#[test]
fn git_fields_next_to_an_oci_source_are_rejected() {
    let found = errors(json!([{ "oci": "ghcr.io/a/b:1", "rev": COMMIT }]));
    assert!(
        matches!(&found[..], [(path, ValidationErrorKind::InvalidGitSource { .. })] if path == "/tool_packages/0/rev")
    );
}

#[test]
fn the_lock_key_names_url_path_and_reference() {
    use grauwolf_spec::GitSource;
    let key = |entry: Value| {
        let spec = spec(json!([entry]));
        GitSource::from_entry(&spec.tool_packages[0])
            .unwrap()
            .lock_key()
    };
    assert_eq!(
        key(json!({ "git": "https://h/r.git", "path": "text", "rev": COMMIT })),
        "git+https://h/r.git//text@4e1a9c2"
    );
    assert_eq!(
        key(json!({ "git": "https://h/r.git", "tag": "v1" })),
        "git+https://h/r.git@tag:v1"
    );
    assert_eq!(
        key(json!({ "git": "https://h/r.git", "path": "a/b", "branch": "main" })),
        "git+https://h/r.git//a/b@branch:main"
    );
}

#[test]
fn needs_exactly_one_source() {
    let found = errors(json!([{}, { "oci": "ghcr.io/a/b:1", "git": "https://x/y.git" }]));
    assert_eq!(
        found,
        vec![
            (
                "/tool_packages/0".to_owned(),
                ValidationErrorKind::ToolPackageSource
            ),
            (
                "/tool_packages/1".to_owned(),
                ValidationErrorKind::ToolPackageSource
            ),
        ]
    );
}

#[test]
fn rejects_invalid_references() {
    for reference in [
        "math:1.2",
        "ghcr.io/Acme/Tools:1",
        "ghcr.io/acme/tools:",
        "ghcr.io/acme/tools@sha256:abc",
        "ghcr.io/acme//tools",
        "https://ghcr.io/acme/tools:1",
        "ghcr.io",
        "user:pw@ghcr.io/acme/tools:1",
    ] {
        let found = errors(json!([{ "oci": reference }]));
        assert!(
            matches!(&found[..], [(path, ValidationErrorKind::InvalidOciReference { .. })] if path == "/tool_packages/0/oci"),
            "{reference}: {found:?}"
        );
    }
}

#[test]
fn parses_reference_parts() {
    let reference = OciReference::parse("localhost:5000/test/add:1.0").unwrap();
    assert_eq!(reference.registry, "localhost:5000");
    assert_eq!(reference.repository, "test/add");
    assert_eq!(reference.tag.as_deref(), Some("1.0"));
    assert_eq!(reference.digest, None);
    let digest = format!("sha256:{}", "a".repeat(64));
    let reference = OciReference::parse(&format!("ghcr.io/acme/t@{digest}")).unwrap();
    assert_eq!(reference.digest.as_deref(), Some(digest.as_str()));
    assert_eq!(reference.reference(), digest);
    assert_eq!(
        OciReference::parse("ghcr.io/acme/t").unwrap().reference(),
        "latest"
    );
}

#[test]
fn reports_duplicate_names_after_renaming_and_against_inline_tools() {
    let found = errors(json!([
        { "oci": "ghcr.io/a/b:1", "tools": ["add", "mul"], "as": { "add": "local" } },
        { "oci": "ghcr.io/a/c:1", "tools": ["mul"] }
    ]));
    assert_eq!(
        found,
        vec![
            (
                "/tool_packages/0/as/add".to_owned(),
                ValidationErrorKind::DuplicateToolName {
                    name: "local".into(),
                    other_origin: ToolOrigin::Own,
                    other_pointer: "/tools/0/name".into()
                }
            ),
            (
                "/tool_packages/1/tools/0".to_owned(),
                ValidationErrorKind::DuplicateToolName {
                    name: "mul".into(),
                    other_origin: ToolOrigin::Package,
                    other_pointer: "/tool_packages/0/tools/1".into()
                }
            ),
        ]
    );
}

#[test]
fn reports_renames_of_unselected_tools_and_bad_names() {
    let found = errors(json!([
        { "oci": "ghcr.io/a/b:1", "tools": ["add"], "as": { "sub": "minus" } },
        { "oci": "ghcr.io/a/c:1", "as": { "x": "Bad-Name" } }
    ]));
    assert_eq!(
        found,
        vec![
            (
                "/tool_packages/0/as/sub".to_owned(),
                ValidationErrorKind::UnknownTool { name: "sub".into() }
            ),
            (
                "/tool_packages/1/as/x".to_owned(),
                ValidationErrorKind::InvalidId {
                    id: "Bad-Name".into()
                }
            ),
        ]
    );
}

#[test]
fn selected_package_tools_count_as_known_tools() {
    let mut value = json!({
        "spec_version": 1, "name": "agent",
        "model": { "provider": "ollama", "id": "m" },
        "tool_packages": [{ "oci": "ghcr.io/a/b:1", "tools": ["add"], "as": { "add": "plus" } }],
        "subgraphs": { "s": { "input": "string", "output": "string", "tools": ["plus"], "graph": { "nodes": [ { "id": "call_llm", "kind": "llm_request" } ], "edges": [ { "from": "start", "to": "call_llm" }, { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } } ] } } },
        "strategy": { "kind": "single_run" }
    });
    let ok = AgentSpec::from_json(&value.to_string()).unwrap();
    assert!(ok.validate().is_ok(), "{:?}", ok.validate());
    value["subgraphs"]["s"]["tools"] = json!(["add"]);
    let bad = AgentSpec::from_json(&value.to_string()).unwrap();
    assert!(bad.validate().is_err());
}
