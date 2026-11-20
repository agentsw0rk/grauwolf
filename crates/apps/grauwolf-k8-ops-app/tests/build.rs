//! `agent build --harness` and the build Secrets (ADR-0129 B4): the Kaniko build of a harness's
//! image in the cluster, without the studio, against a fake API server and real git repositories.

mod common;

use common::{Env, Run, env};
use grauwolf_k8s::testing::JobOutcome;
use grauwolf_test::TestDir;
use serde_json::{Value, json};

const BUILDS: &str = "/api/v1/namespaces/grauwolf-builds";

/// A repository with `origin`, one commit of `build/reviewer/Dockerfile` on `main`, and
/// `origin/main` at that commit (as after a push). Returns the commit.
fn pushed(dir: &TestDir) -> String {
    let repository = git2::Repository::init(&**dir).unwrap();
    repository
        .remote("origin", "https://github.com/agentsw0rk/agents.git")
        .unwrap();
    std::fs::create_dir_all(dir.join("build/reviewer")).unwrap();
    std::fs::write(dir.join("build/reviewer/Dockerfile"), "FROM scratch\n").unwrap();
    let mut index = repository.index().unwrap();
    index
        .add_path(std::path::Path::new("build/reviewer/Dockerfile"))
        .unwrap();
    index.write().unwrap();
    let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
    let signature = git2::Signature::now("t", "t@example.com").unwrap();
    let commit = repository
        .commit(
            Some("refs/heads/main"),
            &signature,
            &signature,
            "first",
            &tree,
            &[],
        )
        .unwrap();
    repository.set_head("refs/heads/main").unwrap();
    repository
        .reference("refs/remotes/origin/main", commit, true, "pushed")
        .unwrap();
    commit.to_string()
}

fn write_harness(dir: &TestDir, fields: Value) -> String {
    let mut harness = json!({"version": 1, "name": "reviewer", "spec": "reviewer.json"});
    harness
        .as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    let file = dir.join("reviewer.harness.json");
    std::fs::write(&file, harness.to_string()).unwrap();
    file.display().to_string()
}

/// A pushed project and a harness that names its image.
fn project(name: &str) -> (TestDir, String, String, String) {
    let dir = TestDir::new(name);
    let commit = pushed(&dir);
    let harness = write_harness(&dir, json!({"image": "ghcr.io/agentsw0rk/reviewer:0.1.0"}));
    let folder = dir.join("build/reviewer").display().to_string();
    (dir, harness, folder, commit)
}

async fn build(env: &Env, harness: &str, folder: &str, extra: &[&str]) -> Run {
    let mut args = vec!["agent", "build", "--harness", harness, "--project", folder];
    args.extend(extra);
    env.run(&args, "").await
}

fn kaniko_args(env: &Env) -> Vec<String> {
    let jobs = env.fake.created_jobs();
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    jobs[0]["spec"]["template"]["spec"]["containers"][0]["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn build_runs_kaniko_for_the_pushed_commit_and_prints_the_log_and_the_outcome() {
    let env = env();
    env.fake.set_job_outcome(JobOutcome::Succeeds(
        "INFO Cloning\nINFO Pushed ghcr.io/agentsw0rk/reviewer:0.1.0\n".into(),
    ));
    let (_dir, harness, folder, commit) = project("build-ok");

    let run = build(&env, &harness, &folder, &[]).await;

    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out
            .contains("INFO Cloning\nINFO Pushed ghcr.io/agentsw0rk/reviewer:0.1.0\n"),
        "{}",
        run.out
    );
    assert!(
        run.out
            .contains("built and pushed ghcr.io/agentsw0rk/reviewer:0.1.0"),
        "{}",
        run.out
    );
    assert_eq!(
        kaniko_args(&env),
        vec![
            format!("--context=git://github.com/agentsw0rk/agents.git#refs/heads/main#{commit}"),
            "--context-sub-path=build/reviewer".to_owned(),
            "--dockerfile=Dockerfile".to_owned(),
            "--destination=ghcr.io/agentsw0rk/reviewer:0.1.0".to_owned(),
        ]
    );
    let namespace = env.fake.object(BUILDS).expect("namespace made");
    assert_eq!(
        namespace["metadata"]["labels"]["app.kubernetes.io/managed-by"],
        "grauwolf-k8-ops"
    );
    assert!(
        namespace["metadata"]["labels"]
            .get("pod-security.kubernetes.io/enforce")
            .is_none(),
        "Kaniko runs as root: {namespace}"
    );
}

#[tokio::test]
async fn a_failed_build_exits_with_1_after_its_log() {
    let env = env();
    env.fake
        .set_job_outcome(JobOutcome::Fails("ERROR denied: push\n".into()));
    let (_dir, harness, folder, _) = project("build-failed");

    let run = build(&env, &harness, &folder, &[]).await;

    assert_eq!(run.code, 1, "{}", run.all());
    assert!(run.out.contains("ERROR denied: push\n"), "{}", run.out);
    assert!(run.out.contains("failed"), "{}", run.out);
}

#[tokio::test]
async fn a_build_that_is_not_finished_in_time_exits_with_1_and_the_job_is_gone() {
    let env = env();
    let (_dir, harness, folder, _) = project("build-timeout");

    let run = build(&env, &harness, &folder, &["--timeout", "0"]).await;

    assert_eq!(run.code, 1, "{}", run.all());
    assert!(
        run.out.contains("not finished within 0 seconds"),
        "{}",
        run.out
    );
    assert!(
        env.fake
            .deleted()
            .iter()
            .any(|path| path.contains("/jobs/reviewer-build-")),
        "{:?}",
        env.fake.deleted()
    );
}

#[tokio::test]
async fn uncommitted_changes_are_refused_before_the_cluster_is_touched() {
    let env = env();
    let (dir, harness, folder, _) = project("build-dirty");
    std::fs::write(
        dir.join("build/reviewer/Dockerfile"),
        "FROM scratch\nRUN x\n",
    )
    .unwrap();

    let run = build(&env, &harness, &folder, &[]).await;

    assert_eq!(run.code, 1, "{}", run.all());
    assert!(run.err.contains("not committed"), "{}", run.err);
    assert!(env.fake.writes().is_empty(), "{:?}", env.fake.writes());
}

#[tokio::test]
async fn without_an_image_or_a_registry_there_is_nothing_to_push() {
    let env = env();
    let (dir, _, folder, _) = project("build-no-image");
    let harness = write_harness(&dir, json!({}));

    let run = build(&env, &harness, &folder, &[]).await;

    assert_eq!(run.code, 1, "{}", run.all());
    assert!(run.err.contains("no image"), "{}", run.err);
    assert!(env.fake.writes().is_empty(), "{:?}", env.fake.writes());
}

#[tokio::test]
async fn without_an_image_the_default_one_of_the_registry_in_grauwolf_toml_is_pushed() {
    let env = env();
    env.fake
        .set_job_outcome(JobOutcome::Succeeds("INFO Pushed\n".into()));
    let (dir, _, folder, _) = project("build-default-image");
    std::fs::write(
        dir.join("grauwolf.toml"),
        "[harness]\nregistry = \"ghcr.io/agentsw0rk\"\n",
    )
    .unwrap();
    let harness = write_harness(&dir, json!({}));

    let run = build(&env, &harness, &folder, &[]).await;

    assert_eq!(run.code, 0, "{}", run.all());
    let destination = format!(
        "--destination=ghcr.io/agentsw0rk/reviewer:{}",
        env!("CARGO_PKG_VERSION")
    );
    assert!(
        kaniko_args(&env).contains(&destination),
        "{:?}",
        kaniko_args(&env)
    );
}

fn insert_secret(env: &Env, name: &str) {
    env.fake.insert(
        &format!("{BUILDS}/secrets/{name}"),
        json!({"kind": "Secret", "metadata": {"name": name, "namespace": "grauwolf-builds"}}),
    );
}

#[tokio::test]
async fn existing_secrets_of_the_build_namespace_are_used_for_the_registry_and_for_git() {
    let env = env();
    env.fake
        .set_job_outcome(JobOutcome::Succeeds("INFO Pushed\n".into()));
    insert_secret(&env, "ghcr");
    insert_secret(&env, "github");
    let (_dir, harness, folder, _) = project("build-secrets");

    let run = build(
        &env,
        &harness,
        &folder,
        &["--registry-secret", "ghcr", "--git-secret", "github"],
    )
    .await;

    assert_eq!(run.code, 0, "{}", run.all());
    let pod = &env.fake.created_jobs()[0]["spec"]["template"]["spec"];
    assert_eq!(pod["volumes"][0]["secret"]["secretName"], "ghcr");
    assert_eq!(
        pod["containers"][0]["env"][1]["valueFrom"]["secretKeyRef"]["name"],
        "github"
    );
}

#[tokio::test]
async fn a_secret_that_is_missing_in_the_build_namespace_is_an_error_naming_it() {
    let env = env();
    insert_secret(&env, "github");
    let (_dir, harness, folder, _) = project("build-missing-secret");

    let run = build(
        &env,
        &harness,
        &folder,
        &["--registry-secret", "ghcr", "--git-secret", "github"],
    )
    .await;

    assert_eq!(run.code, 1, "{}", run.all());
    assert!(
        run.err
            .contains("secret ghcr not found in namespace grauwolf-builds"),
        "{}",
        run.err
    );
    assert!(env.fake.created_jobs().is_empty());
}

#[tokio::test]
async fn build_secret_registry_stores_the_token_from_stdin_as_a_docker_config_and_never_prints_it()
{
    let env = env();

    let run = env
        .run(
            &[
                "build",
                "secret",
                "registry",
                "ghcr",
                "--server",
                "ghcr.io",
                "--username",
                "arek",
            ],
            "ghp_token-1\n",
        )
        .await;

    assert_eq!(run.code, 0, "{}", run.all());
    assert!(!run.all().contains("ghp_token-1"), "{}", run.all());
    assert!(
        run.out.contains("secret grauwolf-builds/ghcr: created"),
        "{}",
        run.out
    );
    let secret = env
        .fake
        .object(&format!("{BUILDS}/secrets/ghcr"))
        .expect("secret");
    assert_eq!(secret["type"], "kubernetes.io/dockerconfigjson");
    let config: Value =
        serde_json::from_str(secret["stringData"][".dockerconfigjson"].as_str().unwrap()).unwrap();
    assert_eq!(config["auths"]["ghcr.io"]["username"], "arek");
    assert_eq!(config["auths"]["ghcr.io"]["password"], "ghp_token-1");
    assert_eq!(
        env.fake.object(BUILDS).expect("namespace")["metadata"]["labels"]["app.kubernetes.io/managed-by"],
        "grauwolf-k8-ops"
    );
}

#[tokio::test]
async fn build_secret_git_stores_username_and_token_and_a_second_run_changes_nothing() {
    let env = env();
    let args = ["build", "secret", "git", "github", "--username", "arek"];

    let first = env.run(&args, "ghp_git-1").await;
    let second = env.run(&args, "ghp_git-1").await;

    assert_eq!(first.code, 0, "{}", first.all());
    assert!(
        first.out.contains("secret grauwolf-builds/github: created"),
        "{}",
        first.out
    );
    assert!(
        second
            .out
            .contains("secret grauwolf-builds/github: unchanged"),
        "{}",
        second.out
    );
    assert!(!(first.all() + &second.all()).contains("ghp_git-1"));
    let secret = env
        .fake
        .object(&format!("{BUILDS}/secrets/github"))
        .expect("secret");
    assert_eq!(
        secret["stringData"],
        json!({"username": "arek", "password": "ghp_git-1"})
    );
}
