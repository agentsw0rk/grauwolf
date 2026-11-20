//! The prepared agent tree, resolve phase (ADR-0115): one lockfile and one policy per tree.

use std::path::{Path, PathBuf};

use grauwolf_agent_tree::{AgentTree, ReadOptions, ResolveOptions};
use grauwolf_config::{MapEnv, RegistryConfig};
use grauwolf_toolbuild::ToolBuilder;
use grauwolf_toolpkg::testing::{PONG_MODULE, TestPackage, TestRegistry};
use serde_json::json;

fn temp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("tree-resolve-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn push(registry: &TestRegistry, repository: &str, tool: &str) -> String {
    registry.push(
        repository,
        "1.0",
        &TestPackage::new("p", "1").tool(
            tool,
            "d",
            json!({ "type": "object" }),
            PONG_MODULE.as_bytes(),
        ),
    );
    format!("{}/{repository}:1.0", registry.host())
}

fn write(dir: &Path, file: &str, packages: &[&str], subs: &[(&str, &str)]) {
    let mut value = json!({
        "spec_version": 1, "name": file.trim_end_matches(".json").replace('/', "_"),
        "model": { "provider": "ollama", "id": "m" }, "strategy": { "kind": "single_run" },
        "tool_packages": packages.iter().map(|p| json!({ "oci": p })).collect::<Vec<_>>(),
    });
    if !subs.is_empty() {
        value["agent_tools"] = subs
            .iter()
            .map(|(n, f)| ((*n).to_owned(), json!({ "spec": f, "description": "d" })))
            .collect::<serde_json::Map<_, _>>()
            .into();
    }
    std::fs::create_dir_all(dir.join(file).parent().unwrap()).unwrap();
    std::fs::write(dir.join(file), value.to_string()).unwrap();
}

async fn resolve(dir: &Path, root: &str, locked: bool, env: &MapEnv) -> Result<AgentTree, String> {
    let mut tree =
        AgentTree::read(&dir.join(root), &ReadOptions::default()).map_err(|e| e.to_string())?;
    let config = RegistryConfig::from_env(env);
    let builder = ToolBuilder::new(dir.join("cache"));
    tree.resolve(&ResolveOptions {
        builder: &builder,
        locked,
        config: &config,
        build_code: true,
        resolve_root: true,
    })
    .await?;
    Ok(tree)
}

fn lock(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("grauwolf.lock")).unwrap()).unwrap()
}

#[tokio::test]
async fn one_lockfile_next_to_the_root_covers_all_nodes_with_prefixed_keys() {
    let dir = temp("one-lock");
    let registry = TestRegistry::start().await;
    let (a, b) = (
        push(&registry, "team/a", "tool_a"),
        push(&registry, "team/b", "tool_b"),
    );
    write(&dir, "root.json", &[&a], &[("w", "agents/writer.json")]);
    write(&dir, "agents/writer.json", &[&b], &[]);

    let tree = resolve(&dir, "root.json", false, &MapEnv::default())
        .await
        .unwrap();

    let packages = lock(&dir)["packages"].as_object().unwrap().clone();
    let keys: Vec<&String> = packages.keys().collect();
    assert_eq!(keys, [&a, &format!("agents/writer.json#{b}")]);
    assert!(!dir.join("agents/grauwolf.lock").exists());
    let tools = |i: usize| -> Vec<String> {
        tree.node(i)
            .resolution()
            .unwrap()
            .package_tools
            .iter()
            .map(|t| t.name.clone())
            .collect()
    };
    assert_eq!(
        (tools(0), tools(1)),
        (vec!["tool_a".into()], vec!["tool_b".into()])
    );
}

#[tokio::test]
async fn locked_resolving_works_without_network_and_refuses_unlocked_entries() {
    let dir = temp("locked");
    let registry = TestRegistry::start().await;
    let a = push(&registry, "team/a", "tool_a");
    write(&dir, "root.json", &[], &[("w", "writer.json")]);
    write(&dir, "writer.json", &[&a], &[]);
    let env = MapEnv::default();
    let before = resolve(&dir, "root.json", true, &env).await.unwrap_err();
    assert!(
        before.contains("is out of date") || before.contains("is not locked"),
        "{before}"
    );
    resolve(&dir, "root.json", false, &env).await.unwrap();
    drop(registry);

    resolve(&dir, "root.json", true, &env).await.unwrap();
}

#[tokio::test]
async fn a_lockfile_next_to_a_sub_spec_is_not_used_and_gives_a_warning() {
    let dir = temp("sub-lock");
    let registry = TestRegistry::start().await;
    let a = push(&registry, "team/a", "tool_a");
    write(&dir, "root.json", &[], &[("w", "agents/writer.json")]);
    write(&dir, "agents/writer.json", &[&a], &[]);
    std::fs::write(
        dir.join("agents/grauwolf.lock"),
        "{ \"lock_version\": 1, \"packages\": {} }",
    )
    .unwrap();

    let tree = resolve(&dir, "root.json", false, &MapEnv::default())
        .await
        .unwrap();

    assert!(
        tree.warnings()
            .iter()
            .any(|w| w.contains("agents/grauwolf.lock") && w.contains("not used")),
        "{:?}",
        tree.warnings()
    );
    assert!(
        lock(&dir)["packages"]
            .as_object()
            .unwrap()
            .contains_key(&format!("agents/writer.json#{a}"))
    );
}

#[tokio::test]
async fn the_policy_of_the_root_applies_to_every_node() {
    let dir = temp("policy");
    let registry = TestRegistry::start().await;
    let a = push(&registry, "team/a", "tool_a");
    write(&dir, "root.json", &[], &[("w", "agents/writer.json")]);
    write(&dir, "agents/writer.json", &[&a], &[]);
    // The sub-spec brings a lax policy, the root a strict one.
    let policy =
        |registries: &str| format!("[tool_packages]\nallowed_registries = [{registries}]\n");
    std::fs::write(
        dir.join("agents/grauwolf.toml"),
        policy(&format!("\"{}\"", registry.host())),
    )
    .unwrap();
    std::fs::write(dir.join("grauwolf.toml"), policy("\"ghcr.io/acme\"")).unwrap();

    let error = resolve(&dir, "root.json", false, &MapEnv::default())
        .await
        .unwrap_err();

    assert!(
        error.contains("writer.json") && error.contains("allowed_registries"),
        "{error}"
    );
}

#[tokio::test]
async fn a_diamond_is_resolved_once() {
    let dir = temp("diamond");
    let registry = TestRegistry::start().await;
    let a = push(&registry, "team/a", "tool_a");
    write(&dir, "root.json", &[], &[("x", "x.json"), ("y", "y.json")]);
    write(&dir, "x.json", &[], &[("s", "shared.json")]);
    write(&dir, "y.json", &[], &[("s", "shared.json")]);
    write(&dir, "shared.json", &[&a], &[]);

    let tree = resolve(&dir, "root.json", false, &MapEnv::default())
        .await
        .unwrap();

    assert_eq!(tree.nodes().len(), 4);
    assert_eq!(lock(&dir)["packages"].as_object().unwrap().len(), 1);
}

#[tokio::test]
async fn a_tree_without_packages_leaves_the_lockfile_alone() {
    let dir = temp("nothing");
    write(&dir, "root.json", &[], &[("w", "writer.json")]);
    write(&dir, "writer.json", &[], &[]);

    resolve(&dir, "root.json", true, &MapEnv::default())
        .await
        .unwrap();

    assert!(!dir.join("grauwolf.lock").exists());
}

/// Code tools of every node are built into the one cache of the builder.
#[tokio::test]
#[ignore = "slow: builds tool code for WASM with cargo"]
async fn code_tools_of_sub_agents_are_built_into_the_shared_cache() {
    let dir = temp("code");
    write(&dir, "root.json", &[], &[("w", "writer.json")]);
    let writer = json!({
        "spec_version": 1, "name": "writer", "model": { "provider": "ollama", "id": "m" },
        "strategy": { "kind": "single_run" },
        "tools": [{ "name": "shout", "description": "d",
          "args": { "type": "object", "properties": { "t": { "type": "string" } } },
          "code": "Ok(args[\"t\"].as_str().unwrap_or(\"\").to_uppercase())" }]
    });
    std::fs::write(dir.join("writer.json"), writer.to_string()).unwrap();

    let tree = resolve(&dir, "root.json", false, &MapEnv::default())
        .await
        .unwrap();

    assert_eq!(tree.node(1).resolution().unwrap().artifacts.len(), 1);
    assert!(tree.node(0).resolution().unwrap().artifacts.is_empty());
    assert_eq!(
        std::fs::read_dir(dir.join("cache/artifacts"))
            .unwrap()
            .count(),
        1
    );
}

/// Adds `entry` under `section` of the lockfile in `dir`.
fn add_lock_entry(dir: &Path, section: &str, key: &str, entry: serde_json::Value) {
    let mut value = if dir.join("grauwolf.lock").exists() {
        lock(dir)
    } else {
        json!({ "lock_version": 1, "packages": {} })
    };
    value[section][key] = entry;
    std::fs::write(dir.join("grauwolf.lock"), value.to_string()).unwrap();
}

#[tokio::test]
async fn an_entry_left_over_from_a_removed_sub_agent_is_stale_although_the_root_has_no_packages() {
    let dir = temp("left-over");
    let registry = TestRegistry::start().await;
    let a = push(&registry, "team/a", "tool_a");
    write(&dir, "root.json", &[], &[("w", "writer.json")]);
    write(&dir, "writer.json", &[&a], &[]);
    let env = MapEnv::default();
    resolve(&dir, "root.json", false, &env).await.unwrap();
    let key = format!("writer.json#{a}");
    assert!(lock(&dir)["packages"].get(&key).is_some());
    // The sub-agent is removed from the tree; the lockfile is not.
    write(&dir, "root.json", &[], &[]);
    drop(registry);

    let error = resolve(&dir, "root.json", true, &env).await.unwrap_err();
    assert!(
        error.contains("grauwolf.lock is out of date") && error.contains(&key),
        "{error}"
    );
    assert!(
        lock(&dir)["packages"].get(&key).is_some(),
        "--locked writes nothing"
    );

    resolve(&dir, "root.json", false, &env).await.unwrap();
    // A lockfile without entries is removed (`Lockfile::save`).
    assert!(!dir.join("grauwolf.lock").exists());
    resolve(&dir, "root.json", true, &env).await.unwrap();
}

#[tokio::test]
async fn a_sub_agent_only_tree_refuses_and_drops_root_level_entries_that_nobody_claims() {
    let dir = temp("sub-only");
    let registry = TestRegistry::start().await;
    let (a, b) = (
        push(&registry, "team/a", "tool_a"),
        push(&registry, "team/b", "tool_b"),
    );
    write(&dir, "root.json", &[], &[("w", "writer.json")]);
    write(&dir, "writer.json", &[&a], &[]);
    let env = MapEnv::default();
    resolve(&dir, "root.json", false, &env).await.unwrap();
    let stale = lock(&dir)["packages"][format!("writer.json#{a}")].clone();
    add_lock_entry(&dir, "packages", &b, stale);

    let error = resolve(&dir, "root.json", true, &env).await.unwrap_err();
    assert!(error.contains(&format!("`{b}`")), "{error}");

    resolve(&dir, "root.json", false, &env).await.unwrap();
    let keys: Vec<String> = lock(&dir)["packages"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys, [format!("writer.json#{a}")]);
}

#[tokio::test]
async fn unclaimed_git_and_skills_entries_are_stale_in_a_tree_without_sources() {
    let dir = temp("git-skills");
    write(&dir, "root.json", &[], &[("w", "writer.json")]);
    write(&dir, "writer.json", &[], &[]);
    let commit = "0123456789abcdef0123456789abcdef01234567";
    add_lock_entry(
        &dir,
        "git",
        "git+https://example.org/tools.git@main",
        json!({ "commit": commit, "modules": {} }),
    );
    add_lock_entry(
        &dir,
        "skills",
        "git+https://example.org/skills.git@main",
        json!({ "commit": commit, "tree": "sha256:00" }),
    );
    let env = MapEnv::default();

    let error = resolve(&dir, "root.json", true, &env).await.unwrap_err();
    assert!(
        error.contains("git+https://example.org/tools.git@main")
            || error.contains("git+https://example.org/skills.git@main"),
        "{error}"
    );

    resolve(&dir, "root.json", false, &env).await.unwrap();
    assert!(!dir.join("grauwolf.lock").exists());
    resolve(&dir, "root.json", true, &env).await.unwrap();
}
