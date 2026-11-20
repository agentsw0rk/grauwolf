//! Jira, Confluence and git tools from the spec (ADR-0104): credentials before the run.

use grauwolf_spec::AgentSpecExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use grauwolf_host::config::MapEnv;
use grauwolf_spec::{AgentSpec, BuiltinToolsSpec};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
mod support;
use support::tools_with_env;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn builtin(value: Value) -> BuiltinToolsSpec {
    serde_json::from_value(value).unwrap()
}

fn env(pairs: &[(&str, &str)]) -> MapEnv {
    MapEnv::new(pairs.iter().copied())
}

fn names(tools: &[Arc<dyn grauwolf_tools::DynTool>]) -> Vec<String> {
    tools.iter().map(|tool| tool.name().to_owned()).collect()
}

#[tokio::test]
async fn provides_jira_and_confluence_tools_with_the_credentials_of_the_environment() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/issue/ABC-1"))
        .and(header("authorization", "Basic YUBiLmM6dG9r"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .expect(1)
        .mount(&server)
        .await;
    let spec = builtin(json!({
        "tools": ["jira_get_issue", "confluence_get_page", "say_to_user"],
        "jira": { "base_url": server.uri() },
        "confluence": { "base_url": server.uri() }
    }));

    let tools = tools_with_env(
        &spec,
        Path::new("."),
        env(&[("ATLASSIAN_EMAIL", "a@b.c"), ("ATLASSIAN_API_TOKEN", "tok")]),
    )
    .unwrap();
    let issue = tools.iter().find(|t| t.name() == "jira_get_issue").unwrap();
    let result = issue
        .call_json(json!({ "issue_key": "ABC-1" }))
        .await
        .unwrap();

    assert_eq!(
        names(&tools),
        ["say_to_user", "jira_get_issue", "confluence_get_page"]
    );
    assert_eq!(result, "HTTP 200\n{}");
}

#[test]
fn missing_atlassian_credentials_fail_before_the_run_with_a_clear_message() {
    let spec = builtin(json!({
        "tools": ["jira_get_issue"],
        "jira": { "base_url": "https://example.atlassian.net" }
    }));

    let no_token = tools_with_env(&spec, Path::new("."), env(&[("ATLASSIAN_EMAIL", "a@b.c")]));
    let nothing = tools_with_env(&spec, Path::new("."), env(&[]));

    let message = |result: Result<Vec<_>, String>| result.err().unwrap();
    assert!(message(no_token).contains("ATLASSIAN_API_TOKEN is not set"));
    let nothing = message(nothing);
    assert!(
        nothing.starts_with(
            "the Jira tools need Atlassian Cloud credentials: ATLASSIAN_EMAIL is not set"
        ),
        "{nothing}"
    );
}

/// A directory of one test that is removed again.
///
/// It is unique per process: the same fixed directory shared by two test processes at once
/// (a second `cargo test` of the crate, a loop over the binary) let one process delete the
/// directory in which the other ran `git`, which failed with "unable to get current working
/// directory", "cannot mkdir remote.git: File exists" and the like.
struct TestDir(PathBuf);

impl std::ops::Deref for TestDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A bare repository with one commit on `main`; the tools take its path as the URL.
fn remote(name: &str) -> (TestDir, PathBuf) {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/facade-git-tests")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = TestDir(base.canonicalize().unwrap());
    let run = |cwd: &Path, args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "s")
            .env("GIT_AUTHOR_EMAIL", "s@s")
            .env("GIT_COMMITTER_NAME", "s")
            .env("GIT_COMMITTER_EMAIL", "s@s")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    run(&base, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
    run(&base, &["init", "-q", "-b", "main", "seed"]);
    std::fs::write(base.join("seed/README.md"), "hi\n").unwrap();
    run(&base.join("seed"), &["add", "."]);
    run(&base.join("seed"), &["commit", "-q", "-m", "first"]);
    run(
        &base.join("seed"),
        &[
            "push",
            "-q",
            &base.join("remote.git").display().to_string(),
            "main",
        ],
    );
    let remote = base.join("remote.git");
    (base, remote)
}

#[tokio::test]
async fn provides_git_tools_for_the_directory_below_the_root() {
    let (base, remote) = remote("provides");
    let spec = builtin(json!({
        "tools": ["git_checkout", "git_list_branches"],
        "root": "work",
        "git": { "repo_url": remote.display().to_string(), "directory": "clone" }
    }));
    std::fs::create_dir_all(base.join("spec/work")).unwrap();

    let tools = tools_with_env(&spec, &base.join("spec"), env(&[])).unwrap();
    let checkout = tools.iter().find(|t| t.name() == "git_checkout").unwrap();
    let result = checkout.call_json(json!({ "ref": "main" })).await.unwrap();

    assert_eq!(names(&tools), ["git_checkout", "git_list_branches"]);
    assert!(result.starts_with("On branch main at "), "{result}");
    assert!(base.join("spec/work/clone/README.md").exists());
}

#[test]
fn use_token_needs_git_token_before_the_run() {
    let spec = builtin(json!({
        "tools": ["git_checkout"],
        "git": { "repo_url": "https://github.com/org/repo.git", "use_token": true }
    }));

    let error = tools_with_env(&spec, Path::new("."), env(&[]))
        .err()
        .unwrap();
    let with_token = tools_with_env(&spec, Path::new("."), env(&[("GIT_TOKEN", "t")]));

    assert!(error.contains("GIT_TOKEN is not set"), "{error}");
    assert!(with_token.is_ok());
}

#[test]
fn a_git_directory_that_leads_out_through_a_symlink_is_refused() {
    let (base, remote) = remote("symlink");
    std::fs::create_dir_all(base.join("outside")).unwrap();
    std::fs::create_dir_all(base.join("spec")).unwrap();
    std::os::unix::fs::symlink(base.join("outside"), base.join("spec/repo")).unwrap();
    let spec = builtin(json!({
        "tools": ["git_checkout"],
        "git": { "repo_url": remote.display().to_string() }
    }));

    let error = tools_with_env(&spec, &base.join("spec"), env(&[]))
        .err()
        .unwrap();

    assert!(error.contains("leads outside the root"), "{error}");
}

#[tokio::test]
async fn the_facade_provides_these_tools_with_the_external_tools_of_a_spec() {
    let (base, remote) = remote("external");
    std::fs::create_dir_all(base.join("spec")).unwrap();
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "coder",
            "model": { "provider": "ollama", "id": "m" },
            "builtin_tools": {
                "tools": ["git_list_branches"],
                "git": { "repo_url": remote.display().to_string() }
            },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    let tools = support::tools(&spec, &base.join("spec")).await.unwrap();
    let listing = tools[0].call_json(json!({})).await.unwrap();

    assert_eq!(listing, "branches: main\ntags: ");
}
