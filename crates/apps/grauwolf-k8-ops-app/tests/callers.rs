//! `agent allow | deny | callers` (ADR-0109 Q7-Q9) against a fake Kubernetes API server and a fake
//! OpenBao.

mod common;

use common::{Env, env};
use serde_json::{Value, json};

const REVIEWER: &str = "/api/v1/namespaces/reviewer";
const POLICY: &str =
    "/apis/networking.k8s.io/v1/namespaces/reviewer/networkpolicies/grauwolf-mtls-callers";
const COORDINATOR_ID: &str = "spiffe://agentswork.de/ns/coordinator/sa/coordinator";

async fn two_agents() -> Env {
    let env = env();
    env.prepare().await;
    for name in ["reviewer", "coordinator"] {
        let run = env.run(&["agent", "add", name], "").await;
        assert_eq!(run.code, 0, "{}", run.all());
    }
    env
}

fn callers_of(env: &Env, namespace: &str) -> Value {
    let map = env
        .fake
        .object(&format!(
            "/api/v1/namespaces/{namespace}/configmaps/grauwolf-callers"
        ))
        .expect("ConfigMap grauwolf-callers");
    serde_json::from_str(map["data"]["callers.json"].as_str().unwrap()).unwrap()
}

fn policy(env: &Env) -> Value {
    env.fake.object(POLICY).expect("NetworkPolicy")
}

#[tokio::test]
async fn allow_maps_the_callers_spiffe_id_to_a_user_named_after_it() {
    let env = two_agents().await;
    let run = env
        .run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out.contains(&format!(
            "agent reviewer (namespace reviewer) allows {COORDINATOR_ID} as user coordinator: created"
        )),
        "{}",
        run.out
    );
    assert_eq!(
        callers_of(&env, "reviewer"),
        json!({COORDINATOR_ID: "coordinator"})
    );
    let map = env
        .fake
        .object(&format!("{REVIEWER}/configmaps/grauwolf-callers"))
        .unwrap();
    assert_eq!(
        map["metadata"]["labels"]["app.kubernetes.io/managed-by"],
        "grauwolf-k8-ops"
    );
}

#[tokio::test]
async fn allow_opens_the_mtls_port_for_the_callers_namespace_and_keeps_the_http_port_open() {
    let env = two_agents().await;
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let policy = policy(&env);
    assert_eq!(
        policy["spec"]["podSelector"]["matchLabels"],
        json!({"grauwolf.io/agent": "reviewer"})
    );
    assert_eq!(policy["spec"]["policyTypes"], json!(["Ingress"]));
    assert_eq!(
        policy["spec"]["ingress"],
        json!([
            // The HTTP port from anywhere: no `from`.
            {"ports": [{"protocol": "TCP", "port": 8080}]},
            {"from": [{"namespaceSelector": {"matchLabels":
                {"kubernetes.io/metadata.name": "coordinator"}}}],
             "ports": [{"protocol": "TCP", "port": 8443}]},
        ])
    );
}

#[tokio::test]
async fn allow_twice_is_unchanged_and_writes_nothing() {
    let env = two_agents().await;
    let args = ["agent", "allow", "reviewer", "--from", "coordinator"];
    env.run(&args, "").await;
    let writes = env.fake.writes().len();
    let again = env.run(&args, "").await;
    assert_eq!(again.code, 0, "{}", again.all());
    assert!(
        again.out.contains("as user coordinator: unchanged"),
        "{}",
        again.out
    );
    assert!(
        again
            .out
            .contains("ConfigMap reviewer/grauwolf-callers: unchanged")
    );
    assert!(
        again
            .out
            .contains("NetworkPolicy reviewer/grauwolf-mtls-callers")
            && again.out.contains("anywhere): unchanged"),
        "{}",
        again.out
    );
    assert_eq!(env.fake.writes().len(), writes);
}

#[tokio::test]
async fn allow_as_names_the_user_and_a_second_run_with_another_user_updates_it() {
    let env = two_agents().await;
    let run = env
        .run(
            &[
                "agent",
                "allow",
                "reviewer",
                "--from",
                "coordinator",
                "--as",
                "team-lead",
            ],
            "",
        )
        .await;
    assert!(
        run.out.contains("as user team-lead: created"),
        "{}",
        run.out
    );
    assert_eq!(
        callers_of(&env, "reviewer"),
        json!({COORDINATOR_ID: "team-lead"})
    );
    let run = env
        .run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    assert!(
        run.out.contains("as user coordinator: updated"),
        "{}",
        run.out
    );
    assert_eq!(
        callers_of(&env, "reviewer"),
        json!({COORDINATOR_ID: "coordinator"})
    );
}

#[tokio::test]
async fn allow_refuses_an_unknown_agent_on_either_side_and_writes_nothing() {
    let env = two_agents().await;
    let writes = env.fake.writes().len();
    for args in [
        ["agent", "allow", "ghost", "--from", "coordinator"],
        ["agent", "allow", "reviewer", "--from", "ghost"],
    ] {
        let run = env.run(&args, "").await;
        assert_eq!(run.code, 1, "{}", run.all());
        assert!(
            run.err.contains(
                "agent ghost not found (create it with `grauwolf-k8-ops agent add ghost`)"
            ),
            "{}",
            run.err
        );
    }
    assert_eq!(env.fake.writes().len(), writes);
    assert!(env.fake.object(POLICY).is_none());
}

#[tokio::test]
async fn allow_refuses_a_user_name_with_whitespace() {
    let env = two_agents().await;
    let run = env
        .run(
            &[
                "agent",
                "allow",
                "reviewer",
                "--from",
                "coordinator",
                "--as",
                "two words",
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 1);
    assert!(run.err.contains("is not a user name"), "{}", run.err);
}

#[tokio::test]
async fn several_callers_open_the_port_for_each_namespace_once() {
    let env = two_agents().await;
    env.run(&["agent", "add", "planner", "--namespace", "plans"], "")
        .await;
    for caller in ["planner", "coordinator"] {
        env.run(&["agent", "allow", "reviewer", "--from", caller], "")
            .await;
    }
    assert_eq!(
        callers_of(&env, "reviewer"),
        json!({COORDINATOR_ID: "coordinator",
            "spiffe://agentswork.de/ns/plans/sa/planner": "planner"})
    );
    let from = policy(&env)["spec"]["ingress"][1]["from"].clone();
    assert_eq!(
        from,
        json!([
            {"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "coordinator"}}},
            {"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "plans"}}},
        ])
    );
}

#[tokio::test]
async fn deny_removes_the_caller_from_the_map_and_the_policy() {
    let env = two_agents().await;
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let run = env
        .run(&["agent", "deny", "reviewer", "--from", "coordinator"], "")
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out.contains(&format!(
            "agent reviewer (namespace reviewer) no longer allows {COORDINATOR_ID}: removed"
        )),
        "{}",
        run.out
    );
    assert_eq!(callers_of(&env, "reviewer"), json!({}));
    // No callers: no rule for the mTLS port (a rule without `from` would admit everybody).
    assert_eq!(
        policy(&env)["spec"]["ingress"],
        json!([{"ports": [{"protocol": "TCP", "port": 8080}]}])
    );
    assert!(run.out.contains("from nobody"), "{}", run.out);
}

#[tokio::test]
async fn deny_twice_is_unchanged() {
    let env = two_agents().await;
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let args = ["agent", "deny", "reviewer", "--from", "coordinator"];
    env.run(&args, "").await;
    let writes = env.fake.writes().len();
    let again = env.run(&args, "").await;
    assert_eq!(again.code, 0, "{}", again.all());
    assert!(
        again.out.contains("does not allow coordinator: unchanged"),
        "{}",
        again.out
    );
    assert_eq!(env.fake.writes().len(), writes);
}

#[tokio::test]
async fn deny_without_any_callers_changes_nothing() {
    let env = two_agents().await;
    let writes = env.fake.writes().len();
    let run = env
        .run(&["agent", "deny", "reviewer", "--from", "coordinator"], "")
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(run.out.contains("has no callers: unchanged"), "{}", run.out);
    assert_eq!(env.fake.writes().len(), writes);
}

#[tokio::test]
async fn deny_still_removes_the_entry_of_an_agent_that_no_longer_exists() {
    let env = two_agents().await;
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    // The agent goes away without `agent remove`: the entry is dangling.
    env.fake.remove("/api/v1/namespaces/coordinator");
    let run = env
        .run(&["agent", "deny", "reviewer", "--from", "coordinator"], "")
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(callers_of(&env, "reviewer"), json!({}));
}

#[tokio::test]
async fn callers_lists_ids_users_and_whether_the_policy_admits_them() {
    let env = two_agents().await;
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let run = env.run(&["agent", "callers", "reviewer"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out
            .contains("agent reviewer (namespace reviewer): 1 caller(s)"),
        "{}",
        run.out
    );
    assert!(
        run.out.contains(&format!(
            "  {COORDINATOR_ID}  user coordinator  agent coordinator  network policy: admitted"
        )),
        "{}",
        run.out
    );
    assert!(
        run.out
            .contains("NetworkPolicy reviewer/grauwolf-mtls-callers: matches"),
        "{}",
        run.out
    );
}

#[tokio::test]
async fn callers_json_and_a_policy_that_was_changed_by_hand() {
    let env = two_agents().await;
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let mut tampered = policy(&env);
    tampered["spec"]["ingress"] = json!([{"ports": [{"protocol": "TCP", "port": 8080}]}]);
    env.fake.insert(POLICY, tampered);
    let run = env
        .run(&["agent", "callers", "reviewer", "--json"], "")
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    let value: Value = serde_json::from_str(run.out.trim()).unwrap();
    assert_eq!(value["agent"], "reviewer");
    assert_eq!(value["network_policy"], "differs");
    assert_eq!(
        value["callers"],
        json!([{"id": COORDINATOR_ID, "user": "coordinator", "agent": "coordinator",
            "network_policy": false}])
    );
    // Any allow or deny puts it right again.
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let run = env
        .run(&["agent", "callers", "reviewer", "--json"], "")
        .await;
    let value: Value = serde_json::from_str(run.out.trim()).unwrap();
    assert_eq!(value["network_policy"], "matches");
}

#[tokio::test]
async fn callers_of_an_agent_without_the_map_says_so() {
    let env = two_agents().await;
    let run = env.run(&["agent", "callers", "reviewer"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(run.out.contains("no callers"), "{}", run.out);
    assert!(
        run.out
            .contains("NetworkPolicy reviewer/grauwolf-mtls-callers: missing")
    );
}

#[tokio::test]
async fn remove_takes_the_agent_out_of_the_callers_of_the_others_and_says_so() {
    let env = two_agents().await;
    env.run(&["agent", "add", "planner"], "").await;
    for caller in ["coordinator", "planner"] {
        env.run(&["agent", "allow", "reviewer", "--from", caller], "")
            .await;
    }
    env.run(&["agent", "allow", "planner", "--from", "coordinator"], "")
        .await;

    let run = env
        .run(&["agent", "remove", "coordinator", "--yes"], "")
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    // The list before the removal and the note after it.
    assert!(
        run.out.contains(
            "its entry in the callers of planner (namespace planner) and the NetworkPolicy there"
        ),
        "{}",
        run.out
    );
    assert!(
        run.out.contains(
            "ConfigMap reviewer/grauwolf-callers: coordinator removed from the callers of reviewer"
        ),
        "{}",
        run.out
    );
    // Only the removed agent's entry goes; the planner stays allowed at the reviewer.
    assert_eq!(
        callers_of(&env, "reviewer"),
        json!({"spiffe://agentswork.de/ns/planner/sa/planner": "planner"})
    );
    assert_eq!(
        policy(&env)["spec"]["ingress"][1]["from"],
        json!([{"namespaceSelector": {"matchLabels":
            {"kubernetes.io/metadata.name": "planner"}}}])
    );
    // The planner had only the coordinator: nobody is admitted on its mTLS port.
    assert_eq!(callers_of(&env, "planner"), json!({}));
    let planner_policy = env
        .fake
        .object(
            "/apis/networking.k8s.io/v1/namespaces/planner/networkpolicies/grauwolf-mtls-callers",
        )
        .unwrap();
    assert_eq!(
        planner_policy["spec"]["ingress"],
        json!([{"ports": [{"protocol": "TCP", "port": 8080}]}])
    );
    // And doctor finds nothing dangling.
    let run = env.run(&["doctor", "--json"], "").await;
    assert!(!run.out.contains("dangling"), "{}", run.out);
}

#[tokio::test]
async fn a_cancelled_remove_leaves_the_callers_of_the_others_alone() {
    let env = two_agents().await;
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let writes = env.fake.writes().len();
    let run = env.run(&["agent", "remove", "coordinator"], "n\n").await;
    assert_eq!(run.code, 1, "{}", run.all());
    assert!(run.err.contains("cancelled"), "{}", run.err);
    assert_eq!(
        callers_of(&env, "reviewer"),
        json!({COORDINATOR_ID: "coordinator"})
    );
    assert_eq!(env.fake.writes().len(), writes);
}

#[tokio::test]
async fn remove_of_an_agent_that_nobody_allows_touches_no_other_agent() {
    let env = two_agents().await;
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let before = callers_of(&env, "reviewer");
    // The coordinator is allowed nowhere: removing it leaves the reviewer's map to `agent deny`,
    // but removing the reviewer needs no clean-up elsewhere.
    let run = env.run(&["agent", "remove", "reviewer", "--yes"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(!run.out.contains("removed from the callers"), "{}", run.out);
    assert!(!run.out.contains("its entry in the callers"), "{}", run.out);
    assert_eq!(before.as_object().unwrap().len(), 1);
}
