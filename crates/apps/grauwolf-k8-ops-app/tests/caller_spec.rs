//! The check of a spec that calls other agents of the cluster (ADR-0109 Q8): the warnings of
//! `agent deploy`, and the lines and warnings of `doctor` about callers, NetworkPolicies and
//! callers' specs.

mod common;

use std::path::PathBuf;

use common::{Env, Run, env};
use grauwolf_k8s::testing::DeploymentOutcome;
use grauwolf_test::TestDir;
use serde_json::{Value, json};

const IMAGE: &str = "registry.example.com/grauwolf:1.0";
const REVIEWER_ID: &str = "spiffe://agentswork.de/ns/reviewer/sa/reviewer";
const COORDINATOR_ID: &str = "spiffe://agentswork.de/ns/coordinator/sa/coordinator";
const CALLERS: &str = "/api/v1/namespaces/reviewer/configmaps/grauwolf-callers";
const POLICY: &str =
    "/apis/networking.k8s.io/v1/namespaces/reviewer/networkpolicies/grauwolf-mtls-callers";

async fn two_agents() -> Env {
    let env = env();
    env.prepare().await;
    for name in ["reviewer", "coordinator"] {
        assert_eq!(env.run(&["agent", "add", name], "").await.code, 0);
    }
    env.fake.set_deployment_outcome(DeploymentOutcome::Ready(
        "listening on 0.0.0.0:8080\n".into(),
    ));
    env
}

/// The spec of `coordinator` with `entry` as its `agent_tools.reviewer`.
fn coordinator_spec(dir: &TestDir, section: &str, entry: Value) -> PathBuf {
    let file = dir.join("coordinator.json");
    std::fs::write(
        &file,
        json!({"spec_version": 1, "name": "coordinator",
            "model": {"provider": "ollama", "id": "qwen3.5:9b"},
            "system_prompt": "You coordinate.",
            section: {"reviewer": entry},
            "strategy": {"kind": "single_run"}})
        .to_string(),
    )
    .unwrap();
    file
}

fn good_entry() -> Value {
    json!({"url": "https://reviewer.reviewer.svc:8443", "description": "Reviews",
        "auth": {"spiffe": {"id": REVIEWER_ID}}})
}

async fn deploy_coordinator(env: &Env, section: &str, entry: Value) -> Run {
    let dir = TestDir::new("caller-spec");
    let file = coordinator_spec(&dir, section, entry);
    let run = env
        .run(
            &[
                "agent",
                "deploy",
                "coordinator",
                "--image",
                IMAGE,
                "--spec",
                &file.display().to_string(),
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    run
}

fn warnings(run: &Run) -> Vec<&str> {
    run.err
        .lines()
        .filter(|line| line.starts_with("warning: "))
        .collect()
}

#[tokio::test]
async fn a_spec_that_matches_an_allowed_agent_draws_no_warning() {
    let env = two_agents().await;
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let run = deploy_coordinator(&env, "agent_tools", good_entry()).await;
    assert!(warnings(&run).is_empty(), "{}", run.err);
}

#[tokio::test]
async fn deploy_warns_when_the_entry_has_no_auth() {
    let env = two_agents().await;
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let run = deploy_coordinator(
        &env,
        "agent_tools",
        json!({"url": "https://reviewer.reviewer.svc:8443", "description": "Reviews"}),
    )
    .await;
    let found = warnings(&run);
    assert_eq!(found.len(), 1, "{}", run.err);
    assert!(
        found[0].contains("coordinator: agent_tools.reviewer (agent reviewer) has no auth.spiffe"),
        "{}",
        found[0]
    );
    assert!(
        found[0].contains(REVIEWER_ID),
        "the hint names the ID: {}",
        found[0]
    );
}

#[tokio::test]
async fn deploy_warns_when_the_id_is_not_the_targets() {
    let env = two_agents().await;
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let wrong = "spiffe://agentswork.de/ns/agents/sa/reviewer";
    let run = deploy_coordinator(
        &env,
        "agent_tools",
        json!({"url": "https://reviewer.reviewer.svc:8443", "description": "Reviews",
            "auth": {"spiffe": {"id": wrong}}}),
    )
    .await;
    let found = warnings(&run);
    assert_eq!(found.len(), 1, "{}", run.err);
    assert!(
        found[0].contains(&format!("is {wrong}, but agent reviewer is {REVIEWER_ID}")),
        "{}",
        found[0]
    );
}

#[tokio::test]
async fn deploy_warns_about_http_and_about_a_port_that_is_not_the_mtls_port() {
    let env = two_agents().await;
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    for url in [
        "http://reviewer.reviewer.svc:8443",
        "https://reviewer.reviewer.svc:8080",
        "https://reviewer.reviewer.svc",
    ] {
        let mut entry = good_entry();
        entry["url"] = json!(url);
        let run = deploy_coordinator(&env, "agent_tools", entry).await;
        let found = warnings(&run);
        assert_eq!(found.len(), 1, "{url}: {}", run.err);
        assert!(
            found[0].contains(&format!(
                "calls {url}, which is not the mTLS port of agent reviewer (https, port 8443)"
            )),
            "{}",
            found[0]
        );
        assert!(found[0].contains("use `https://reviewer.reviewer.svc:8443`"));
    }
}

#[tokio::test]
async fn deploy_warns_when_the_target_has_not_allowed_the_caller() {
    let env = two_agents().await;
    let run = deploy_coordinator(&env, "agent_tools", good_entry()).await;
    let found = warnings(&run);
    assert_eq!(found.len(), 1, "{}", run.err);
    assert!(
        found[0].contains(&format!(
            "calls agent reviewer, which has not allowed coordinator ({COORDINATOR_ID})"
        )),
        "{}",
        found[0]
    );
    assert!(
        found[0].contains("run `grauwolf-k8-ops agent allow reviewer --from coordinator`"),
        "{}",
        found[0]
    );
    // Allowing it takes the warning away.
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let run = deploy_coordinator(&env, "agent_tools", good_entry()).await;
    assert!(warnings(&run).is_empty(), "{}", run.err);
}

#[tokio::test]
async fn mcp_servers_and_the_long_service_name_are_checked_too() {
    let env = two_agents().await;
    let run = deploy_coordinator(
        &env,
        "mcp_servers",
        json!({"url": "http://reviewer.reviewer.svc.cluster.local:8080/mcp", "tools": ["review"]}),
    )
    .await;
    let found = warnings(&run);
    assert_eq!(found.len(), 1, "{}", run.err);
    assert!(
        found[0].contains("coordinator: mcp_servers.reviewer (agent reviewer) has no auth.spiffe"),
        "{}",
        found[0]
    );
}

#[tokio::test]
async fn a_jwt_call_to_the_http_port_needs_no_mtls_port_but_still_the_permission() {
    let env = two_agents().await;
    let entry = json!({"url": "https://reviewer.reviewer.svc:8080", "description": "Reviews",
        "auth": {"spiffe": {"mode": "jwt", "audience": "reviewer", "id": REVIEWER_ID}}});
    let run = deploy_coordinator(&env, "agent_tools", entry).await;
    let found = warnings(&run);
    assert_eq!(found.len(), 1, "{}", run.err);
    assert!(
        found[0].contains("has not allowed coordinator"),
        "{}",
        found[0]
    );
}

#[tokio::test]
async fn urls_that_are_not_agents_of_the_cluster_are_left_alone() {
    let env = two_agents().await;
    for url in [
        "https://example.com/a2a",
        "https://ghost.ghost.svc:8443",
        "http://127.0.0.1:8080",
    ] {
        let run = deploy_coordinator(
            &env,
            "agent_tools",
            json!({"url": url, "description": "Elsewhere"}),
        )
        .await;
        assert!(warnings(&run).is_empty(), "{url}: {}", run.err);
    }
}

async fn doctor_lines(env: &Env) -> (i32, Vec<Value>) {
    let run = env.run(&["doctor", "--json"], "").await;
    (run.code, serde_json::from_str(&run.out).unwrap())
}

fn line<'a>(lines: &'a [Value], check: &str) -> Vec<&'a Value> {
    lines.iter().filter(|line| line["check"] == check).collect()
}

fn one<'a>(lines: &'a [Value], check: &str) -> &'a Value {
    let found = line(lines, check);
    assert_eq!(found.len(), 1, "{check}: {found:?}");
    found[0]
}

#[tokio::test]
async fn doctor_reports_callers_policy_and_a_spec_that_fits() {
    let env = two_agents().await;
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    deploy_coordinator(&env, "agent_tools", good_entry()).await;
    let (code, lines) = doctor_lines(&env).await;
    assert_eq!(code, 0, "{lines:?}");
    let callers = one(&lines, "agent reviewer: callers");
    assert_eq!(callers["status"], "ok");
    assert!(
        callers["reason"].as_str().unwrap().contains("1 caller(s)"),
        "{callers}"
    );
    let policy = one(&lines, "agent reviewer: NetworkPolicy");
    assert_eq!(policy["status"], "ok", "{policy}");
    assert!(
        policy["reason"]
            .as_str()
            .unwrap()
            .contains("matches the callers")
    );
    let spec = one(&lines, "agent coordinator: caller spec");
    assert_eq!(spec["status"], "ok", "{spec}");
    assert!(
        spec["reason"].as_str().unwrap().contains("calls reviewer"),
        "{spec}"
    );
}

#[tokio::test]
async fn doctor_warns_about_what_deploy_warns_about() {
    let env = two_agents().await;
    deploy_coordinator(
        &env,
        "agent_tools",
        json!({"url": "http://reviewer.reviewer.svc:8080", "description": "Reviews"}),
    )
    .await;
    let (_, lines) = doctor_lines(&env).await;
    let found = line(&lines, "agent coordinator: caller spec");
    assert_eq!(
        found.len(),
        1,
        "no auth, so no check of the permission: {found:?}"
    );
    assert_eq!(found[0]["status"], "warn");
    assert!(
        found[0]["reason"]
            .as_str()
            .unwrap()
            .contains("has no auth.spiffe")
    );

    // Wrong ID, wrong port, and not allowed: three lines.
    let mut entry = good_entry();
    entry["url"] = json!("https://reviewer.reviewer.svc:9999");
    entry["auth"]["spiffe"]["id"] = json!("spiffe://agentswork.de/ns/x/sa/reviewer");
    deploy_coordinator(&env, "agent_tools", entry).await;
    let (_, lines) = doctor_lines(&env).await;
    let found = line(&lines, "agent coordinator: caller spec");
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(found.iter().all(|line| line["status"] == "warn"));
    let text: String = found
        .iter()
        .map(|line| line["reason"].as_str().unwrap())
        .collect();
    for wanted in [
        "but agent reviewer is",
        "not the mTLS port",
        "has not allowed coordinator",
    ] {
        assert!(text.contains(wanted), "{wanted}: {text}");
    }
}

#[tokio::test]
async fn doctor_says_it_cannot_read_the_calls_of_an_image_deploy() {
    let env = two_agents().await;
    let run = env
        .run(&["agent", "deploy", "coordinator", "--image", IMAGE], "")
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    let (_, lines) = doctor_lines(&env).await;
    let spec = one(&lines, "agent coordinator: caller spec");
    assert!(
        spec["reason"]
            .as_str()
            .unwrap()
            .contains("not checked: the agent was deployed from an image without --spec"),
        "{spec}"
    );
}

#[tokio::test]
async fn doctor_fails_on_callers_that_do_not_parse_and_warns_on_a_missing_map() {
    let env = two_agents().await;
    env.run(&["agent", "deploy", "reviewer", "--image", IMAGE], "")
        .await;
    env.fake.insert(
        CALLERS,
        json!({"metadata": {"name": "grauwolf-callers"}, "data": {"callers.json": "{ not json"}}),
    );
    let (code, lines) = doctor_lines(&env).await;
    assert_eq!(code, 1);
    let callers = one(&lines, "agent reviewer: callers");
    assert_eq!(callers["status"], "fail");
    assert!(
        callers["reason"]
            .as_str()
            .unwrap()
            .contains("is not valid JSON")
    );

    env.fake.remove(CALLERS);
    let (code, lines) = doctor_lines(&env).await;
    assert_eq!(code, 0);
    let callers = one(&lines, "agent reviewer: callers");
    assert_eq!(callers["status"], "warn");
    assert!(callers["reason"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn doctor_warns_about_a_policy_that_differs_or_is_missing() {
    let env = two_agents().await;
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let mut policy = env.fake.object(POLICY).unwrap();
    policy["spec"]["ingress"] = json!([{"ports": [{"protocol": "TCP", "port": 8080}]}]);
    env.fake.insert(POLICY, policy);
    let (_, lines) = doctor_lines(&env).await;
    let found = one(&lines, "agent reviewer: NetworkPolicy");
    assert_eq!(found["status"], "warn");
    assert!(
        found["reason"].as_str().unwrap().contains("differs"),
        "{found}"
    );

    env.fake.remove(POLICY);
    let (_, lines) = doctor_lines(&env).await;
    let found = one(&lines, "agent reviewer: NetworkPolicy");
    assert_eq!(found["status"], "warn");
    assert!(
        found["reason"].as_str().unwrap().contains("not found"),
        "{found}"
    );
}

#[tokio::test]
async fn doctor_warns_about_entries_of_agents_that_are_gone_or_of_another_trust_domain() {
    let env = two_agents().await;
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let mut map = env.fake.object(CALLERS).unwrap();
    let stale = json!({
        COORDINATOR_ID: "coordinator",
        "spiffe://agentswork.de/ns/ghost/sa/ghost": "ghost",
        "spiffe://example.org/ns/coordinator/sa/coordinator": "old",
        "spiffe://agentswork.de/ns/batch/*": "batch",
    });
    map["data"]["callers.json"] = json!(stale.to_string());
    env.fake.insert(CALLERS, map);
    let (_, lines) = doctor_lines(&env).await;
    let found = line(&lines, "agent reviewer: callers");
    let warns: Vec<&str> = found
        .iter()
        .filter(|line| line["status"] == "warn")
        .map(|line| line["reason"].as_str().unwrap())
        .collect();
    assert_eq!(warns.len(), 2, "{found:?}");
    assert!(
        warns
            .iter()
            .any(|text| text.contains("ns/ghost/sa/ghost") && text.contains("no longer exists"))
    );
    assert!(
        warns
            .iter()
            .any(|text| text.contains("example.org") && text.contains("not agentswork.de"))
    );
}

#[tokio::test]
async fn doctor_says_honestly_that_it_cannot_know_whether_policies_are_enforced() {
    let env = two_agents().await;
    // No policy yet: nothing to warn about.
    let (_, lines) = doctor_lines(&env).await;
    assert!(line(&lines, "NetworkPolicy enforcement").is_empty());

    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let (code, lines) = doctor_lines(&env).await;
    assert_eq!(code, 0, "a warning does not fail doctor");
    let found = one(&lines, "NetworkPolicy enforcement");
    assert_eq!(found["status"], "warn");
    assert!(
        found["reason"]
            .as_str()
            .unwrap()
            .contains("does not say whether the cluster enforces NetworkPolicies"),
        "{found}"
    );
    let hint = found["hint"].as_str().unwrap();
    assert!(
        hint.contains("live check") && hint.contains("not allowed"),
        "{hint}"
    );
}

#[tokio::test]
async fn an_agent_that_was_only_added_has_no_callers_lines() {
    let env = two_agents().await;
    let (_, lines) = doctor_lines(&env).await;
    assert!(
        lines.iter().all(|line| {
            let check = line["check"].as_str().unwrap();
            !check.ends_with(": callers") && !check.ends_with(": NetworkPolicy")
        }),
        "{lines:?}"
    );
}
