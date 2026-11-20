//! Every spec in the example corpus `examples/specs/` must be valid.

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;

use grauwolf_spec::AgentSpec;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/specs")
}

#[test]
fn every_example_spec_is_valid() {
    let mut checked = Vec::new();
    for entry in std::fs::read_dir(corpus_dir()).expect("examples/specs is missing") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let json = std::fs::read_to_string(&path).unwrap();
            let spec =
                AgentSpec::from_json(&json).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            if let Err(errors) = spec.validate() {
                panic!("{}: {errors:#?}", path.display());
            }
            checked.push(path);
        }
    }

    assert!(
        checked.iter().any(|path| path.ends_with("echo.json")),
        "echo.json is missing from the corpus: {checked:?}"
    );
}

#[test]
fn every_example_spec_survives_a_round_trip_through_json() {
    for entry in std::fs::read_dir(corpus_dir()).expect("examples/specs is missing") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let spec = AgentSpec::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();

            let again = AgentSpec::from_json(&spec.to_json())
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));

            // A strategy is already expanded to `graph` after loading.
            let expected = AgentSpec {
                strategy: None,
                ..spec
            };
            assert_eq!(again, expected, "{}", path.display());
        }
    }
}
