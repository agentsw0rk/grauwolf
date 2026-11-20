//! `tool_packages` with git sources through the host: fetched from a local git server, built
//! with the real tool build, locked, then used offline from the cache.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use grauwolf_git::testing::{TestGitServer, commit_files};
use grauwolf_host::config::{MapEnv, RegistryConfig};
use grauwolf_host::packages::resolve_tools_with;
use grauwolf_spec::{AgentSpec, AgentSpecExt};
use grauwolf_toolbuild::ToolBuilder;
use serde_json::json;

fn example(name: &str) -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../docs/user-guide/examples/03-package")
            .join(name),
    )
    .unwrap()
}

fn temp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("git-packages-{name}"));
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
            "tool_packages": [{ "git": url, "path": "pkg", "rev": commit }]
        })
        .to_string(),
    )
    .unwrap()
}

#[test]
fn a_locked_spec_without_a_lockfile_fails_before_any_request() {
    let root = temp("locked");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(async {
        let repository = root.join("repo.git");
        commit_files(
            &repository,
            "main",
            "one",
            &BTreeMap::from([("x".into(), vec![1])]),
        );
        TestGitServer::start(&repository, None).await
    });
    let spec = spec(&server.url("r"), "0123456");
    let error = runtime
        .block_on(resolve_tools_with(
            &root.join("spec.json"),
            &spec,
            &root.join("cache"),
            true,
            &RegistryConfig::from_env(&MapEnv::default()),
            &ToolBuilder::new(root.join("cache")),
        ))
        .unwrap_err();
    assert!(error.contains("is not locked"), "{error}");
    assert!(server.requests().is_empty());
}

#[test]
#[ignore = "slow: builds tool code for WASM with cargo"]
fn builds_a_git_package_locks_it_and_then_works_offline() {
    let root = temp("build");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let repository = root.join("repo.git");
    let files: BTreeMap<String, Vec<u8>> =
        ["grauwolf-tools.json", "src/add.rs", "src/count_words.rs"]
            .into_iter()
            .map(|name| (format!("pkg/{name}"), example(name)))
            .collect();
    let commit = commit_files(&repository, "main", "one", &files).to_string();
    let (server, port) = runtime.block_on(async {
        let server = TestGitServer::start(&repository, None).await;
        let port = server.port();
        (server, port)
    });
    let spec = spec(&format!("http://127.0.0.1:{port}/r.git"), &commit[..7]);
    let file = root.join("spec.json");
    let config = RegistryConfig::from_env(&MapEnv::default());
    let builder = ToolBuilder::new(root.join("cache"));
    let tools = runtime
        .block_on(resolve_tools_with(
            &file,
            &spec,
            &root.join("cache"),
            false,
            &config,
            &builder,
        ))
        .unwrap();
    let names: Vec<_> = tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, ["add", "count_words"]);
    assert!(tools[0].artifact.bytes.starts_with(b"\0asm"));
    let lock = std::fs::read_to_string(root.join("grauwolf.lock")).unwrap();
    assert!(lock.contains(&commit), "{lock}");

    // Server gone, and a builder that cannot build: the cache and the lock are enough.
    runtime.block_on(server.stop());
    let broken = ToolBuilder::new(root.join("cache")).with_wrapper(vec!["false".into()]);
    let again = runtime
        .block_on(resolve_tools_with(
            &file,
            &spec,
            &root.join("cache"),
            true,
            &config,
            &broken,
        ))
        .unwrap();
    assert_eq!(again[0].artifact.hash, tools[0].artifact.hash);
    std::fs::remove_dir_all(&root).unwrap();
}
