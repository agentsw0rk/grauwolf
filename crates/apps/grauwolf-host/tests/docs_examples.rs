//! The example specs of the handbooks must be valid – so the documentation does not go stale
//! unnoticed: `docs/user-guide/examples/*.json` (English) and `docs/handbuch/beispiele/*.json`
//! (German version, ADR-0090 addendum). Tool files are in subdirectories.

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;

use grauwolf_host::loader::load_spec;

/// Checks all specs in `dir` (relative to the workspace); `first` must be among them.
fn check_examples(dir: &str, first: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join(dir);
    let mut checked = Vec::new();
    for entry in
        std::fs::read_dir(&dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
    {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            let spec = load_spec(&path).unwrap_or_else(|error| panic!("{error}"));
            if let Err(errors) = spec.validate() {
                panic!("{}: {errors:#?}", path.display());
            }
            checked.push(path);
        }
    }

    assert!(
        checked.iter().any(|path| path.ends_with(first)),
        "{first} is missing: {checked:?}"
    );
}

#[test]
fn every_example_of_the_user_guide_is_a_valid_spec() {
    check_examples("docs/user-guide/examples", "01-hello.json");
}

#[test]
fn every_example_of_the_german_handbook_is_a_valid_spec() {
    check_examples("docs/handbuch/beispiele", "01-hallo.json");
}
