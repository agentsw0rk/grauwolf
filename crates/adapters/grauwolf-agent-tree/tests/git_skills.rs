//! `skills` with git sources in the agent tree: fetched from a local git server, locked, then used
//! offline from the cache as a local directory (ADR-0103).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use grauwolf_agent_tree::{AgentTree, ReadOptions, ResolveOptions};
use grauwolf_builtin_tools::{SkillLibrary, SkillRoot};
use grauwolf_config::{MapEnv, RegistryConfig};
use grauwolf_git::testing::{TestGitServer, commit_files};
use grauwolf_spec::{AgentSpec, AgentSpecExt};
use serde_json::json;

fn temp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("git-skills-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn spec(url: &str, commit: &str) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" },
            "skills": [{ "git": url, "path": "skills", "rev": commit, "names": ["haiku-writer"] }]
        })
        .to_string(),
    )
    .unwrap()
}

#[tokio::test]
async fn a_git_source_becomes_a_local_directory_that_the_skill_tools_read() {
    let root = temp("localize");
    let repository = root.join("repo.git");
    let files: BTreeMap<String, Vec<u8>> = BTreeMap::from([
        (
            "skills/haiku-writer/SKILL.md".to_owned(),
            b"---\nname: haiku-writer\ndescription: Writes haiku\n---\nFive.".to_vec(),
        ),
        (
            "skills/other/SKILL.md".to_owned(),
            b"---\nname: other\ndescription: Other\n---\nx".to_vec(),
        ),
    ]);
    let commit = commit_files(&repository, "main", "one", &files).to_string();
    let server = TestGitServer::start(&repository, None).await;
    let file = root.join("spec.json");
    let mut value = serde_json::to_value(spec(&server.url("r"), &commit)).unwrap();
    value.as_object_mut().unwrap().remove("graph");
    value["strategy"] = json!({ "kind": "single_run" });
    std::fs::write(&file, value.to_string()).unwrap();
    let config = RegistryConfig::from_env(&MapEnv::default());
    let builder = grauwolf_toolbuild::ToolBuilder::new(root.join("cache"));
    let resolve = |locked: bool| {
        let (file, config, builder) = (file.clone(), config.clone(), builder.clone());
        async move {
            let mut tree = AgentTree::read(&file, &ReadOptions::default()).unwrap();
            tree.resolve(&ResolveOptions {
                builder: &builder,
                locked,
                config: &config,
                build_code: true,
                resolve_root: true,
            })
            .await
            .map(|()| tree)
        }
    };

    let tree = resolve(false).await.unwrap();

    let local = tree.root().resolution().unwrap().spec.clone();
    assert!(local.skills[0].git.is_none());
    assert_eq!(local.skills[0].names, Some(vec!["haiku-writer".to_owned()]));
    let library = SkillLibrary::load(&[SkillRoot {
        dir: root.join(local.skills[0].dir.as_ref().unwrap()),
        names: local.skills[0].names.clone(),
    }])
    .unwrap();
    let tools = library.tools();
    let text = tools[0]
        .call_json(json!({ "name": "haiku-writer" }))
        .await
        .unwrap();
    assert!(text.starts_with("Five."), "{text}");
    assert!(root.join("grauwolf.lock").is_file());
    // Offline and locked.
    server.stop().await;
    resolve(true).await.unwrap();
}

#[tokio::test]
async fn a_spec_without_git_skills_needs_no_cache_and_no_lockfile() {
    let root = temp("none");
    std::fs::create_dir_all(root.join("skills/haiku-writer")).unwrap();
    std::fs::write(
        root.join("skills/haiku-writer/SKILL.md"),
        "---\nname: haiku-writer\ndescription: d\n---\nx",
    )
    .unwrap();
    std::fs::write(
        root.join("spec.json"),
        json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" },
            "skills": [{ "dir": "skills" }]
        })
        .to_string(),
    )
    .unwrap();
    let mut tree = AgentTree::read(&root.join("spec.json"), &ReadOptions::default()).unwrap();
    tree.resolve(&ResolveOptions {
        builder: &grauwolf_toolbuild::ToolBuilder::new(root.join("cache")),
        locked: true,
        config: &RegistryConfig::from_env(&MapEnv::default()),
        build_code: true,
        resolve_root: true,
    })
    .await
    .unwrap();

    assert_eq!(tree.root().resolution().unwrap().spec, *tree.root().spec());
    assert!(!root.join("cache").exists() && !root.join("grauwolf.lock").exists());
}
