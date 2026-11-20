//! The tool policy of the host: `grauwolf.toml` next to the spec and `GRAUWOLF_TOOL_POLICY`
//! (ADR-0107 stage 4).

use std::path::{Path, PathBuf};

use grauwolf_host::config::{MapEnv, RegistryConfig};
use grauwolf_host::packages::{POLICY_FILE, load_policy, resolve_tools_with};
use grauwolf_spec::{AgentSpec, AgentSpecExt};
use grauwolf_toolbuild::ToolBuilder;
use grauwolf_toolpkg::testing::{PONG_MODULE, TestPackage, TestRegistry};
use serde_json::json;

fn temp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("tool-policy-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(policy: Option<&Path>) -> RegistryConfig {
    match policy {
        Some(path) => RegistryConfig::from_env(&MapEnv::new([(
            "GRAUWOLF_TOOL_POLICY",
            path.to_str().unwrap(),
        )])),
        None => RegistryConfig::from_env(&MapEnv::default()),
    }
}

fn spec(reference: &str) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" },
            "tool_packages": [{ "oci": reference }]
        })
        .to_string(),
    )
    .unwrap()
}

async fn resolve(root: &Path, reference: &str, config: &RegistryConfig) -> Result<usize, String> {
    resolve_tools_with(
        &root.join("spec.json"),
        &spec(reference),
        &root.join("cache"),
        false,
        config,
        &ToolBuilder::new(root.join("cache")),
    )
    .await
    .map(|tools| tools.len())
}

fn write_policy(path: &Path, registries: &str) {
    std::fs::write(
        path,
        format!("[tool_packages]\nallowed_registries = [{registries}]\n"),
    )
    .unwrap();
}

#[test]
fn without_any_policy_file_there_is_no_policy() {
    let root = temp("none");
    assert!(
        load_policy(Some(&root.join("spec.json")), &config(None))
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn grauwolf_toml_next_to_the_spec_applies() {
    let root = temp("toml");
    let registry = TestRegistry::start().await;
    registry.push(
        "team/add",
        "1.0",
        &TestPackage::new("p", "1").tool(
            "add",
            "Adds",
            json!({ "type": "object" }),
            PONG_MODULE.as_bytes(),
        ),
    );
    let reference = format!("{}/team/add:1.0", registry.host());
    write_policy(&root.join(POLICY_FILE), "\"ghcr.io/acme\"");
    let error = resolve(&root, &reference, &config(None)).await.unwrap_err();
    assert!(error.contains("allowed_registries"), "{error}");
    write_policy(&root.join(POLICY_FILE), &format!("\"{}\"", registry.host()));
    assert_eq!(resolve(&root, &reference, &config(None)).await, Ok(1));
}

#[tokio::test]
async fn the_environment_policy_wins_over_grauwolf_toml() {
    let root = temp("env");
    let registry = TestRegistry::start().await;
    registry.push(
        "team/add",
        "1.0",
        &TestPackage::new("p", "1").tool(
            "add",
            "Adds",
            json!({ "type": "object" }),
            PONG_MODULE.as_bytes(),
        ),
    );
    let reference = format!("{}/team/add:1.0", registry.host());
    // The file next to the spec allows the registry, the environment policy does not ...
    write_policy(&root.join(POLICY_FILE), &format!("\"{}\"", registry.host()));
    let strict = root.join("strict.toml");
    write_policy(&strict, "\"ghcr.io/acme\"");
    assert!(
        resolve(&root, &reference, &config(Some(&strict)))
            .await
            .is_err()
    );
    // ... and the other way round.
    write_policy(&root.join(POLICY_FILE), "\"ghcr.io/acme\"");
    let open = root.join("open.toml");
    write_policy(&open, &format!("\"{}\"", registry.host()));
    assert_eq!(
        resolve(&root, &reference, &config(Some(&open))).await,
        Ok(1)
    );
}

#[tokio::test]
async fn a_named_policy_file_that_is_missing_is_an_error() {
    let root = temp("missing");
    let error = resolve(
        &root,
        "localhost:1/a/b:1",
        &config(Some(&root.join("nope.toml"))),
    )
    .await
    .unwrap_err();
    assert!(error.contains("nope.toml"), "{error}");
}
