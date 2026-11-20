//! `grauwolf tools pull|inspect` for git tool packages (ADR-0107), from the outside against the
//! in-process test git server on 127.0.0.1. The tests that do not build run without `cargo` in
//! the `PATH`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use grauwolf_git::testing::{TestGitServer, TestSigner, commit_files, commit_files_signed};

const MANIFEST: &str =
    include_str!("../../../../docs/user-guide/examples/03-package/grauwolf-tools.json");
const ADD: &str = include_str!("../../../../docs/user-guide/examples/03-package/src/add.rs");
const COUNT: &str =
    include_str!("../../../../docs/user-guide/examples/03-package/src/count_words.rs");

fn work(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("gitcli-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn package_files() -> BTreeMap<String, Vec<u8>> {
    [
        ("grauwolf-tools.json", MANIFEST),
        ("src/add.rs", ADD),
        ("src/count_words.rs", COUNT),
    ]
    .into_iter()
    .map(|(name, text)| (name.to_owned(), text.as_bytes().to_vec()))
    .collect()
}

/// Runs the binary with an empty `PATH` (no toolchain) and the extra environment.
fn grauwolf(args: &[&str], dir: &Path, env: &[(&str, &str)]) -> Output {
    let empty = dir.join("no-tools");
    std::fs::create_dir_all(&empty).unwrap();
    Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(args)
        .env("PATH", &empty)
        .envs(env.iter().copied())
        .current_dir(dir)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A repository with the example package on branch `main`, signed by `signer` if given.
fn repository(dir: &Path, signer: Option<&TestSigner>) -> PathBuf {
    let path = dir.join("repo.git");
    match signer {
        Some(signer) => {
            commit_files_signed(&path, "main", "one", &package_files(), signer);
        }
        None => {
            commit_files(&path, "main", "one", &package_files());
        }
    }
    path
}

fn signers_file(dir: &Path, signer: &TestSigner) -> PathBuf {
    let file = dir.join("signers");
    std::fs::write(&file, signer.allowed_signers("test@example.com", None)).unwrap();
    file
}

#[tokio::test(flavor = "multi_thread")]
async fn inspect_shows_the_package_and_the_signer_without_building() {
    let dir = work("inspect-signed");
    let signer = TestSigner::new();
    let server = TestGitServer::start(&repository(&dir, Some(&signer)), None).await;
    let signers = signers_file(&dir, &signer);
    let reference = format!("git+{}@branch:main", server.url("r"));

    let output = grauwolf(
        &[
            "tools",
            "inspect",
            &reference,
            "--cache",
            "cache",
            "--allowed-signers",
            signers.to_str().unwrap(),
        ],
        &dir,
        &[],
    );
    let out = text(&output.stdout);
    assert!(output.status.success(), "{output:?}");
    for expected in [
        reference.as_str(),
        "commit:  ",
        &format!(
            "signature: signed by test@example.com ({})",
            signer.fingerprint()
        ),
        "package: text-and-math 1.0.0",
        "abi:     1",
        "cache:   not cached",
        "add: ",
        "count_words: ",
    ] {
        assert!(out.contains(expected), "{expected} missing in {out}");
    }
    assert!(
        !dir.join("cache/packages").exists(),
        "inspect stores nothing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn inspect_names_a_missing_a_foreign_and_an_unchecked_signature() {
    let dir = work("inspect-status");
    let stranger = TestSigner::new();
    let signed = TestGitServer::start(&repository(&dir, Some(&stranger)), None).await;
    let plain_dir = dir.join("plain");
    std::fs::create_dir_all(&plain_dir).unwrap();
    let plain = TestGitServer::start(&repository(&plain_dir, None), None).await;
    let trusted = signers_file(&dir, &TestSigner::new());
    let trusted = trusted.to_str().unwrap();
    let status = |server: &TestGitServer, extra: &[&str]| {
        let reference = format!("git+{}@branch:main", server.url("r"));
        let mut args = vec!["tools", "inspect", &reference, "--cache", "cache"];
        args.extend_from_slice(extra);
        let output = grauwolf(&args, &dir, &[]);
        assert!(output.status.success(), "{output:?}");
        text(&output.stdout)
    };
    let allowed = ["--allowed-signers", trusted];
    assert!(status(&plain, &allowed).contains("signature: unsigned"));
    let foreign = status(&signed, &allowed);
    assert!(
        foreign.contains("signature: signature invalid: "),
        "{foreign}"
    );
    assert!(foreign.contains("not an allowed signer"), "{foreign}");
    assert!(status(&signed, &[]).contains("signature: signed, not checked"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_policy_without_the_host_refuses_pull_and_inspect() {
    let dir = work("host");
    let server = TestGitServer::start(&repository(&dir, None), None).await;
    let policy = dir.join("policy.toml");
    std::fs::write(
        &policy,
        "[tool_packages]\nallowed_git_hosts = [\"github.com\"]\n",
    )
    .unwrap();
    let env = [("GRAUWOLF_TOOL_POLICY", policy.to_str().unwrap())];
    let reference = format!("git+{}@branch:main", server.url("r"));
    for command in ["pull", "inspect"] {
        let output = grauwolf(
            &["tools", command, &reference, "--cache", "cache"],
            &dir,
            &env,
        );
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(
            text(&output.stderr).contains("allowed_git_hosts"),
            "{output:?}"
        );
    }
    assert!(server.requests().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn pull_refuses_an_unsigned_commit_when_the_policy_requires_signatures() {
    let dir = work("pull-unsigned");
    let server = TestGitServer::start(&repository(&dir, None), None).await;
    let signers = signers_file(&dir, &TestSigner::new());
    let policy = dir.join("policy.toml");
    std::fs::write(
        &policy,
        format!(
            "[tool_packages]\nrequire_signatures = true\ngit_allowed_signers = \"{}\"\n",
            signers.display()
        ),
    )
    .unwrap();
    let reference = format!("git+{}@branch:main", server.url("r"));
    let output = grauwolf(
        &["tools", "pull", &reference, "--cache", "cache"],
        &dir,
        &[("GRAUWOLF_TOOL_POLICY", policy.to_str().unwrap())],
    );
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let err = text(&output.stderr);
    assert!(
        err.contains("require_signatures") && err.contains("unsigned"),
        "{err}"
    );
    assert!(!dir.join("cache/packages").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn errors_never_contain_the_git_token() {
    let dir = work("token");
    let server = TestGitServer::start(&repository(&dir, None), Some("right-token")).await;
    let reference = format!("git+{}@branch:main", server.url("r"));
    for command in ["pull", "inspect"] {
        let output = grauwolf(
            &["tools", command, &reference, "--cache", "cache"],
            &dir,
            &[("GIT_TOKEN", "wrong-token-value")],
        );
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let all = format!("{}{}", text(&output.stdout), text(&output.stderr));
        assert!(
            !all.contains("wrong-token-value") && !all.contains("right-token"),
            "{all}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reference_without_a_revision_is_refused_with_an_example() {
    let dir = work("no-revision");
    for command in ["pull", "inspect"] {
        let output = grauwolf(
            &["tools", command, "git+https://h.example/r.git"],
            &dir,
            &[],
        );
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(text(&output.stderr).contains("@tag:v1"), "{output:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: builds the tools of the package with cargo"]
async fn pull_builds_and_caches_and_a_later_lock_needs_neither_network_nor_toolchain() {
    let dir = work("pull-build");
    let path = repository(&dir, None);
    let server = TestGitServer::start(&path, None).await;
    let url = server.url("r");
    let reference = format!("git+{url}@branch:main");
    let cargo = which_cargo();
    let path_env = format!("{}:{}", cargo.display(), std::env::var("PATH").unwrap());
    let pulled = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["tools", "pull", &reference, "--cache", "cache"])
        .env("PATH", path_env)
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(pulled.status.success(), "{pulled:?}");
    let out = text(&pulled.stdout);
    let commit = out
        .split(" -> commit ")
        .nth(1)
        .and_then(|rest| rest.get(..40))
        .unwrap()
        .to_owned();
    assert!(
        out.contains("add: ") && out.contains("count_words: "),
        "{out}"
    );
    server.stop().await;

    let spec = dir.join("spec.json");
    std::fs::write(
        &spec,
        serde_json::json!({
            "spec_version": 1, "name": "a",
            "model": { "provider": "ollama", "id": "m" },
            "tool_packages": [{ "git": url, "rev": commit }],
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let locked = grauwolf(
        &["tools", "lock", spec.to_str().unwrap(), "--cache", "cache"],
        &dir,
        &[],
    );
    assert!(locked.status.success(), "{locked:?}");
}

fn which_cargo() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap()).join(".cargo/bin")
}
