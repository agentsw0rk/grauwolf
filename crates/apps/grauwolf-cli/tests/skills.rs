//! `skills` on the command line (ADR-0103): `spec validate` checks local skill directories,
//! `tools lock` locks git sources, `--locked` works from the cache. The binary runs with an empty
//! `PATH` (no toolchain) against the in-process test git server.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use grauwolf_git::testing::{TestGitServer, commit_files};
use serde_json::json;

fn work(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("skills-cli-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn grauwolf(args: &[&str], dir: &Path) -> Output {
    let empty = dir.join("no-tools");
    std::fs::create_dir_all(&empty).unwrap();
    Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(args)
        .env("PATH", &empty)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn write_spec(dir: &Path, skills: serde_json::Value) -> PathBuf {
    let file = dir.join("agent.json");
    std::fs::write(
        &file,
        json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" },
            "skills": skills
        })
        .to_string(),
    )
    .unwrap();
    file
}

const SKILL: &str = "---\nname: haiku-writer\ndescription: Writes haiku\n---\nFive.";

#[test]
fn validate_checks_the_local_skill_directories() {
    let dir = work("validate");
    std::fs::create_dir_all(dir.join("skills/haiku-writer")).unwrap();
    std::fs::write(dir.join("skills/haiku-writer/SKILL.md"), SKILL).unwrap();
    let file = write_spec(&dir, json!([{ "dir": "skills" }]));
    let output = grauwolf(&["spec", "validate", file.to_str().unwrap()], &dir);
    assert!(output.status.success(), "{}", text(&output.stderr));

    std::fs::write(dir.join("skills/haiku-writer/SKILL.md"), "no front matter").unwrap();
    let output = grauwolf(&["spec", "validate", file.to_str().unwrap()], &dir);
    assert!(!output.status.success());
    let error = text(&output.stderr);
    assert!(
        error.contains("agent.json:/skills") && error.contains("SKILL.md"),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tools_lock_locks_git_skills_and_locked_runs_work_offline() {
    let dir = work("lock");
    let repository = dir.join("repo.git");
    let files = BTreeMap::from([(
        "skills/haiku-writer/SKILL.md".to_owned(),
        SKILL.as_bytes().to_vec(),
    )]);
    let commit = commit_files(&repository, "main", "one", &files).to_string();
    let server = TestGitServer::start(&repository, None).await;
    let file = write_spec(
        &dir,
        json!([{ "git": server.url("r"), "path": "skills", "rev": commit }]),
    );
    let path = file.to_str().unwrap();
    let output = grauwolf(&["tools", "lock", path, "--locked"], &dir);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("is not locked"));
    let output = grauwolf(&["tools", "lock", path], &dir);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(text(&output.stdout).contains("1 git skills source(s) locked"));
    let lock = std::fs::read_to_string(dir.join("grauwolf.lock")).unwrap();
    assert!(
        lock.contains("\"skills\"") && lock.contains(&commit),
        "{lock}"
    );
    server.stop().await;
    let output = grauwolf(&["tools", "lock", path, "--locked"], &dir);
    assert!(output.status.success(), "{}", text(&output.stderr));
}

#[test]
fn tools_lock_refuses_a_spec_with_nothing_to_lock() {
    let dir = work("nothing");
    std::fs::create_dir_all(dir.join("skills/haiku-writer")).unwrap();
    std::fs::write(dir.join("skills/haiku-writer/SKILL.md"), SKILL).unwrap();
    let file = write_spec(&dir, json!([{ "dir": "skills" }]));
    let output = grauwolf(&["tools", "lock", file.to_str().unwrap()], &dir);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("no `tool_packages` and no git `skills`"));
}

fn generate(dir: &Path, spec: &Path, out: &Path, dockerfile: bool) -> Output {
    let mut args = vec![
        "spec",
        "generate",
        spec.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ];
    if dockerfile {
        args.push("--dockerfile");
    }
    // Generating prepares the tool build, which needs the toolchain in the PATH.
    Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(&args)
        .current_dir(dir)
        .output()
        .unwrap()
}

#[test]
fn generate_dockerfile_copies_local_skills_into_the_image() {
    let dir = work("image-local");
    let out = dir.join("out");
    std::fs::create_dir_all(dir.join("skills/haiku-writer/references")).unwrap();
    std::fs::write(dir.join("skills/haiku-writer/SKILL.md"), SKILL).unwrap();
    std::fs::write(dir.join("skills/haiku-writer/references/sig.md"), "-- me").unwrap();
    std::fs::write(dir.join("unrelated.txt"), "no").unwrap();
    let file = write_spec(&dir, json!([{ "dir": "skills" }]));
    let output = generate(&dir, &file, &out, true);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        std::fs::read_to_string(out.join("app/skills/haiku-writer/references/sig.md")).unwrap(),
        "-- me"
    );
    assert!(out.join("app/skills/haiku-writer/SKILL.md").is_file());
    assert!(!out.join("app/unrelated.txt").exists());
    let lib = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    assert!(lib.contains("load_skill"), "{lib}");
}

#[tokio::test(flavor = "multi_thread")]
async fn generate_dockerfile_fetches_git_skills_and_the_crate_names_the_copies() {
    let dir = work("image-git");
    let out = dir.join("out");
    let repository = dir.join("repo.git");
    let files = BTreeMap::from([
        (
            "skills/haiku-writer/SKILL.md".to_owned(),
            SKILL.as_bytes().to_vec(),
        ),
        (
            "skills/haiku-writer/references/sig.md".to_owned(),
            b"-- git".to_vec(),
        ),
    ]);
    let commit = commit_files(&repository, "main", "one", &files).to_string();
    let server = TestGitServer::start(&repository, None).await;
    let url = server.url("r");
    let file = write_spec(
        &dir,
        json!([{ "git": url, "path": "skills", "rev": commit, "names": ["haiku-writer"] }]),
    );
    // Without --dockerfile there is nowhere to copy them to.
    let output = generate(&dir, &file, &out, false);
    assert!(!output.status.success());
    assert!(
        text(&output.stderr).contains("--dockerfile"),
        "{}",
        text(&output.stderr)
    );

    let output = generate(&dir, &file, &out, true);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        std::fs::read_to_string(out.join("app/skills-git/0/haiku-writer/references/sig.md"))
            .unwrap(),
        "-- git"
    );
    let lib = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    assert!(lib.contains("skills-git/0"), "{lib}");
    assert!(
        !lib.contains("127.0.0.1"),
        "the crate must not name the repository: {lib}"
    );
    // The lockfile next to the spec records the commit.
    assert!(
        std::fs::read_to_string(dir.join("grauwolf.lock"))
            .unwrap()
            .contains(&commit)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn generate_dockerfile_localizes_the_git_skills_of_sub_agents() {
    let dir = work("image-sub");
    let out = dir.join("out");
    let repository = dir.join("repo.git");
    let files = BTreeMap::from([(
        "haiku-writer/SKILL.md".to_owned(),
        SKILL.as_bytes().to_vec(),
    )]);
    let commit = commit_files(&repository, "main", "one", &files).to_string();
    let server = TestGitServer::start(&repository, None).await;
    std::fs::create_dir_all(dir.join("helper")).unwrap();
    let graph = json!({
        "nodes": [{ "id": "call_llm", "kind": "llm_request" }],
        "edges": [
            { "from": "start", "to": "call_llm" },
            { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } }
        ]
    });
    std::fs::write(
        dir.join("helper/helper.json"),
        json!({
            "spec_version": 1, "name": "helper",
            "model": { "provider": "ollama", "id": "m" },
            "skills": [{ "git": server.url("r"), "rev": commit }],
            "graph": graph
        })
        .to_string(),
    )
    .unwrap();
    let file = dir.join("agent.json");
    std::fs::write(
        &file,
        json!({
            "spec_version": 1, "name": "boss",
            "model": { "provider": "ollama", "id": "m" },
            "agent_tools": { "ask_helper": { "spec": "helper/helper.json", "description": "Helps" } },
            "graph": graph
        })
        .to_string(),
    )
    .unwrap();
    let output = generate(&dir, &file, &out, true);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        out.join("app/helper/skills-git/0/haiku-writer/SKILL.md")
            .is_file()
    );
    let patched: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("app/helper/helper.json")).unwrap())
            .unwrap();
    assert_eq!(patched["skills"], json!([{ "dir": "skills-git/0" }]));
    assert_eq!(patched["name"], "helper");
}
