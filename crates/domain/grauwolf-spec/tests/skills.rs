//! `skills` in specs (ADR-0103).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ValidationErrorKind};
use serde_json::{Value, json};

fn spec(skills: Value) -> AgentSpec {
    let spec = json!({
        "spec_version": 1, "name": "agent",
        "model": { "provider": "ollama", "id": "m" },
        "strategy": { "kind": "single_run" },
        "skills": skills
    });
    AgentSpec::from_json(&spec.to_string()).unwrap()
}

fn errors(skills: Value) -> Vec<(String, ValidationErrorKind)> {
    match spec(skills).validate() {
        Ok(()) => Vec::new(),
        Err(errors) => errors.into_iter().map(|e| (e.path, e.kind)).collect(),
    }
}

#[test]
fn accepts_local_and_git_sources() {
    let found = errors(json!([
        { "dir": "skills" },
        { "dir": "more/skills", "names": ["haiku-writer"] },
        { "git": "https://github.com/acme/skills.git", "path": "writing", "tag": "v1", "names": ["a"] },
        { "git": "http://localhost:8080/s.git", "rev": "4e1a9c2" }
    ]));
    assert_eq!(found, vec![]);
}

#[test]
fn rejects_sources_without_exactly_one_of_dir_and_git() {
    let found = errors(json!([{}, { "dir": "d", "git": "https://x/s.git", "rev": "4e1a9c2" }]));
    assert_eq!(
        found,
        vec![
            ("/skills/0".to_owned(), ValidationErrorKind::SkillSource),
            ("/skills/1".to_owned(), ValidationErrorKind::SkillSource),
        ]
    );
}

#[test]
fn rejects_unusable_directories_and_git_sources() {
    let cases = [
        (json!({ "dir": "/abs" }), "/skills/0/dir"),
        (json!({ "dir": "../up" }), "/skills/0/dir"),
        (json!({ "dir": "" }), "/skills/0/dir"),
        (
            json!({ "git": "http://example.com/s.git", "rev": "4e1a9c2" }),
            "/skills/0/git",
        ),
        (
            json!({ "git": "https://u:p@example.com/s.git", "rev": "4e1a9c2" }),
            "/skills/0/git",
        ),
        (json!({ "git": "https://x/s.git" }), "/skills/0/git"),
        (
            json!({ "git": "https://x/s.git", "rev": "4e1a9c2", "tag": "v" }),
            "/skills/0/git",
        ),
        (
            json!({ "git": "https://x/s.git", "rev": "4e1a9c2", "path": "../x" }),
            "/skills/0/path",
        ),
        (json!({ "dir": "d", "tag": "v1" }), "/skills/0/tag"),
        (json!({ "dir": "d", "path": "p" }), "/skills/0/path"),
    ];
    for (source, pointer) in cases {
        let found = errors(json!([source.clone()]));
        assert_eq!(found.len(), 1, "{source}: {found:?}");
        assert_eq!(found[0].0, pointer, "{source}");
    }
}

#[test]
fn rejects_invalid_skill_names_in_the_selection() {
    let found = errors(json!([{ "dir": "d", "names": ["ok-name", "Bad Name", ""] }]));
    let paths: Vec<_> = found.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(paths, ["/skills/0/names/1", "/skills/0/names/2"]);
}

#[test]
fn reserves_the_skill_tool_names() {
    let mut value = json!({
        "spec_version": 1, "name": "agent",
        "model": { "provider": "ollama", "id": "m" },
        "strategy": { "kind": "single_run" },
        "skills": [{ "dir": "skills" }],
        "tools": [{ "name": "load_skill", "description": "d",
            "args": { "type": "object" }, "code": "Ok(String::new())" }]
    });
    let found = AgentSpec::from_json(&value.to_string()).unwrap().validate();
    assert!(found.is_err());
    value.as_object_mut().unwrap().remove("skills");
    assert!(
        AgentSpec::from_json(&value.to_string())
            .unwrap()
            .validate()
            .is_ok()
    );
}

#[test]
fn offers_the_skill_tools_as_external_tools_only_with_skills() {
    assert_eq!(
        spec(json!([{ "dir": "s" }])).external_tools(),
        ["load_skill", "read_skill_file"]
    );
    assert!(spec(json!([])).external_tools().is_empty());
}
