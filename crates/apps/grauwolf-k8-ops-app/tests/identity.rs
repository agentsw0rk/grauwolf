//! `identity install | status | uninstall | allow | deny` (ADR-0111 Q3, Q4) against a fake
//! Kubernetes API server and a fake OpenBao.

mod common;

use common::{Env, Run, env};
use grauwolf_k8s::testing::DeploymentOutcome;
use grauwolf_test::TestDir;
use serde_json::{Value, json};

const IMAGE: &str = "registry.example.com/grauwolf:1.0";
const TENANT: &str = "11111111-1111-1111-1111-111111111111";
const ISSUER: &str = "https://entra.example.com/11111111-1111-1111-1111-111111111111/v2.0";
const AUDIENCE: &str = "22222222-2222-2222-2222-222222222222";
const JWKS: &str =
    "https://entra.example.com/11111111-1111-1111-1111-111111111111/discovery/v2.0/keys";
const DEPLOYMENT: &str = "/apis/apps/v1/namespaces/identity/deployments/grauwolf-identity";
const POLICY_MAP: &str = "/api/v1/namespaces/identity/configmaps/grauwolf-identity-policy";
const ID: &str = "spiffe://agentswork.de/ns/identity/sa/grauwolf-identity";

async fn prepared() -> Env {
    let env = env();
    env.prepare().await;
    env.fake.set_deployment_outcome(DeploymentOutcome::Ready(
        "grauwolf identity service\n".into(),
    ));
    env
}

async fn install(env: &Env, extra: &[&str]) -> Run {
    let mut args = vec![
        "identity",
        "install",
        "--tenant",
        TENANT,
        "--issuer",
        ISSUER,
        "--audience",
        AUDIENCE,
        "--jwks-url",
        JWKS,
        "--image",
        IMAGE,
    ];
    args.extend(extra);
    env.run(&args, "").await
}

fn env_of(deployment: &Value, name: &str) -> Option<String> {
    deployment["spec"]["template"]["spec"]["containers"][0]["env"]
        .as_array()?
        .iter()
        .find(|entry| entry["name"] == name)
        .and_then(|entry| entry["value"].as_str().map(str::to_owned))
}

fn policy_in_cluster(env: &Env) -> Value {
    let map = env.fake.object(POLICY_MAP).expect("policy ConfigMap");
    serde_json::from_str(map["data"]["policy.json"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn install_copies_a_pull_secret_and_attaches_it_to_the_service_account() {
    let env = prepared().await;
    env.fake.insert(
        "/api/v1/namespaces/default/secrets/registry",
        json!({"type": "kubernetes.io/dockerconfigjson", "data": {".dockerconfigjson": "c2VjcmV0LXRva2Vu"}}),
    );
    let run = install(&env, &["--pull-secret", "default/registry"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(!run.all().contains("c2VjcmV0"), "{}", run.all());
    assert!(
        run.out
            .contains("pull secret identity/registry (copied from default/registry): created"),
        "{}",
        run.out
    );
    let copy = env
        .fake
        .object("/api/v1/namespaces/identity/secrets/registry")
        .unwrap();
    assert_eq!(copy["data"][".dockerconfigjson"], "c2VjcmV0LXRva2Vu");
    let account = env
        .fake
        .object("/api/v1/namespaces/identity/serviceaccounts/grauwolf-identity")
        .unwrap();
    assert_eq!(account["imagePullSecrets"][0]["name"], "registry");

    let again = install(&env, &["--pull-secret", "default/registry"]).await;
    assert_eq!(again.code, 0, "{}", again.all());
    assert!(
        again
            .out
            .contains("pull secret identity/registry (copied from default/registry): unchanged"),
        "{}",
        again.out
    );
}

#[tokio::test]
async fn install_sets_up_the_service_its_identity_and_its_signing_key() {
    let env = prepared().await;
    let run = install(&env, &[]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(run.out.contains("identity service: ready"), "{}", run.out);
    assert!(
        run.out
            .contains("mTLS endpoint https://grauwolf-identity.identity.svc:8443"),
        "{}",
        run.out
    );

    let namespace = env.fake.object("/api/v1/namespaces/identity").unwrap();
    let labels = &namespace["metadata"]["labels"];
    assert_eq!(labels["pod-security.kubernetes.io/enforce"], "restricted");
    assert_eq!(labels["grauwolf.io/component"], "identity");
    assert!(labels.get("grauwolf.io/agent").is_none(), "not an agent");
    let spiffe = env
        .fake
        .object("/apis/spire.spiffe.io/v1alpha1/clusterspiffeids/grauwolf-identity")
        .unwrap();
    assert_eq!(
        spiffe["spec"]["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"],
        "identity"
    );

    let deployment = env.fake.object(DEPLOYMENT).unwrap();
    let pod = &deployment["spec"]["template"]["spec"];
    assert_eq!(pod["serviceAccountName"], "grauwolf-identity");
    assert_eq!(pod["automountServiceAccountToken"], false);
    assert_eq!(pod["securityContext"]["runAsNonRoot"], true);
    assert_eq!(
        pod["securityContext"]["seccompProfile"]["type"],
        "RuntimeDefault"
    );
    let container = &pod["containers"][0];
    assert_eq!(container["image"], IMAGE);
    assert_eq!(
        container["command"],
        json!([
            "grauwolf",
            "identity",
            "serve",
            "--port",
            "8080",
            "--mtls-port",
            "8443"
        ])
    );
    assert_eq!(
        container["securityContext"]["capabilities"]["drop"],
        json!(["ALL"])
    );
    for (name, value) in [
        ("GRAUWOLF_IDENTITY_TENANT", TENANT),
        ("GRAUWOLF_IDENTITY_ISSUER", ISSUER),
        ("GRAUWOLF_IDENTITY_AUDIENCE", AUDIENCE),
        ("GRAUWOLF_IDENTITY_SCOPE", "access_as_user"),
        ("GRAUWOLF_IDENTITY_JWKS_URL", JWKS),
        (
            "GRAUWOLF_IDENTITY_PUBLIC_URL",
            "https://grauwolf-identity.identity.svc:8443",
        ),
        (
            "GRAUWOLF_IDENTITY_POLICY",
            "/etc/grauwolf/identity/policy.json",
        ),
        ("GRAUWOLF_IDENTITY_TRANSIT_KEY", "grauwolf-identity"),
        (
            "GRAUWOLF_IDENTITY_OPENBAO_ADDR",
            "http://openbao.openbao.svc:8200",
        ),
        ("GRAUWOLF_IDENTITY_OPENBAO_ROLE", "grauwolf-identity"),
        (
            "SPIFFE_ENDPOINT_SOCKET",
            "unix:///spiffe-workload-api/agent.sock",
        ),
    ] {
        assert_eq!(env_of(&deployment, name).as_deref(), Some(value), "{name}");
    }
    // The policy is a directory mount, so changes reach the running service.
    let mounts = container["volumeMounts"].as_array().unwrap();
    assert!(
        mounts
            .iter()
            .any(|mount| mount["mountPath"] == "/etc/grauwolf/identity"
                && mount.get("subPath").is_none())
    );
    assert_eq!(policy_in_cluster(&env), json!({"callers": {}}));

    let service = env
        .fake
        .object("/api/v1/namespaces/identity/services/grauwolf-identity")
        .unwrap();
    assert_eq!(service["spec"]["ports"][1]["port"], 8443);

    // Ingress: mTLS only from the agents' namespaces.
    let ingress = env
        .fake
        .object("/apis/networking.k8s.io/v1/namespaces/identity/networkpolicies/grauwolf-identity-ingress")
        .unwrap();
    assert_eq!(
        ingress["spec"]["ingress"][1]["from"][0]["namespaceSelector"]["matchExpressions"][0]["key"],
        "grauwolf.io/agent"
    );
    // Egress: DNS, OpenBao in its namespace, the keys host by port.
    let egress = env
        .fake
        .object("/apis/networking.k8s.io/v1/namespaces/identity/networkpolicies/grauwolf-egress")
        .unwrap();
    let rules = egress["spec"]["egress"].to_string();
    assert!(rules.contains("\"port\":53"), "{rules}");
    assert!(
        rules.contains("\"kubernetes.io/metadata.name\":\"openbao\"") && rules.contains("8200"),
        "{rules}"
    );
    assert!(
        rules.contains("0.0.0.0/0") && rules.contains("443"),
        "{rules}"
    );

    let bao = env.bao.lock().unwrap();
    assert_eq!(bao.mounts["transit"]["type"], "transit");
    assert_eq!(bao.transit_keys["transit/grauwolf-identity"], "ecdsa-p256");
    let policy = &bao.policies["grauwolf-identity"];
    assert!(
        policy.contains("path \"transit/sign/grauwolf-identity\"")
            && policy.contains("path \"transit/keys/grauwolf-identity\""),
        "{policy}"
    );
    // Besides the key only the grants' folders of the KV mount.
    assert!(!policy.contains("kv/data/agents"), "{policy}");
    let role = &bao.roles["grauwolf-identity"];
    assert_eq!(role["bound_subject"], ID);
    assert_eq!(role["token_policies"], json!(["grauwolf-identity"]));
}

#[tokio::test]
async fn a_second_install_changes_nothing_and_keeps_the_policy() {
    let env = prepared().await;
    install(&env, &[]).await;
    env.run(&["agent", "add", "coordinator"], "").await;
    let allowed = env.run(&["identity", "allow", "coordinator"], "").await;
    assert_eq!(allowed.code, 0, "{}", allowed.all());
    let (k8s_writes, bao_writes) = (env.fake.writes().len(), env.bao_writes());

    let again = install(&env, &[]).await;
    assert_eq!(again.code, 0, "{}", again.all());
    assert!(!again.out.contains(": created"), "{}", again.out);
    assert!(!again.out.contains(": updated"), "{}", again.out);
    assert_eq!(env.fake.writes().len(), k8s_writes, "{}", again.out);
    assert_eq!(env.bao_writes(), bao_writes, "{}", again.out);
    assert!(
        policy_in_cluster(&env)["callers"]
            .get("spiffe://agentswork.de/ns/coordinator/sa/coordinator")
            .is_some()
    );
}

#[tokio::test]
async fn a_policy_file_replaces_the_policy_and_a_bad_one_is_refused_before_anything_is_written() {
    let env = prepared().await;
    let dir = TestDir::new("identity-policy");
    let good = dir.join("policy.json");
    std::fs::write(
        &good,
        r#"{"callers": {"spiffe://agentswork.de/ns/frontends/*": {"purposes": ["chat"]}}}"#,
    )
    .unwrap();
    let bad = dir.join("bad.json");
    std::fs::write(&bad, r#"{"callers": {"coordinator": {}}}"#).unwrap();

    let refused = install(&env, &["--policy", bad.to_str().unwrap()]).await;
    assert_eq!(refused.code, 1);
    assert!(refused.err.contains("not a SPIFFE ID"), "{}", refused.err);
    assert!(env.fake.object(DEPLOYMENT).is_none());
    assert!(env.bao.lock().unwrap().transit_keys.is_empty());

    let run = install(&env, &["--policy", good.to_str().unwrap()]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(
        policy_in_cluster(&env),
        json!({"callers": {"spiffe://agentswork.de/ns/frontends/*": {"purposes": ["chat"]}}})
    );
}

#[tokio::test]
async fn install_refuses_without_openbao_setup_a_foreign_namespace_and_bad_urls() {
    let bare = env();
    let run = install(&bare, &[]).await;
    assert_eq!(run.code, 1);
    assert!(run.err.contains("openbao setup"), "{}", run.err);

    let env = prepared().await;
    let run = env
        .run(
            &[
                "identity",
                "install",
                "--tenant",
                TENANT,
                "--issuer",
                "entra.example.com",
                "--audience",
                AUDIENCE,
                "--image",
                IMAGE,
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 1);
    assert!(run.err.contains("--issuer"), "{}", run.err);

    env.fake.insert(
        "/api/v1/namespaces/identity",
        json!({"kind": "Namespace", "metadata": {"name": "identity"}}),
    );
    let run = install(&env, &[]).await;
    assert_eq!(run.code, 1);
    assert!(
        run.err.contains("not made by `identity install`"),
        "{}",
        run.err
    );
    assert!(env.fake.object(DEPLOYMENT).is_none());
}

#[tokio::test]
async fn a_policy_change_says_that_kubernetes_passes_it_on_after_about_a_minute() {
    let env = prepared().await;
    env.run(&["agent", "add", "coordinator"], "").await;
    install(&env, &[]).await;
    for args in [
        ["identity", "allow", "coordinator"],
        ["identity", "deny", "coordinator"],
    ] {
        let run = env.run(&args, "").await;
        assert_eq!(run.code, 0, "{}", run.all());
        assert!(
            run.out.contains(
                "(Kubernetes passes it to the pod within about a minute or two; the service then reads it again)"
            ),
            "{}",
            run.out
        );
        assert!(!run.out.contains("within seconds"), "{}", run.out);
    }
}

#[tokio::test]
async fn install_takes_several_audiences_and_joins_them_with_commas() {
    let env = prepared().await;
    let run = install(&env, &["--audience", "api://grauwolf-identity"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out
            .contains(&format!("audience {AUDIENCE},api://grauwolf-identity")),
        "{}",
        run.out
    );
    let deployment = env.fake.object(DEPLOYMENT).unwrap();
    assert_eq!(
        env_of(&deployment, "GRAUWOLF_IDENTITY_AUDIENCE").as_deref(),
        Some(format!("{AUDIENCE},api://grauwolf-identity").as_str())
    );
    let bad = install(&env, &["--audience", "two words"]).await;
    assert_eq!(bad.code, 1);
    assert!(bad.err.contains("--audience"), "{}", bad.err);
}

#[tokio::test]
async fn allow_and_deny_edit_the_policy_of_the_service() {
    let env = prepared().await;
    env.run(&["agent", "add", "coordinator"], "").await;
    let before = env.run(&["identity", "allow", "coordinator"], "").await;
    assert_eq!(before.code, 1);
    assert!(before.err.contains("identity install"), "{}", before.err);

    install(&env, &[]).await;
    let unknown = env.run(&["identity", "allow", "nobody"], "").await;
    assert_eq!(unknown.code, 1);
    assert!(
        unknown.err.contains("agent nobody not found"),
        "{}",
        unknown.err
    );

    let run = env
        .run(
            &[
                "identity",
                "allow",
                "coordinator",
                "--purpose",
                "review",
                "--purpose",
                "chat",
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out.contains("purposes: chat, review): created"),
        "{}",
        run.out
    );
    let id = "spiffe://agentswork.de/ns/coordinator/sa/coordinator";
    assert_eq!(
        policy_in_cluster(&env)["callers"][id],
        json!({"purposes": ["chat", "review"]})
    );
    let again = env
        .run(
            &[
                "identity",
                "allow",
                "coordinator",
                "--purpose",
                "review",
                "--purpose",
                "chat",
            ],
            "",
        )
        .await;
    assert!(again.out.contains(": unchanged"), "{}", again.out);
    let any = env.run(&["identity", "allow", "coordinator"], "").await;
    assert!(any.out.contains("(purposes: any): updated"), "{}", any.out);
    assert_eq!(policy_in_cluster(&env)["callers"][id], json!({}));

    let status = env.run(&["identity", "status"], "").await;
    assert!(
        status
            .out
            .contains(&format!("{id}  purposes: any  agent coordinator")),
        "{}",
        status.out
    );

    let denied = env.run(&["identity", "deny", "coordinator"], "").await;
    assert_eq!(denied.code, 0, "{}", denied.all());
    assert!(denied.out.contains("removed"), "{}", denied.out);
    assert_eq!(policy_in_cluster(&env), json!({"callers": {}}));
    let twice = env.run(&["identity", "deny", "coordinator"], "").await;
    assert!(
        twice.out.contains("does not allow coordinator"),
        "{}",
        twice.out
    );
}

#[tokio::test]
async fn status_shows_the_service_its_urls_and_its_openbao_objects() {
    let env = prepared().await;
    let none = env.run(&["identity", "status"], "").await;
    assert_eq!(none.code, 0);
    assert!(none.out.contains("not installed"), "{}", none.out);
    let none = env.run(&["identity", "status", "--json"], "").await;
    assert_eq!(none.out.trim(), r#"{"installed":false}"#);

    install(&env, &[]).await;
    let run = env.run(&["identity", "status"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    for expected in [
        "installed, ready",
        &format!("SPIFFE ID {ID}"),
        "mTLS endpoint https://grauwolf-identity.identity.svc:8443",
        "keys http://grauwolf-identity.identity.svc:8080/api/identity/jwks.json",
        &format!("issuer {ISSUER}"),
        "policy: 0 caller(s)",
        "OpenBao role grauwolf-identity: exists, policy grauwolf-identity: exists, Transit key transit/keys/grauwolf-identity: ecdsa-p256",
    ] {
        assert!(run.out.contains(expected), "{expected}\n{}", run.out);
    }
    let json: Value = serde_json::from_str(
        env.run(&["identity", "status", "--json"], "")
            .await
            .out
            .trim(),
    )
    .unwrap();
    assert_eq!(json["ready"], true);
    assert_eq!(json["spiffe_id"], ID);
    assert_eq!(json["config"]["audience"], AUDIENCE);
}

#[tokio::test]
async fn uninstall_keeps_the_key_unless_asked_and_removes_everything_else() {
    let env = prepared().await;
    install(&env, &["--transit-key", "txn"]).await;
    let cancelled = env.run(&["identity", "uninstall"], "n\n").await;
    assert_eq!(cancelled.code, 1);
    assert!(env.fake.object(DEPLOYMENT).is_some());

    let run = env.run(&["identity", "uninstall", "--yes"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out
            .contains("kept: OpenBao Transit key transit/keys/txn"),
        "{}",
        run.out
    );
    assert!(env.fake.object("/api/v1/namespaces/identity").is_none());
    {
        let bao = env.bao.lock().unwrap();
        assert!(!bao.roles.contains_key("grauwolf-identity"));
        assert!(!bao.policies.contains_key("grauwolf-identity"));
        assert!(bao.transit_keys.contains_key("transit/txn"));
    }

    install(&env, &["--transit-key", "txn"]).await;
    let run = env
        .run(&["identity", "uninstall", "--yes", "--delete-key"], "")
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(env.bao.lock().unwrap().transit_keys.is_empty());
    let gone = env.run(&["identity", "uninstall", "--yes"], "").await;
    assert_eq!(gone.code, 1);
    assert!(gone.err.contains("not installed"), "{}", gone.err);
}

#[tokio::test]
async fn doctor_says_whether_the_identity_service_is_installed() {
    let env = prepared().await;
    let run = env.run(&["doctor"], "").await;
    assert!(
        run.out.contains("identity service: not installed"),
        "{}",
        run.out
    );
    install(&env, &[]).await;
    let run = env.run(&["doctor"], "").await;
    assert!(
        run.out
            .contains("ok    identity service: installed in namespace identity, ready"),
        "{}",
        run.out
    );
}

#[tokio::test]
async fn install_and_uninstall_need_nothing_beyond_the_restricted_policy() {
    let env = prepared().await;
    env.run(&["agent", "add", "coordinator"], "").await;
    env.restrict_to("grauwolf-k8-ops");
    let run = install(&env, &[]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    let run = env.run(&["identity", "allow", "coordinator"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    let run = env.run(&["identity", "status"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    let run = env
        .run(&["identity", "uninstall", "--yes", "--delete-key"], "")
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        env.bao.lock().unwrap().denied.is_empty(),
        "{:?}",
        env.bao.lock().unwrap().denied
    );
}

const AGENT_DEPLOYMENT: &str = "/apis/apps/v1/namespaces/coordinator/deployments/coordinator";

async fn deploy_coordinator(env: &Env, extra: &[&str]) -> Run {
    let mut args = vec!["agent", "deploy", "coordinator", "--image", IMAGE];
    args.extend(extra);
    env.run(&args, "").await
}

#[tokio::test]
async fn deploy_with_identity_needs_the_service_and_gives_the_agent_its_url_and_id() {
    let env = prepared().await;
    env.run(&["agent", "add", "coordinator"], "").await;
    let refused = deploy_coordinator(&env, &["--identity"]).await;
    assert_eq!(refused.code, 1);
    assert!(refused.err.contains("identity install"), "{}", refused.err);
    assert!(env.fake.object(AGENT_DEPLOYMENT).is_none());

    install(&env, &[]).await;
    let run = deploy_coordinator(&env, &["--identity"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out.contains(
            "delegation: on, identity service https://grauwolf-identity.identity.svc:8443"
        ),
        "{}",
        run.out
    );
    let deployment = env.fake.object(AGENT_DEPLOYMENT).unwrap();
    assert_eq!(
        env_of(&deployment, "GRAUWOLF_IDENTITY_URL").as_deref(),
        Some("https://grauwolf-identity.identity.svc:8443")
    );
    assert_eq!(
        env_of(&deployment, "GRAUWOLF_IDENTITY_SPIFFE_ID").as_deref(),
        Some(ID)
    );
    // SPIFFE is always on in a deployed agent, as the service requires for delegation.
    assert_eq!(env_of(&deployment, "GRAUWOLF_SPIFFE").as_deref(), Some("1"));
    let status = env.run(&["agent", "status", "coordinator"], "").await;
    assert!(
        status
            .out
            .contains("identity: on (https://grauwolf-identity.identity.svc:8443)"),
        "{}",
        status.out
    );

    // Like every option, only on while given.
    let hash = |env: &Env| {
        env.fake.object(AGENT_DEPLOYMENT).unwrap()["spec"]["template"]["metadata"]["annotations"]
            ["grauwolf.io/config-hash"]
            .clone()
    };
    let before = hash(&env);
    let run = deploy_coordinator(&env, &[]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_ne!(hash(&env), before);
    let deployment = env.fake.object(AGENT_DEPLOYMENT).unwrap();
    assert!(env_of(&deployment, "GRAUWOLF_IDENTITY_URL").is_none());
    let status = env.run(&["agent", "status", "coordinator"], "").await;
    assert!(status.out.contains("identity: off"), "{}", status.out);
}

#[tokio::test]
async fn env_cannot_set_the_identity_variables() {
    let env = prepared().await;
    env.run(&["agent", "add", "coordinator"], "").await;
    let run = deploy_coordinator(
        &env,
        &["--env", "GRAUWOLF_IDENTITY_URL=https://elsewhere:8443"],
    )
    .await;
    assert_eq!(run.code, 1);
    assert!(run.err.contains("use --identity"), "{}", run.err);
}

#[tokio::test]
async fn with_egress_the_identity_service_is_an_allowed_destination() {
    let env = prepared().await;
    install(&env, &[]).await;
    env.run(&["agent", "add", "coordinator"], "").await;
    let dir = TestDir::new("identity-egress");
    let spec = dir.join("coordinator.json");
    std::fs::write(
        &spec,
        json!({"spec_version": 1, "name": "coordinator",
            "model": {"provider": "ollama", "id": "m"},
            "strategy": {"kind": "single_run"}})
        .to_string(),
    )
    .unwrap();
    let run = deploy_coordinator(
        &env,
        &[
            "--spec",
            spec.to_str().unwrap(),
            "--env",
            "OLLAMA_HOST=http://ollama.ollama.svc:11434",
            "--identity",
            "--egress",
        ],
    )
    .await;
    assert_eq!(run.code, 0, "{}", run.all());
    let policy = env
        .fake
        .object("/apis/networking.k8s.io/v1/namespaces/coordinator/networkpolicies/grauwolf-egress")
        .unwrap();
    let identity_rule = policy["spec"]["egress"]
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| {
            rule["to"][0]["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"]
                == "identity"
        })
        .cloned()
        .expect("a rule for the identity namespace");
    assert_eq!(identity_rule["ports"][0]["port"], 8443);
}

/// Split DNS: the issuer's host resolves, inside the cluster, to the LoadBalancer of the cluster's
/// own ingress, which kube-proxy sends straight to the Traefik pods.
#[tokio::test]
async fn a_keys_host_behind_the_clusters_own_ingress_is_allowed_to_the_ingress_pods() {
    let env = prepared().await;
    env.dns.lock().unwrap().insert(
        "entra.example.com".to_owned(),
        vec!["192.168.178.199".parse().unwrap()],
    );
    env.fake.insert(
        "/api/v1/namespaces/kube-system/services/traefik",
        json!({"apiVersion": "v1", "kind": "Service",
            "metadata": {"name": "traefik", "namespace": "kube-system"},
            "spec": {"type": "LoadBalancer",
                "selector": {"app.kubernetes.io/name": "traefik"},
                "ports": [{"name": "websecure", "port": 443, "targetPort": "websecure"}]},
            "status": {"loadBalancer": {"ingress": [{"ip": "192.168.178.199"}]}}}),
    );
    env.fake.insert(
        "/apis/apps/v1/namespaces/kube-system/deployments/traefik",
        json!({"apiVersion": "apps/v1", "kind": "Deployment",
            "metadata": {"name": "traefik", "namespace": "kube-system"},
            "spec": {"template": {
                "metadata": {"labels": {"app.kubernetes.io/name": "traefik"}},
                "spec": {"containers": [{"name": "traefik", "ports": [
                    {"name": "websecure", "containerPort": 8443}]}]}}}}),
    );
    let run = install(&env, &[]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out.contains(
            "egress: entra.example.com resolves to the LoadBalancer of kube-system/traefik; allowing its pods (port 8443)"
        ),
        "{}",
        run.all()
    );
    let policy = env
        .fake
        .object("/apis/networking.k8s.io/v1/namespaces/identity/networkpolicies/grauwolf-egress")
        .unwrap();
    let rules = policy["spec"]["egress"].as_array().unwrap();
    let pods = rules
        .iter()
        .find(|rule| {
            rule["to"][0].get("podSelector").is_some()
                && rule["to"][0]["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"]
                    == "kube-system"
                && rule["ports"][0]["port"] == 8443
        })
        .expect("a rule for the ingress pods");
    assert_eq!(
        pods["to"][0]["podSelector"]["matchLabels"]["app.kubernetes.io/name"],
        "traefik"
    );
    assert_eq!(pods["ports"].as_array().unwrap().len(), 1);
    // The external rule stays.
    assert!(
        rules
            .iter()
            .any(|rule| rule["to"][0]["ipBlock"]["cidr"] == "0.0.0.0/0"
                && rule["ports"][0]["port"] == 443),
        "{policy}"
    );
}

#[tokio::test]
async fn check_and_doctor_point_at_a_missing_identity_service() {
    let env = prepared().await;
    install(&env, &[]).await;
    env.run(&["agent", "add", "coordinator"], "").await;
    deploy_coordinator(&env, &["--identity"]).await;
    env.run(&["identity", "uninstall", "--yes"], "").await;

    let run = env
        .run(
            &[
                "agent",
                "check",
                "coordinator",
                "--image",
                IMAGE,
                "--timeout",
                "1",
            ],
            "",
        )
        .await;
    assert!(
        run.err.contains(
            "hint: agent coordinator is deployed with --identity, but the identity service is not installed"
        ),
        "{}",
        run.all()
    );
    let doctor = env.run(&["doctor"], "").await;
    assert!(
        doctor.out.contains(
            "fail  identity service: not installed, but deployed with --identity: coordinator"
        ),
        "{}",
        doctor.out
    );
}

#[tokio::test]
async fn allow_and_deny_edit_the_grants_section_for_agents_and_triggers() {
    let env = prepared().await;
    install(&env, &[]).await;
    env.run(&["agent", "add", "coordinator"], "").await;
    let run = env
        .run(
            &[
                "identity",
                "allow",
                "coordinator",
                "--grants",
                "--purpose",
                "review",
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out
            .contains("to run on grants of people (purposes: review): created"),
        "{}",
        run.out
    );
    let run = env
        .run(&["identity", "allow", "--trigger", "morning"], "")
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    let run = env
        .run(
            &[
                "identity",
                "allow",
                "--trigger",
                "spiffe://agentswork.de/ns/ci/*",
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    let policy = policy_in_cluster(&env);
    let coordinator = "spiffe://agentswork.de/ns/coordinator/sa/coordinator";
    assert_eq!(
        policy["grants"]["agents"][coordinator],
        json!({"purposes": ["review"]})
    );
    assert_eq!(
        policy["grants"]["triggers"],
        json!([
            "spiffe://agentswork.de/ns/ci/*",
            "spiffe://agentswork.de/ns/schedules/sa/morning"
        ])
    );
    // The callers of stage 1 are a separate list.
    assert_eq!(policy["callers"], json!({}));
    let again = env
        .run(&["identity", "allow", "--trigger", "morning"], "")
        .await;
    assert!(again.out.contains(": unchanged"), "{}", again.out);
    let status = env.run(&["identity", "status"], "").await;
    assert!(
        status
            .out
            .contains("grants: 1 agent(s) may run on grants, 2 trigger(s) may start them"),
        "{}",
        status.out
    );
    let status = env.run(&["identity", "status", "--json"], "").await;
    let status: Value = serde_json::from_str(status.out.trim()).unwrap();
    assert_eq!(
        status["grants"]["agents"][coordinator],
        json!({"purposes": ["review"]})
    );

    // Both at once, or neither, is refused by the parser.
    use clap::Parser;
    for args in [
        &["identity", "allow", "coordinator", "--trigger", "morning"][..],
        &["identity", "allow"][..],
        &["identity", "deny", "--trigger", "morning", "--grants"][..],
    ] {
        let argv = std::iter::once("grauwolf-k8-ops").chain(args.iter().copied());
        assert!(
            grauwolf_k8_ops_app::Cli::try_parse_from(argv).is_err(),
            "{args:?}"
        );
    }

    let run = env
        .run(&["identity", "deny", "coordinator", "--grants"], "")
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    let run = env
        .run(&["identity", "deny", "--trigger", "morning"], "")
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    let policy = policy_in_cluster(&env);
    assert_eq!(policy["grants"]["agents"], json!({}));
    assert_eq!(
        policy["grants"]["triggers"],
        json!(["spiffe://agentswork.de/ns/ci/*"])
    );
}

#[tokio::test]
async fn install_with_a_grant_client_enables_grants_and_lets_the_service_keep_them_in_kv() {
    let env = prepared().await;
    let run = install(&env, &["--grant-client-id", "cli-app"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    let deployment = env.fake.object(DEPLOYMENT).unwrap();
    assert_eq!(
        env_of(&deployment, "GRAUWOLF_IDENTITY_GRANT_CLIENT_ID").as_deref(),
        Some("cli-app")
    );
    assert_eq!(
        env_of(&deployment, "GRAUWOLF_IDENTITY_GRANT_KV_MOUNT").as_deref(),
        Some("kv")
    );
    let policy = env.bao.lock().unwrap().policies["grauwolf-identity"].clone();
    for (path, capability) in [
        ("kv/data/identity/grants/grt_1", "create"),
        ("kv/data/identity/grant-tokens/grt_1", "update"),
        ("kv/data/identity/grant-tokens/grt_1", "read"),
        ("kv/metadata/identity/grants", "list"),
        ("kv/metadata/identity/grant-tokens/grt_1", "delete"),
    ] {
        assert!(
            common::policy_capabilities(&policy, path).contains(&capability.to_owned()),
            "{path} {capability}\n{policy}"
        );
    }
    assert!(common::policy_capabilities(&policy, "kv/data/agents/a/key").is_empty());
    // Repeatable.
    let again = install(&env, &["--grant-client-id", "cli-app"]).await;
    assert_eq!(again.code, 0, "{}", again.all());
    assert!(again.out.contains("unchanged"), "{}", again.out);
}

const GRANT: &str = "grt_0123456789abcdef0123456789abcdef";

fn alices_grant(id: &str, valid_until: u64) -> Value {
    json!({"id": id, "owner": {"oid": "oid-alice", "name": "Alice", "upn": "alice@example.org"},
        "agent": "spiffe://agentswork.de/ns/coordinator/sa/coordinator",
        "trigger": "spiffe://agentswork.de/ns/schedules/sa/morning",
        "purposes": ["review"], "valid_until": valid_until, "max_run": 14400,
        "created": 1_790_000_000_u64, "last_used": 1_790_000_600_u64})
}

#[tokio::test]
async fn admins_list_every_grant_without_tokens_and_revoke_one() {
    let env = prepared().await;
    let none = env.run(&["identity", "grants"], "").await;
    assert_eq!(none.code, 0, "{}", none.all());
    assert!(none.out.contains("no grants"), "{}", none.out);
    env.store_grant(alices_grant(GRANT, 4_000_000_000));
    let expired = "grt_ffffffffffffffffffffffffffffffff";
    env.store_grant(alices_grant(expired, 1_000));
    env.restrict_to("grauwolf-k8-ops");

    let run = env.run(&["identity", "grants"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(!run.all().contains("rt-secret"), "{}", run.all());
    assert!(
        run.out.contains(&format!(
            "{GRANT}  owner Alice <alice@example.org> (oid-alice)  agent spiffe://agentswork.de/ns/coordinator/sa/coordinator  trigger spiffe://agentswork.de/ns/schedules/sa/morning  purposes review  valid to 2096-10-02 07:06 UTC  last used 2026-09-21 14:23 UTC"
        )),
        "{}",
        run.out
    );
    assert!(run.out.contains("(expired)"), "{}", run.out);
    let listed = env.run(&["identity", "grants", "--json"], "").await;
    let listed: Value = serde_json::from_str(listed.out.trim()).unwrap();
    assert_eq!(listed["grants"].as_array().unwrap().len(), 2);
    assert!(!listed.to_string().contains("rt-secret"));

    let run = env
        .run(&["identity", "grants", "--revoke", GRANT], "")
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    {
        let bao = env.bao.lock().unwrap();
        assert!(!bao.kv.contains_key(&format!("kv/identity/grants/{GRANT}")));
        assert!(
            !bao.kv
                .contains_key(&format!("kv/identity/grant-tokens/{GRANT}"))
        );
        assert!(
            bao.kv
                .contains_key(&format!("kv/identity/grant-tokens/{expired}"))
        );
        assert!(bao.denied.is_empty(), "{:?}", bao.denied);
    }
    let again = env
        .run(&["identity", "grants", "--revoke", GRANT], "")
        .await;
    assert_eq!(again.code, 1);
    assert!(again.err.contains("no grant"), "{}", again.err);
    let bad = env
        .run(&["identity", "grants", "--revoke", "../x"], "")
        .await;
    assert_eq!(bad.code, 1);
}
