//! The prepared agent tree copied into an image's `app/` (ADR-0115): the sub-agents run from the
//! copy without network, toolchain or git.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use grauwolf_agent_tree::{APP_TOOL_CACHE, AgentTree, ReadOptions, ResolveOptions};
use grauwolf_config::{MapEnv, RegistryConfig};
use grauwolf_git::testing::{TestGitServer, commit_files};
use grauwolf_toolbuild::ToolBuilder;
use grauwolf_toolpkg::testing::{PONG_MODULE, TestPackage, TestRegistry};
use serde_json::json;

fn temp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("tree-copy-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn model() -> serde_json::Value {
    json!({ "provider": "ollama", "id": "m" })
}

#[tokio::test]
async fn sub_agents_with_a_package_and_a_git_skill_run_from_the_copy_without_network() {
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
            "skills": [{ "git": git.url("r"), "path": "skills", "rev": commit }],
            "builtin_tools": { "tools": ["store_value"], "root": "notes" }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::create_dir_all(src.join("agents/notes")).unwrap();
    std::fs::write(src.join("agents/notes/a.md"), "hello").unwrap();
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
    let config = RegistryConfig::from_env(&MapEnv::default());
    let mut tree = AgentTree::read(&src.join("root.json"), &ReadOptions::default()).unwrap();
    tree.resolve(&ResolveOptions {
        builder: &builder,
        locked: false,
        config: &config,
        build_code: true,
        resolve_root: true,
    })
    .await
    .unwrap();

    let app = dir.join("app");
    tree.copy_into(&app, &builder).unwrap();
    drop(registry);
    git.stop().await;
    std::fs::remove_dir_all(dir.join("cache")).unwrap();

    assert!(app.join("grauwolf.lock").is_file());
    // The git skills travel as directories, so the copied lockfile does not list them: no
    // agent of the copy would claim them and `--locked` would call them stale (ADR-0115).
    let copied_lock: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(app.join("grauwolf.lock")).unwrap()).unwrap();
    assert!(copied_lock.get("skills").is_none(), "{copied_lock}");
    assert!(app.join("agents/notes/a.md").is_file());
    assert!(app.join("agents/skills-git/0/haiku/SKILL.md").is_file());
    let copied: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(app.join("agents/helper.json")).unwrap())
            .unwrap();
    assert_eq!(copied["skills"], json!([{ "dir": "skills-git/0" }]));
}

/// The code tools of a sub-agent are built once and travel in the copy of the tool cache, with
/// the toolchain version that stands in for `rustc` in the image.
#[tokio::test]
#[ignore = "slow: builds tool code for WASM with cargo"]
async fn the_copy_carries_the_built_code_tools_of_sub_agents() {
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
    let config = RegistryConfig::from_env(&MapEnv::default());
    let mut tree = AgentTree::read(&dir.join("root.json"), &ReadOptions::default()).unwrap();
    tree.resolve(&ResolveOptions {
        builder: &builder,
        locked: false,
        config: &config,
        build_code: true,
        resolve_root: true,
    })
    .await
    .unwrap();
    let hash = tree.node(1).resolution().unwrap().artifacts[0].hash.clone();

    let app = dir.join("app");
    tree.copy_into(&app, &builder).unwrap();

    let cache = app.join(APP_TOOL_CACHE);
    assert!(cache.join(format!("artifacts/{hash}.wasm")).is_file());
    assert!(cache.join("toolchain").is_file());
    assert!(cache.join("Cargo.lock").is_file());
}
