//! The host's read of an agent tree (ADR-0115, ADR-0118): the generic read phase lives in
//! `grauwolf-agent-tree`; what the host adds are the checks only it can make.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use grauwolf_git::testing::{TestGitServer, commit_files};
use grauwolf_host::agent_tree::{APP_TOOL_CACHE, ResolveOptions};
use grauwolf_host::config::{MapEnv, RegistryConfig};
use grauwolf_host::loader::AgentLoader;
use grauwolf_host::provisioning::SubAgentSupplier as _;
use grauwolf_toolbuild::ToolBuilder;
use grauwolf_toolpkg::testing::{PONG_MODULE, TestPackage, TestRegistry};
use serde_json::json;

#[test]
fn the_host_refuses_an_unknown_provider_and_a_missing_skill_directory() {
    let dir = grauwolf_test::TestDir::new("host-tree");
    std::fs::write(
        dir.join("root.json"),
        r#"{ "spec_version": 1, "name": "root", "model": { "provider": "nope", "id": "m" },
             "strategy": { "kind": "single_run" }, "skills": [ { "dir": "no-such-skills" } ] }"#,
    )
    .unwrap();

    let error = grauwolf_host::agent_tree::read(&dir.join("root.json"))
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("root.json:/model/provider: unknown provider 'nope'"),
        "{error}"
    );
    assert!(error.contains("root.json:/skills:"), "{error}");
}

fn temp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("tree-copy-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn model() -> serde_json::Value {
    json!({ "provider": "ollama", "id": "m" })
}

/// Reads and resolves the tree of `root`, copies it into `<dir>/app` and returns the copy.
async fn resolve_and_copy(dir: &Path, root: &Path, builder: &ToolBuilder) -> PathBuf {
    let config = RegistryConfig::from_env(&MapEnv::default());
    let mut tree = grauwolf_host::agent_tree::read(root).unwrap();
    tree.resolve(&ResolveOptions {
        builder,
        locked: false,
        config: &config,
        build_code: true,
        resolve_root: true,
    })
    .await
    .unwrap();
    let app = dir.join("app");
    tree.copy_into(&app, builder).unwrap();
    app
}

/// The copy of the tree is what the service loads: locked, from the cache in the copy.
async fn supplied_agents(root: &Path, app: &Path) -> usize {
    let spec = grauwolf_host::loader::read_spec(root).unwrap();
    let loader = AgentLoader::new(ToolBuilder::new(app.join(APP_TOOL_CACHE)))
        .with_locked(true)
        .with_env(Arc::new(MapEnv::default()));
    loader.supply(&spec, app).await.unwrap().len()
}

#[tokio::test]
async fn the_host_loads_sub_agents_with_a_package_and_a_git_skill_from_the_copy_without_network() {
    let dir = temp("offline");
    let registry = TestRegistry::start().await;
    registry.push(
        "team/ping",
        "1.0",
        &TestPackage::new("p", "1").tool(
            "ping",
            "d",
            json!({ "type": "object" }),
            PONG_MODULE.as_bytes(),
        ),
    );
    let reference = format!("{}/team/ping:1.0", registry.host());
    let repository = dir.join("repo.git");
    let files: BTreeMap<String, Vec<u8>> = BTreeMap::from([(
        "skills/haiku/SKILL.md".to_owned(),
        b"---\nname: haiku\ndescription: Writes haiku\n---\nFive.".to_vec(),
    )]);
    let commit = commit_files(&repository, "main", "one", &files).to_string();
    let git = TestGitServer::start(&repository, None).await;
    let src = dir.join("src");
    std::fs::create_dir_all(src.join("agents")).unwrap();
    std::fs::write(
        src.join("agents/helper.json"),
        json!({
            "spec_version": 1, "name": "helper", "model": model(), "strategy": { "kind": "single_run" },
            "tool_packages": [{ "oci": reference }],
            "skills": [{ "git": git.url("r"), "path": "skills", "rev": commit }]
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        src.join("root.json"),
        json!({
            "spec_version": 1, "name": "root", "model": model(), "strategy": { "kind": "single_run" },
            "agent_tools": { "helper": { "spec": "agents/helper.json", "description": "Helps" } }
        })
        .to_string(),
    )
    .unwrap();
    let builder = ToolBuilder::new(dir.join("cache"));
    let app = resolve_and_copy(&dir, &src.join("root.json"), &builder).await;
    drop(registry);
    git.stop().await;
    std::fs::remove_dir_all(dir.join("cache")).unwrap();

    assert_eq!(supplied_agents(&src.join("root.json"), &app).await, 1);
}

#[tokio::test]
#[ignore = "slow: builds tool code for WASM with cargo"]
async fn the_host_loads_the_built_code_tools_of_sub_agents_from_the_copy() {
    let dir = temp("code");
    std::fs::write(
        dir.join("helper.json"),
        json!({
            "spec_version": 1, "name": "helper", "model": model(), "strategy": { "kind": "single_run" },
            "tools": [{ "name": "shout", "description": "d",
              "args": { "type": "object", "properties": { "t": { "type": "string" } } },
              "code": "Ok(args[\"t\"].as_str().unwrap_or(\"\").to_uppercase())" }]
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("root.json"),
        json!({
            "spec_version": 1, "name": "root", "model": model(), "strategy": { "kind": "single_run" },
            "agent_tools": { "helper": { "spec": "helper.json", "description": "Helps" } }
        })
        .to_string(),
    )
    .unwrap();
    let builder = ToolBuilder::new(dir.join("cache"));
    let app = resolve_and_copy(&dir, &dir.join("root.json"), &builder).await;
    // The sub-agent loads from the copy without building anything.
    std::fs::remove_dir_all(dir.join("cache")).unwrap();

    assert_eq!(supplied_agents(&dir.join("root.json"), &app).await, 1);
}
