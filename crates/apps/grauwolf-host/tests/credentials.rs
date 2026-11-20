//! Credential sources reach the loader (ADR-0110): the provider key comes from a file named in
//! the `grauwolf.toml` next to the spec instead of the environment.

use std::sync::Arc;

use grauwolf_config::MapEnv;
use grauwolf_host::loader::AgentLoader;
use grauwolf_test::TestDir;
use grauwolf_toolbuild::ToolBuilder;

const SPEC: &str = r#"{
  "spec_version": 1,
  "name": "creds",
  "model": { "provider": "openai", "id": "gpt-test" },
  "system_prompt": "x",
  "strategy": { "kind": "single_run" }
}"#;

fn loader(dir: &TestDir, vars: &[(&str, &str)]) -> AgentLoader {
    AgentLoader::new(ToolBuilder::new(dir.join("cache")))
        .with_env(Arc::new(MapEnv::new(vars.iter().copied())))
}

#[tokio::test]
async fn the_provider_key_comes_from_a_credential_file() {
    let dir = TestDir::new("host-credentials-load");
    std::fs::write(dir.join("agent.json"), SPEC).unwrap();
    std::fs::write(dir.join("openai-key"), "sk-from-file\n").unwrap();
    std::fs::write(
        dir.join("grauwolf.toml"),
        "[credentials.variables]\nOPENAI_API_KEY = \"file:openai-key\"\n",
    )
    .unwrap();
    loader(&dir, &[])
        .load(&dir.join("agent.json"), None)
        .await
        .unwrap();
}

#[tokio::test]
async fn without_the_credentials_the_missing_key_is_reported() {
    let dir = TestDir::new("host-credentials-missing");
    std::fs::write(dir.join("agent.json"), SPEC).unwrap();
    let error = loader(&dir, &[])
        .load(&dir.join("agent.json"), None)
        .await
        .err()
        .unwrap();
    assert!(error.contains("OPENAI_API_KEY"), "{error}");
}

#[tokio::test]
async fn a_mapped_variable_that_is_also_in_the_environment_fails_the_load() {
    let dir = TestDir::new("host-credentials-conflict");
    std::fs::write(dir.join("agent.json"), SPEC).unwrap();
    std::fs::write(dir.join("openai-key"), "sk-from-file").unwrap();
    std::fs::write(
        dir.join("grauwolf.toml"),
        "[credentials.variables]\nOPENAI_API_KEY = \"file:openai-key\"\n",
    )
    .unwrap();
    let error = loader(&dir, &[("OPENAI_API_KEY", "sk-env")])
        .load(&dir.join("agent.json"), None)
        .await
        .err()
        .unwrap();
    assert!(
        error.contains("OPENAI_API_KEY") && !error.contains("sk-"),
        "{error}"
    );
}
