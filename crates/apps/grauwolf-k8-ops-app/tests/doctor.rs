//! `doctor`: prerequisites and setup, with hints.

mod common;

use common::{Env, env, oidc_ready};
use serde_json::{Value, json};

/// The checks of `doctor --json`: `(section, check, status, reason, hint)`.
async fn doctor(env: &Env) -> (i32, Vec<Value>) {
    let run = env.run(&["doctor", "--json"], "").await;
    (run.code, serde_json::from_str(&run.out).expect("json"))
}

fn find<'a>(checks: &'a [Value], name: &str) -> &'a Value {
    checks
        .iter()
        .find(|check| check["check"] == name)
        .unwrap_or_else(|| panic!("no check '{name}' in {checks:#?}"))
}

async fn complete() -> Env {
    let env = env();
    env.prepare().await;
    env.run(&["agent", "add", "reviewer"], "").await;
    env
}

#[tokio::test]
async fn a_complete_setup_passes_every_check() {
    let env = complete().await;
    let (code, checks) = doctor(&env).await;
    let bad: Vec<_> = checks
        .iter()
        .filter(|check| check["status"] == "fail")
        .collect();
    assert_eq!(code, 0, "{bad:#?}");
    assert!(checks.iter().all(|check| check["status"] != "fail"));
    for name in [
        "cluster",
        "SPIRE server",
        "SPIRE agents",
        "SPIFFE CSI driver",
        "ClusterSPIFFEID CRD",
        "spire-controller-manager",
        "OpenBao",
        "OpenBao token",
        "OIDC discovery provider",
        "JWT auth method",
        "KV v2 mount",
        "trust domain",
        "agent reviewer: OpenBao role",
        "agent reviewer: OpenBao policy",
        "agent reviewer: ConfigMap",
    ] {
        assert_eq!(find(&checks, name)["status"], "ok", "{name}");
    }
    assert!(
        find(&checks, "cluster")["reason"]
            .as_str()
            .unwrap()
            .contains("v1.34")
    );
    assert_eq!(find(&checks, "cluster")["section"], "prerequisites");
    assert_eq!(find(&checks, "KV v2 mount")["section"], "setup");
}

#[tokio::test]
async fn the_text_output_has_two_sections_and_hints() {
    let env = env();
    let run = env.run(&["doctor"], "").await;
    assert_eq!(run.code, 1);
    let prerequisites = run.out.find("Prerequisites").unwrap();
    let setup = run.out.find("Setup").unwrap();
    assert!(prerequisites < setup);
    assert!(
        run.out.contains("  fail  OIDC discovery provider: "),
        "{}",
        run.out
    );
    assert!(
        run.out
            .contains("        next: run `grauwolf-k8-ops oidc install`"),
        "{}",
        run.out
    );
}

#[tokio::test]
async fn missing_spire_is_reported_per_piece_with_the_hint_that_it_is_not_installed() {
    let env = Env {
        fake: grauwolf_k8s::testing::FakeCluster::new(),
        ..env()
    };
    // Re-point the proxy: the fake cluster is empty, only OpenBao answers.
    let state = env.bao.clone();
    env.fake.on_proxy(move |_, _, _, _| {
        let _ = &state;
        (
            200,
            json!({"initialized": true, "sealed": false, "version": "2.7.2"}).to_string(),
        )
    });
    let (code, checks) = doctor(&env).await;
    assert_eq!(code, 1);
    for name in ["SPIRE server", "SPIRE agents", "ClusterSPIFFEID CRD"] {
        let check = find(&checks, name);
        assert_eq!(check["status"], "fail", "{name}");
        assert!(
            check["hint"]
                .as_str()
                .unwrap()
                .contains("does not install SPIRE"),
            "{check}"
        );
    }
    assert_eq!(find(&checks, "SPIFFE CSI driver")["status"], "fail");
    assert_eq!(find(&checks, "spire-controller-manager")["status"], "fail");
    assert_eq!(find(&checks, "trust domain")["status"], "fail");
}

#[tokio::test]
async fn a_sealed_openbao_fails_with_the_unseal_hint() {
    let env = complete().await;
    env.bao.lock().unwrap().sealed = true;
    let (code, checks) = doctor(&env).await;
    assert_eq!(code, 1);
    let check = find(&checks, "OpenBao");
    assert_eq!(check["status"], "fail");
    assert_eq!(check["reason"], "reachable but sealed");
    assert!(check["hint"].as_str().unwrap().contains("unseal"));
}

#[tokio::test]
async fn a_token_without_rights_names_the_paths_it_lacks() {
    let env = complete().await;
    env.bao.lock().unwrap().capabilities = Some(vec!["read".into()]);
    let (code, checks) = doctor(&env).await;
    assert_eq!(code, 1);
    let check = find(&checks, "OpenBao token");
    assert_eq!(check["status"], "fail");
    let reason = check["reason"].as_str().unwrap();
    for path in [
        "sys/auth/jwt",
        "sys/mounts/kv",
        "sys/policies/acl/",
        "auth/jwt/role/",
        "kv/data/agents/",
    ] {
        assert!(reason.contains(path), "{reason}");
    }
}

#[tokio::test]
async fn an_invalid_token_fails_and_no_token_only_warns() {
    let env = complete().await;
    env.bao.lock().unwrap().capabilities = None;
    let (_, checks) = doctor(&env).await;
    assert_eq!(find(&checks, "OpenBao token")["status"], "fail");
    assert!(
        find(&checks, "OpenBao token")["reason"]
            .as_str()
            .unwrap()
            .contains("not valid")
    );

    let mut env = complete().await;
    env.token = None;
    let (code, checks) = doctor(&env).await;
    assert_eq!(code, 0);
    assert_eq!(find(&checks, "OpenBao token")["status"], "warn");
    assert_eq!(find(&checks, "JWT auth method")["status"], "warn");
}

#[tokio::test]
async fn missing_rights_in_the_cluster_are_named() {
    let env = complete().await;
    env.fake.deny("create", "jobs");
    env.fake.deny("get", "services/proxy");
    let (code, checks) = doctor(&env).await;
    assert_eq!(code, 1);
    assert!(
        find(&checks, "rights on jobs")["reason"]
            .as_str()
            .unwrap()
            .contains("create")
    );
    assert!(
        find(&checks, "rights on services/proxy")["reason"]
            .as_str()
            .unwrap()
            .contains("get")
    );
    assert_eq!(find(&checks, "rights on pods/log")["status"], "ok");
}

#[tokio::test]
async fn a_daemonset_that_is_not_ready_on_every_node_fails() {
    let env = complete().await;
    env.fake
        .insert("/api/v1/nodes/node-2", json!({"kind": "Node"}));
    let (code, checks) = doctor(&env).await;
    assert_eq!(code, 1);
    assert_eq!(find(&checks, "SPIRE agents")["status"], "fail");
}

#[tokio::test]
async fn missing_setup_is_reported_with_the_command_that_fixes_it() {
    let env = env();
    let (code, checks) = doctor(&env).await;
    assert_eq!(code, 1);
    assert_eq!(
        find(&checks, "OIDC discovery provider")["hint"],
        "run `grauwolf-k8-ops oidc install`"
    );
    assert_eq!(
        find(&checks, "JWT auth method")["hint"],
        "run `grauwolf-k8-ops openbao setup`"
    );
    assert_eq!(
        find(&checks, "KV v2 mount")["hint"],
        "run `grauwolf-k8-ops openbao setup`"
    );
    assert_eq!(
        find(&checks, "OIDC provider ClusterSPIFFEID")["status"],
        "fail"
    );
    // The prerequisites are fine.
    assert_eq!(find(&checks, "SPIRE server")["status"], "ok");
}

#[tokio::test]
async fn an_oidc_provider_that_is_not_ready_fails() {
    let env = env();
    env.run(&["oidc", "install"], "").await;
    let (_, checks) = doctor(&env).await;
    assert_eq!(find(&checks, "OIDC discovery provider")["status"], "fail");
    oidc_ready(&env.fake);
    let (_, checks) = doctor(&env).await;
    assert_eq!(find(&checks, "OIDC discovery provider")["status"], "ok");
}

#[tokio::test]
async fn a_mismatched_trust_domain_fails_and_names_the_places() {
    let env = complete().await;
    env.fake.insert(
        "/api/v1/namespaces/spire-system/configmaps/spire-controller-manager",
        json!({"data": {"controller-manager.yaml": "trustDomain: other.example\n"}}),
    );
    let (code, checks) = doctor(&env).await;
    assert_eq!(code, 1);
    let check = find(&checks, "trust domain");
    assert_eq!(check["status"], "fail");
    let reason = check["reason"].as_str().unwrap();
    assert!(
        reason.contains("other.example (spire-controller-manager)"),
        "{reason}"
    );
    assert!(reason.contains("agentswork.de"), "{reason}");
}

#[tokio::test]
async fn a_role_bound_to_another_domain_is_a_mismatch_too() {
    let env = complete().await;
    env.bao
        .lock()
        .unwrap()
        .roles
        .get_mut("agent-reviewer")
        .unwrap()["bound_subject"] = json!("spiffe://old.example/ns/reviewer/sa/reviewer");
    let (code, checks) = doctor(&env).await;
    assert_eq!(code, 1);
    assert_eq!(
        find(&checks, "agent reviewer: OpenBao role")["status"],
        "fail"
    );
    assert!(
        find(&checks, "trust domain")["reason"]
            .as_str()
            .unwrap()
            .contains("old.example")
    );
}

#[tokio::test]
async fn a_policy_wider_than_the_agents_path_fails() {
    let env = complete().await;
    env.bao.lock().unwrap().policies.insert(
        "agent-reviewer".into(),
        "path \"kv/data/agents/*\" {\n  capabilities = [\"read\"]\n}\n".into(),
    );
    let (_, checks) = doctor(&env).await;
    assert_eq!(
        find(&checks, "agent reviewer: OpenBao policy")["status"],
        "fail"
    );
}

#[tokio::test]
async fn the_shared_role_of_the_example_is_a_warning() {
    let env = complete().await;
    env.bao
        .lock()
        .unwrap()
        .roles
        .insert("grauwolf-agents".into(), json!({}));
    let (code, checks) = doctor(&env).await;
    assert_eq!(code, 0);
    assert_eq!(find(&checks, "legacy shared role")["status"], "warn");
}

#[tokio::test]
async fn a_missing_namespace_service_account_or_configmap_of_an_agent_fails_with_the_fix() {
    let env = complete().await;
    env.fake.insert(
        "/api/v1/namespaces/reviewer",
        json!({"kind": "Namespace", "metadata": {"name": "reviewer", "labels": {"grauwolf.io/agent": "reviewer", "app.kubernetes.io/managed-by": "grauwolf-k8-ops"}}}),
    );
    let (code, checks) = doctor(&env).await;
    assert_eq!(code, 1);
    let check = find(&checks, "agent reviewer: namespace");
    assert_eq!(check["status"], "fail");
    assert_eq!(check["hint"], "run `grauwolf-k8-ops agent add reviewer`");
}

#[tokio::test]
async fn the_first_line_of_the_text_output_names_the_server_and_the_context() {
    let env = env();
    let run = env.run(&["doctor"], "").await;
    assert_eq!(
        run.out.lines().next(),
        Some("Cluster https://fake-cluster.test:6443 (context fake-context)"),
        "{}",
        run.out
    );
    let (_, checks) = doctor(&env).await;
    assert!(
        find(&checks, "cluster")["reason"]
            .as_str()
            .unwrap()
            .contains("https://fake-cluster.test:6443 (context fake-context)")
    );
}
