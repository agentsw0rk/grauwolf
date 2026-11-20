//! The command-line form of a git source: the lock key (ADR-0107).

use grauwolf_spec::{GitRef, GitSource};

const COMMIT: &str = "4e1a9c2f";

fn parse(text: &str) -> Result<GitSource, String> {
    GitSource::parse_reference(text)
}

#[test]
fn parses_the_three_reference_kinds() {
    let source = parse(&format!("git+https://h.example/r.git@{COMMIT}")).unwrap();
    assert_eq!(source.url, "https://h.example/r.git");
    assert_eq!(source.path, None);
    assert_eq!(source.reference, GitRef::Rev(COMMIT.into()));
    assert_eq!(
        parse("git+https://h.example/r.git@tag:v1")
            .unwrap()
            .reference,
        GitRef::Tag("v1".into())
    );
    assert_eq!(
        parse("git+https://h.example/r.git@branch:feature/x")
            .unwrap()
            .reference,
        GitRef::Branch("feature/x".into())
    );
}

#[test]
fn parses_a_folder_and_local_http() {
    let source = parse("git+http://127.0.0.1:8080/tools.git//a/b@tag:v1").unwrap();
    assert_eq!(source.url, "http://127.0.0.1:8080/tools.git");
    assert_eq!(source.path.as_deref(), Some("a/b"));
}

#[test]
fn round_trips_through_the_lock_key() {
    for text in [
        "git+https://h.example/r.git@4e1a9c2f",
        "git+https://h.example/r.git//text@tag:v1",
        "git+https://h.example/r.git//a/b@branch:main",
        "git+http://localhost:9/r.git@branch:main",
    ] {
        assert_eq!(parse(text).unwrap().lock_key(), text);
    }
}

#[test]
fn a_missing_reference_names_an_example() {
    let error = parse("git+https://h.example/r.git").unwrap_err();
    assert!(error.contains("@tag:v1"), "{error}");
}

#[test]
fn rejects_invalid_sources_with_the_rules_of_the_spec() {
    for text in [
        "git+ftp://h.example/r.git@tag:v1",
        "git+http://h.example/r.git@tag:v1",
        "git+https://user:pw@h.example/r.git@tag:v1",
        "git+https://h.example/r.git//../x@tag:v1",
        "git+https://h.example/r.git//@tag:v1",
        "git+https://h.example/r.git@abc",
        "git+https://h.example/r.git@ABCDEF1",
        "git+https://h.example/r.git@tag:",
        "git+https://h.example/r.git@branch:a..b",
        "git+@tag:v1",
    ] {
        assert!(parse(text).is_err(), "{text}");
    }
}

#[test]
fn errors_never_repeat_credentials() {
    let error = parse("git+https://user:secret@h.example/r.git@tag:v1").unwrap_err();
    assert!(!error.contains("secret"), "{error}");
}
