//! Working without the root token (ADR-0120): the policy `grauwolf-k8-ops`, `openbao token` and
//! the root warning of `doctor`. The fake OpenBao enforces the policy path by path when a test
//! calls `restrict_to`, so every command is run with only what the policy gives.

mod common;

use clap::Parser;
use common::{Env, env, policy_allows};
use grauwolf_k8s::testing::{DeploymentOutcome, JobOutcome};
use serde_json::{Value, json};

const POLICY: &str = "grauwolf-k8-ops";

fn settings(extra: &[&str]) -> grauwolf_k8_ops_app::Settings {
    let mut argv = vec!["grauwolf-k8-ops"];
    argv.extend(extra);
    argv.push("doctor");
    grauwolf_k8_ops_app::Cli::try_parse_from(argv)
        .unwrap()
        .settings
}

#[tokio::test]
async fn setup_writes_the_policy_once() {
    let env = env();
    let run = env.run(&["openbao", "setup"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out.contains("OpenBao policy grauwolf-k8-ops") && run.out.contains("created"),
        "{}",
        run.out
    );
    assert_eq!(
        env.bao.lock().unwrap().policies[POLICY],
        grauwolf_k8_ops_app::openbao_policy(&settings(&[]))
    );

    let writes = env.bao_writes();
    let again = env.run(&["openbao", "setup"], "").await;
    assert_eq!(env.bao_writes(), writes, "{}", again.out);
}

#[test]
fn the_policy_follows_the_mounts_and_allows_exactly_what_the_tool_needs() {
    let custom = grauwolf_k8_ops_app::openbao_policy(&settings(&[
        "--jwt-mount",
        "oidc",
        "--kv-mount",
        "secret",
    ]));
    for path in [
        "sys/auth/oidc",
        "auth/oidc/config",
        "sys/mounts/secret",
        "auth/oidc/role/agent-*",
        "secret/data/agents/*",
        "secret/metadata/agents/*",
    ] {
        assert!(
            custom.contains(&format!("path \"{path}\"")),
            "{path}\n{custom}"
        );
    }
    assert!(
        !custom.contains("jwt") && !custom.contains("kv/"),
        "{custom}"
    );

    let policy = grauwolf_k8_ops_app::openbao_policy(&settings(&[]));
    assert!(
        !policy.contains("\"sys/*\"") && !policy.contains("path \"*\""),
        "{policy}"
    );
    let allowed = |method: &str, path: &str, list: bool| policy_allows(&policy, method, path, list);
    for (method, path, list) in [
        ("GET", "/v1/sys/auth", false),
        ("POST", "/v1/sys/auth/jwt", false),
        ("GET", "/v1/auth/jwt/config", false),
        ("POST", "/v1/auth/jwt/config", false),
        ("POST", "/v1/sys/mounts/kv", false),
        ("PUT", "/v1/sys/policies/acl/agent-reviewer", false),
        ("DELETE", "/v1/sys/policies/acl/agent-reviewer", false),
        ("GET", "/v1/sys/policies/acl/grauwolf-k8-ops", false),
        ("POST", "/v1/auth/jwt/role/agent-reviewer", false),
        ("GET", "/v1/auth/jwt/role/grauwolf-agents", false),
        ("POST", "/v1/kv/data/agents/reviewer/jira", false),
        ("GET", "/v1/kv/metadata/agents/reviewer", true),
        ("DELETE", "/v1/kv/metadata/agents/reviewer/jira", false),
        ("POST", "/v1/sys/capabilities-self", false),
        ("GET", "/v1/auth/token/lookup-self", false),
    ] {
        assert!(
            allowed(method, path, list),
            "{method} {path} must be allowed"
        );
    }
    for (method, path) in [
        ("PUT", "/v1/sys/policies/acl/grauwolf-k8-ops"),
        ("PUT", "/v1/sys/policies/acl/root"),
        ("PUT", "/v1/sys/policies/acl/default"),
        ("POST", "/v1/auth/jwt/role/other"),
        ("POST", "/v1/sys/auth/userpass"),
        ("POST", "/v1/sys/mounts/secret"),
        ("GET", "/v1/kv/data/other/x"),
        ("GET", "/v1/secret/data/agents/x"),
        ("POST", "/v1/auth/token/create"),
        ("PUT", "/v1/sys/seal"),
        ("GET", "/v1/sys/policies/acl/root"),
    ] {
        assert!(
            !allowed(method, path, false),
            "{method} {path} must be refused"
        );
    }
}

#[tokio::test]
async fn openbao_token_prints_only_the_token_on_stdout() {
    let env = env();
    env.run(&["openbao", "setup"], "").await;

    let run = env.run(&["openbao", "token"], "").await;

    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(run.out, "s.restricted-token\n");
    assert!(
        run.err.contains("grauwolf-k8-ops") && !run.err.contains("s.restricted"),
        "{}",
        run.err
    );
    let created = env.bao.lock().unwrap().created_tokens.clone();
    assert_eq!(created[0]["policies"], json!([POLICY]));
    assert_eq!(created[0]["ttl"], "8h");
    assert_eq!(created[0]["renewable"], false);

    let run = env
        .run(&["openbao", "token", "--ttl", "30m", "--renewable"], "")
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    let created = env.bao.lock().unwrap().created_tokens.clone();
    assert_eq!(created[1]["ttl"], "30m");
    assert_eq!(created[1]["renewable"], true);
}

#[tokio::test]
async fn openbao_token_refuses_a_bad_ttl_and_a_missing_policy_and_a_missing_token() {
    let env = env();
    let run = env.run(&["openbao", "token"], "").await;
    assert_eq!(run.code, 1);
    assert!(run.err.contains("openbao setup"), "{}", run.err);
    assert_eq!(run.out, "");

    env.run(&["openbao", "setup"], "").await;
    let run = env.run(&["openbao", "token", "--ttl", "soon"], "").await;
    assert_eq!(run.code, 1);
    assert!(run.err.contains("--ttl soon"), "{}", run.err);
    assert!(env.bao.lock().unwrap().created_tokens.is_empty());

    let mut none = common::env();
    none.token = None;
    let run = none.run(&["openbao", "token"], "").await;
    assert_eq!(run.code, 1);
    assert!(run.err.contains("OPENBAO_TOKEN"), "{}", run.err);
}

async fn check(env: &Env, args: &[&str], stdin: &str) -> String {
    let run = env.run(args, stdin).await;
    assert_eq!(run.code, 0, "{args:?}\n{}", run.all());
    run.out
}

/// Every command with a token that has only the policy `grauwolf-k8-ops`: nothing is refused.
#[tokio::test]
async fn every_command_works_with_the_restricted_token() {
    let env = env();
    env.prepare().await;
    env.fake
        .set_deployment_outcome(DeploymentOutcome::Ready("listening\n".into()));
    env.restrict_to(POLICY);

    check(&env, &["openbao", "setup"], "").await;
    check(&env, &["agent", "add", "reviewer"], "").await;
    check(&env, &["agent", "add", "writer"], "").await;
    check(
        &env,
        &[
            "secret",
            "set",
            "reviewer",
            "jira",
            "--variable",
            "ATLASSIAN_API_TOKEN",
        ],
        "v\n",
    )
    .await;
    check(&env, &["secret", "list", "reviewer"], "").await;
    assert_eq!(
        check(&env, &["secret", "get", "reviewer", "jira", "--reveal"], "").await,
        "v\n"
    );
    env.fake.set_job_outcome(JobOutcome::Succeeds(
        "resolved ATLASSIAN_API_TOKEN from openbao (1 bytes)\n".into(),
    ));
    check(
        &env,
        &["agent", "check", "reviewer", "--image", "img:1"],
        "",
    )
    .await;
    check(
        &env,
        &["agent", "deploy", "reviewer", "--image", "img:1"],
        "",
    )
    .await;
    check(&env, &["agent", "status", "reviewer"], "").await;
    check(
        &env,
        &["agent", "allow", "reviewer", "--from", "writer"],
        "",
    )
    .await;

    let doctor = env.run(&["doctor", "--json"], "").await;
    let checks: Vec<Value> = serde_json::from_str(&doctor.out).unwrap();
    let bad: Vec<_> = checks.iter().filter(|c| c["status"] == "fail").collect();
    assert!(bad.is_empty() && doctor.code == 0, "{bad:#?}");
    let token = checks
        .iter()
        .find(|c| c["check"] == "OpenBao token policy")
        .unwrap();
    assert_eq!(token["status"], "ok", "{token}");

    check(&env, &["secret", "delete", "reviewer", "jira"], "").await;
    check(&env, &["agent", "undeploy", "reviewer"], "").await;
    check(
        &env,
        &["agent", "remove", "reviewer", "--purge-secrets", "--yes"],
        "",
    )
    .await;

    let denied = env.bao.lock().unwrap().denied.clone();
    assert!(denied.is_empty(), "refused by the policy: {denied:#?}");
}

/// The fake really enforces: with a policy that lacks the secrets, reading them fails.
#[tokio::test]
async fn the_fake_refuses_what_the_policy_does_not_give() {
    let env = env();
    env.prepare().await;
    env.run(&["agent", "add", "reviewer"], "").await;
    env.bao.lock().unwrap().policies.insert(
        "small".into(),
        "path \"sys/health\" {\n  capabilities = [\"read\"]\n}\n".into(),
    );
    env.restrict_to("small");

    let run = env.run(&["secret", "list", "reviewer"], "").await;

    assert_eq!(run.code, 1, "{}", run.all());
    assert!(run.err.contains("permission denied"), "{}", run.err);
    assert!(!env.bao.lock().unwrap().denied.is_empty());
}

async fn policy_check(env: &Env) -> Value {
    let run = env.run(&["doctor", "--json"], "").await;
    let checks: Vec<Value> = serde_json::from_str(&run.out).unwrap();
    checks
        .into_iter()
        .find(|check| check["check"] == "OpenBao token policy")
        .expect("the check exists")
}

#[tokio::test]
async fn doctor_warns_about_the_root_token_and_is_content_with_the_restricted_one() {
    let env = env();
    env.prepare().await;

    let root = policy_check(&env).await;
    assert_eq!(root["status"], "warn");
    assert_eq!(root["reason"], "working with the root token");
    assert!(
        root["hint"]
            .as_str()
            .unwrap()
            .contains("grauwolf-k8-ops openbao token"),
        "{root}"
    );

    env.restrict_to(POLICY);
    let restricted = policy_check(&env).await;
    assert_eq!(restricted["status"], "ok", "{restricted}");
    assert!(restricted["reason"].as_str().unwrap().contains(POLICY));
}
