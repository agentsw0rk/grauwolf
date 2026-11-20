//! `grauwolf tools lock` and `grauwolf-tools lock` read the same agent tree (ADR-0118) and write
//! the same lockfile: a diamond, a sub-agent with a tool package and a git skill, against the
//! test registry and the test git server.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use grauwolf_cli::Io;
use grauwolf_git::testing::{TestGitServer, commit_files};
use grauwolf_toolpkg::testing::{PONG_MODULE, TestPackage, TestRegistry};
use serde_json::json;

const SKILL: &str = "---\nname: haiku-writer\ndescription: Writes haiku\n---\nFive.";

fn work(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("lock-equality-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// root -> {writer, reviewer}, writer -> reviewer (a diamond); the reviewer has a tool package,
/// the writer a git skill.
fn write_tree(dir: &Path, package: &str, skills_git: &str, commit: &str) {
    let spec = |name: &str, extra: serde_json::Value| {
        let mut spec = json!({
            "spec_version": 1, "name": name,
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" }
        });
        spec.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        spec.to_string()
    };
    let agent = |file: &str| json!({ "spec": file, "description": "d" });
    std::fs::create_dir_all(dir.join("agents")).unwrap();
    std::fs::write(
        dir.join("agents/reviewer.json"),
        spec("reviewer", json!({ "tool_packages": [{ "oci": package }] })),
    )
    .unwrap();
    std::fs::write(
        dir.join("agents/writer.json"),
        spec(
            "writer",
            json!({
                "skills": [{ "git": skills_git, "path": "skills", "rev": commit }],
                "agent_tools": { "reviewer": agent("reviewer.json") }
            }),
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("root.json"),
        spec(
            "root",
            json!({ "agent_tools": {
                "writer": agent("agents/writer.json"),
                "reviewer": agent("agents/reviewer.json")
            } }),
        ),
    )
    .unwrap();
}

/// Runs `program args…` in `dir` (`--cache` inside it) and returns the exit success, stdout and
/// the lockfile with `dir` made anonymous.
fn lock(
    run: impl Fn(&[&str], &Io) -> std::process::ExitCode,
    dir: &Path,
) -> (bool, String, String) {
    let root = dir.join("root.json");
    let cache = dir.join("cache");
    let args = [
        "lock",
        root.to_str().unwrap(),
        "--cache",
        cache.to_str().unwrap(),
    ];
    let (io, captured) = Io::captured();
    let code = run(&args, &io);
    let anonymous = |text: String| text.replace(dir.to_str().unwrap(), "<dir>");
    let lockfile = std::fs::read_to_string(dir.join("grauwolf.lock")).unwrap_or_default();
    (
        code == std::process::ExitCode::SUCCESS,
        anonymous(captured.stdout()),
        anonymous(lockfile),
    )
}

#[test]
fn both_programs_write_the_same_lockfile_for_a_tree_with_a_diamond_a_package_and_a_git_skill() {
    let base = work("same");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let registry = runtime.block_on(TestRegistry::start());
    registry.push(
        "test/ping",
        "1.0",
        &TestPackage::new("ping-pack", "1.0.0").tool(
            "ping",
            "Answers pong",
            json!({ "type": "object" }),
            PONG_MODULE.as_bytes(),
        ),
    );
    let package = format!("{}/test/ping:1.0", registry.host());
    let repository = base.join("repo.git");
    let files = BTreeMap::from([(
        "skills/haiku-writer/SKILL.md".to_owned(),
        SKILL.as_bytes().to_vec(),
    )]);
    let commit = commit_files(&repository, "main", "one", &files).to_string();
    let server = runtime.block_on(TestGitServer::start(&repository, None));
    let (host_dir, tools_dir) = (base.join("host"), base.join("tools"));
    for dir in [&host_dir, &tools_dir] {
        write_tree(dir, &package, &server.url("r"), &commit);
    }

    let host = lock(
        |args, io| {
            grauwolf_cli::run(
                ["grauwolf", "tools"]
                    .into_iter()
                    .chain(args.iter().copied()),
                io,
            )
        },
        &host_dir,
    );
    let tools = lock(
        |args, io| {
            grauwolf_tools_app::run(
                ["grauwolf-tools"].into_iter().chain(args.iter().copied()),
                io,
            )
        },
        &tools_dir,
    );

    assert!(host.0 && tools.0, "{host:?} {tools:?}");
    assert!(host.2.contains("agents/writer.json#git+"), "{}", host.2);
    assert!(host.2.contains("agents/reviewer.json#"), "{}", host.2);
    assert_eq!(host.2, tools.2, "lockfiles differ");
    assert_eq!(host.1, tools.1, "output differs");
    runtime.block_on(server.stop());
}
