//! The mapping file of the test git server's mTLS mode (ADR-0110).

use grauwolf_git::testing::{Grant, SpiffeAccess};
use grauwolf_spiffe::SpiffeId;

fn id(text: &str) -> SpiffeId {
    SpiffeId::parse(text).unwrap()
}

#[test]
fn an_exact_entry_wins_over_a_prefix_and_prefixes_match_whole_segments() {
    let access = SpiffeAccess::from_json(
        r#"{ "spiffe://d.de/ns/a/*": { "user": "team", "push": false },
             "spiffe://d.de/ns/a/sa/lead": { "user": "lead", "push": true } }"#,
    )
    .unwrap();
    let grant = |text: &str| access.grant(&id(text));
    assert_eq!(
        grant("spiffe://d.de/ns/a/sa/lead"),
        Some(Grant {
            user: "lead".into(),
            push: true
        })
    );
    assert_eq!(
        grant("spiffe://d.de/ns/a/sa/other"),
        Some(Grant {
            user: "team".into(),
            push: false
        })
    );
    assert_eq!(grant("spiffe://d.de/ns/ab/sa/x"), None);
    assert_eq!(grant("spiffe://d.de/ns/a"), None);
}

#[test]
fn a_bad_mapping_is_an_error_and_a_file_is_read_again() {
    assert!(SpiffeAccess::from_json(r#"{ "spiffe://d.de/x": { "push": true } }"#).is_err());
    assert!(SpiffeAccess::from_json(r#"{ "nonsense": { "user": "u" } }"#).is_err());
    let dir = grauwolf_test::TestDir::new("git-access-file");
    let file = dir.join("map.json");
    std::fs::write(
        &file,
        r#"{ "spiffe://d.de/x": { "user": "a", "push": false } }"#,
    )
    .unwrap();
    let access = SpiffeAccess::from_file(&file).unwrap();
    assert!(!access.grant(&id("spiffe://d.de/x")).unwrap().push);
    std::fs::write(
        &file,
        r#"{ "spiffe://d.de/x": { "user": "a", "push": true } }"#,
    )
    .unwrap();
    assert!(access.grant(&id("spiffe://d.de/x")).unwrap().push);
}
