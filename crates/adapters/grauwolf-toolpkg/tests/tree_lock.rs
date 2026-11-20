//! Locking an agent tree (ADR-0115, ADR-0118): one lockfile, prefixed keys for sub-agents.

use std::path::Path;

use grauwolf_config::{MapEnv, RegistryConfig};
use grauwolf_spec::{AgentSpec, AgentSpecExt as _};
use grauwolf_toolbuild::ToolBuilder;
use grauwolf_toolpkg::testing::{PONG_MODULE, TestPackage, TestRegistry};
use grauwolf_toolpkg::tree_lock::{LockNode, LockOptions, lock_tree};
use grauwolf_toolpkg::{LOCKFILE_NAME, Lockfile};
use serde_json::json;

fn spec(name: &str, packages: serde_json::Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": name,
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" },
            "tool_packages": packages
        })
        .to_string(),
    )
    .unwrap()
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("grauwolf-tree-lock-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn options<'a>(builder: &'a ToolBuilder, config: &'a RegistryConfig) -> LockOptions<'a> {
    LockOptions {
        builder,
        locked: false,
        config,
        resolve_root: true,
    }
}

#[tokio::test]
async fn the_root_keeps_plain_keys_and_a_sub_agent_gets_its_spec_path_as_prefix() {
    let registry = TestRegistry::start().await;
    registry.push(
        "test/math",
        "1.0",
        &TestPackage::new("acme-math", "1.0.0").tool(
            "add",
            "Adds",
            json!({ "type": "object" }),
            PONG_MODULE.as_bytes(),
        ),
    );
    let reference = format!("{}/test/math:1.0", registry.host());
    let dir = temp("prefix");
    let root = spec("root", json!([{ "oci": reference }]));
    let sub = spec("sub", json!([{ "oci": reference }]));
    let (root_file, sub_file) = (dir.join("root.json"), dir.join("agents/sub.json"));
    let relative = Path::new("agents/sub.json");
    let builder = ToolBuilder::new(dir.join("cache"));
    let config = RegistryConfig::from_env(&MapEnv::default());

    let locked = lock_tree(
        &[
            LockNode {
                file: &root_file,
                relative: Path::new("root.json"),
                spec: &root,
            },
            LockNode {
                file: &sub_file,
                relative,
                spec: &sub,
            },
        ],
        &options(&builder, &config),
    )
    .await
    .unwrap();

    assert_eq!(locked.len(), 2);
    assert_eq!(locked[0].package_tools.len(), 1);
    assert_eq!(locked[1].package_tools.len(), 1);
    let lock = Lockfile::load(&dir.join(LOCKFILE_NAME)).unwrap();
    let mut keys: Vec<&String> = lock.packages.keys().collect();
    keys.sort();
    let mut expected = [format!("agents/sub.json#{reference}"), reference.clone()];
    expected.sort();
    assert_eq!(keys, expected.iter().collect::<Vec<_>>());
}

#[tokio::test]
async fn a_tree_without_packages_leaves_the_lockfile_alone() {
    let dir = temp("none");
    let root = spec("root", json!([]));
    let root_file = dir.join("root.json");
    let builder = ToolBuilder::new(dir.join("cache"));
    let config = RegistryConfig::from_env(&MapEnv::default());

    let locked = lock_tree(
        &[LockNode {
            file: &root_file,
            relative: Path::new("root.json"),
            spec: &root,
        }],
        &options(&builder, &config),
    )
    .await
    .unwrap();

    assert!(locked[0].package_tools.is_empty());
    assert!(!dir.join(LOCKFILE_NAME).exists());
}
