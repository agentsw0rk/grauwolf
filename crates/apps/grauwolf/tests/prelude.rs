//! `use grauwolf::prelude::*;` is enough for the everyday API (ADR-0108 stage 6 addendum).

use grauwolf::prelude::*;

#[test]
fn the_prelude_brings_the_spec_with_its_extension_trait() {
    let spec = AgentSpec::from_json(
        r#"{ "spec_version": 1, "name": "assistant",
             "model": { "provider": "ollama", "id": "qwen3.5:9b" },
             "strategy": { "kind": "single_run" } }"#,
    )
    .unwrap();

    assert!(spec.validate().is_ok());
    assert!(spec.warnings().is_empty());
    let env = MapEnv::new([("OLLAMA_HOST", "http://localhost:11434")]);
    assert!(Config::from_env(&env).is_ok());
}
