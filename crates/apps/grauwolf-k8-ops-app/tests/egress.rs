//! `agent deploy --egress`: the NetworkPolicy `grauwolf-egress`, its lifecycle, `agent status` and
//! `doctor` (ADR-0110 stage 6), against a fake Kubernetes API server.

mod common;

use std::path::{Path, PathBuf};

use common::{Env, Run, env};
use grauwolf_k8s::testing::DeploymentOutcome;
use grauwolf_test::TestDir;
use serde_json::{Value, json};

const POLICY: &str =
    "/apis/networking.k8s.io/v1/namespaces/reviewer/networkpolicies/grauwolf-egress";
const CALLERS_POLICY: &str =
    "/apis/networking.k8s.io/v1/namespaces/reviewer/networkpolicies/grauwolf-mtls-callers";
const DEPLOYMENT: &str = "/apis/apps/v1/namespaces/reviewer/deployments/reviewer";
const IMAGE: &str = "registry.example.com/grauwolf:1.0";

/// An agent with an Ollama (address from `--env`), an A2A agent in the cluster, Jira outside and
/// a sub-agent that uses another model host.
fn write_spec(dir: &TestDir) -> PathBuf {
    std::fs::write(
        dir.join("helper.json"),
        json!({"spec_version": 1, "name": "helper",
            "model": {"provider": "ollama", "id": "m"},
            "strategy": {"kind": "single_run"}})
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("reviewer.json"),
        json!({"spec_version": 1, "name": "reviewer",
            "model": {"provider": "ollama", "id": "m"},
            "agent_tools": {
                "peer": {"url": "http://peer.team.svc:8080/a2a", "description": "Peer"},
                "helper": {"spec": "helper.json", "description": "Helps"}},
            "builtin_tools": {"tools": ["jira_get_issue"],
                "jira": {"base_url": "https://acme.atlassian.net"}},
            "strategy": {"kind": "react"}})
        .to_string(),
    )
    .unwrap();
    dir.join("reviewer.json")
}

async fn added() -> Env {
    let env = env();
    env.prepare().await;
    env.run(&["agent", "add", "reviewer"], "").await;
    env.fake.set_deployment_outcome(DeploymentOutcome::Ready(
        "listening on 0.0.0.0:8080\n".into(),
    ));
    // The peer's Service: port 8080 leads to port 9090 of its pods.
    env.fake.insert(
        "/api/v1/namespaces/team/services/peer",
        json!({"apiVersion": "v1", "kind": "Service",
            "metadata": {"name": "peer", "namespace": "team"},
            "spec": {"ports": [{"port": 8080, "targetPort": 9090}]}}),
    );
    env
}

async fn deploy(env: &Env, spec: &Path, extra: &[&str]) -> Run {
    let mut args = vec![
        "agent",
        "deploy",
        "reviewer",
        "--image",
        IMAGE,
        "--spec",
        spec.to_str().unwrap(),
        "--env",
        "OLLAMA_HOST=http://10.1.2.3:11434",
    ];
    args.extend(extra);
    env.run(&args, "").await
}

fn rules(policy: &Value) -> Vec<Value> {
    policy["spec"]["egress"].as_array().unwrap().clone()
}

fn ports_of(rule: &Value) -> Vec<u64> {
    rule["ports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|port| port["port"].as_u64().unwrap())
        .collect()
}

fn namespace_rule(policy: &Value, namespace: &str) -> Option<Value> {
    rules(policy).into_iter().find(|rule| {
        rule["to"][0]["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"]
            == namespace
            && rule["to"][0].get("podSelector").is_none()
    })
}

#[tokio::test]
async fn the_policy_allows_dns_the_cluster_destinations_the_ips_and_the_external_ports() {
    let env = added().await;
    let dir = TestDir::new("egress-policy");
    let spec = write_spec(&dir);
    let run = deploy(
        &env,
        &spec,
        &[
            "--egress",
            "--otel",
            "--otel-endpoint",
            "http://lgtm.lgtm.svc:4318",
        ],
    )
    .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out
            .contains("NetworkPolicy reviewer/grauwolf-egress (5 destinations, 1 only port-restricted): created"),
        "{}",
        run.out
    );
    let policy = env.fake.object(POLICY).expect("the policy");
    assert_eq!(policy["spec"]["policyTypes"], json!(["Egress"]));
    assert_eq!(
        policy["spec"]["podSelector"]["matchLabels"],
        json!({"grauwolf.io/agent": "reviewer"})
    );
    let all = rules(&policy);
    // DNS: UDP and TCP 53 to kube-dns.
    assert_eq!(
        all[0]["to"][0],
        json!({"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "kube-system"}},
            "podSelector": {"matchLabels": {"k8s-app": "kube-dns"}}})
    );
    assert_eq!(
        all[0]["ports"],
        json!([{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}])
    );
    // The peer: by namespace, on the pod port of its Service.
    assert_eq!(ports_of(&namespace_rule(&policy, "team").unwrap()), [9090]);
    // OpenBao and the collector.
    assert_eq!(
        ports_of(&namespace_rule(&policy, "openbao").unwrap()),
        [8200]
    );
    assert_eq!(ports_of(&namespace_rule(&policy, "lgtm").unwrap()), [4318]);
    // Ollama by IP.
    let ollama = all
        .iter()
        .find(|rule| rule["to"][0]["ipBlock"]["cidr"] == "10.1.2.3/32")
        .unwrap();
    assert_eq!(ports_of(ollama), [11434]);
    // Jira outside: everything outside the cluster's ranges, on 443.
    let external = all
        .iter()
        .find(|rule| rule["to"][0]["ipBlock"]["cidr"] == "0.0.0.0/0")
        .unwrap();
    assert_eq!(
        external["to"][0]["ipBlock"]["except"],
        json!(["10.42.0.0/16", "10.43.0.0/16"])
    );
    assert_eq!(ports_of(external), [443]);
    assert_eq!(all.len(), 6);
    // The warning names the host that is only port-restricted.
    assert!(run.err.contains("acme.atlassian.net:443"), "{}", run.err);
    assert!(run.err.contains("--egress-strict"), "{}", run.err);
    assert!(
        run.out.contains("port 8080 is port 9090 of its pods"),
        "{}",
        run.out
    );
    // The callers' policy stays what it was.
    assert_eq!(
        env.fake.object(CALLERS_POLICY).unwrap()["spec"]["policyTypes"],
        json!(["Ingress"])
    );
}

#[tokio::test]
async fn strict_mode_leaves_the_external_names_out_and_says_they_are_blocked() {
    let env = added().await;
    let dir = TestDir::new("egress-strict");
    let spec = write_spec(&dir);
    let run = deploy(&env, &spec, &["--egress", "--egress-strict"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    let policy = env.fake.object(POLICY).unwrap();
    assert!(
        rules(&policy)
            .iter()
            .all(|rule| rule["to"][0]["ipBlock"]["cidr"] != "0.0.0.0/0")
    );
    assert!(
        run.out.contains("blocked: acme.atlassian.net:443"),
        "{}",
        run.out
    );
    assert!(
        run.out.contains("(3 destinations, 0 only port-restricted)"),
        "{}",
        run.out
    );
    assert!(
        !run.err.contains("only be restricted by port"),
        "{}",
        run.err
    );
}

#[tokio::test]
async fn a_service_that_is_not_there_is_allowed_by_its_port_with_a_warning() {
    let env = added().await;
    env.fake.remove("/api/v1/namespaces/team/services/peer");
    let dir = TestDir::new("egress-no-service");
    let spec = write_spec(&dir);
    let run = deploy(&env, &spec, &["--egress", "--egress-strict"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.err.contains("Service team/peer not found"),
        "{}",
        run.err
    );
    let policy = env.fake.object(POLICY).unwrap();
    assert_eq!(ports_of(&namespace_rule(&policy, "team").unwrap()), [8080]);
}

/// An agent whose only destinations (besides its model, an Ollama by IP) are `fetch` hosts.
fn write_fetch_spec(dir: &TestDir, hosts: &[&str]) -> PathBuf {
    std::fs::write(
        dir.join("reviewer.json"),
        json!({"spec_version": 1, "name": "reviewer",
            "model": {"provider": "ollama", "id": "m"},
            "builtin_tools": {"tools": ["fetch"], "fetch": {"allowed_hosts": hosts}},
            "strategy": {"kind": "react"}})
        .to_string(),
    )
    .unwrap();
    dir.join("reviewer.json")
}

fn service(namespace: &str, name: &str, ports: Value) -> (String, Value) {
    (
        format!("/api/v1/namespaces/{namespace}/services/{name}"),
        json!({"apiVersion": "v1", "kind": "Service",
            "metadata": {"name": name, "namespace": namespace}, "spec": {"ports": ports}}),
    )
}

#[tokio::test]
async fn a_fetch_host_in_the_cluster_is_allowed_on_every_pod_port_of_its_service() {
    let env = added().await;
    let (path, object) = service(
        "target",
        "target",
        json!([{"port": 8080, "targetPort": 8080}, {"port": 8443, "targetPort": 8443},
            {"port": 9000, "targetPort": 9001}]),
    );
    env.fake.insert(&path, object);
    let dir = TestDir::new("egress-fetch-service");
    let spec = write_fetch_spec(&dir, &["target.target.svc", "example.com"]);
    let run = deploy(&env, &spec, &["--egress"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    let policy = env.fake.object(POLICY).unwrap();
    assert_eq!(
        ports_of(&namespace_rule(&policy, "target").unwrap()),
        [8080, 8443, 9001]
    );
    assert!(!run.err.contains("has no port"), "{}", run.err);
    assert!(run.out.contains("any port"), "{}", run.out);
}

#[tokio::test]
async fn a_wildcard_fetch_host_allows_the_namespace_without_a_port_restriction() {
    let env = added().await;
    let dir = TestDir::new("egress-fetch-wildcard");
    let spec = write_fetch_spec(&dir, &["*.target.svc"]);
    let run = deploy(&env, &spec, &["--egress"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    let rule = namespace_rule(&env.fake.object(POLICY).unwrap(), "target").unwrap();
    assert!(rule.get("ports").is_none(), "{rule}");
}

#[tokio::test]
async fn a_fetch_service_that_is_not_there_allows_the_namespace_with_a_warning() {
    let env = added().await;
    let dir = TestDir::new("egress-fetch-no-service");
    let spec = write_fetch_spec(&dir, &["target.target.svc"]);
    let run = deploy(&env, &spec, &["--egress"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.err.contains("Service target/target not found"),
        "{}",
        run.err
    );
    let rule = namespace_rule(&env.fake.object(POLICY).unwrap(), "target").unwrap();
    assert!(rule.get("ports").is_none(), "{rule}");
}

#[tokio::test]
async fn the_cluster_ranges_and_the_dns_pods_are_configurable() {
    let env = added().await;
    let dir = TestDir::new("egress-unknown");
    let spec = write_spec(&dir);
    // Without --env the Ollama is `localhost`: inside the pod, so no rule.
    let run = env
        .run(
            &[
                "agent",
                "deploy",
                "reviewer",
                "--image",
                IMAGE,
                "--spec",
                spec.to_str().unwrap(),
                "--egress",
                "--cluster-cidr",
                "10.0.0.0/8",
                "--egress-dns-namespace",
                "dns",
                "--egress-dns-selector",
                "app=coredns",
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    let policy = env.fake.object(POLICY).unwrap();
    let all = rules(&policy);
    assert_eq!(
        all[0]["to"][0]["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"],
        "dns"
    );
    assert_eq!(
        all[0]["to"][0]["podSelector"]["matchLabels"],
        json!({"app": "coredns"})
    );
    let external = all
        .iter()
        .find(|rule| rule["to"][0]["ipBlock"]["cidr"] == "0.0.0.0/0")
        .unwrap();
    assert_eq!(
        external["to"][0]["ipBlock"]["except"],
        json!(["10.0.0.0/8"])
    );
}

#[tokio::test]
async fn egress_needs_a_spec() {
    let env = added().await;
    let run = env
        .run(
            &["agent", "deploy", "reviewer", "--image", IMAGE, "--egress"],
            "",
        )
        .await;
    assert_eq!(run.code, 1);
    assert!(run.err.contains("--egress needs --spec"), "{}", run.err);
    assert!(env.fake.object(DEPLOYMENT).is_none());
}

#[tokio::test]
async fn a_redeploy_updates_the_policy_and_one_without_egress_deletes_it_without_a_restart() {
    let env = added().await;
    let dir = TestDir::new("egress-lifecycle");
    let spec = write_spec(&dir);
    deploy(&env, &spec, &["--egress"]).await;
    let hash =
        env.fake.object(DEPLOYMENT).unwrap()["metadata"]["annotations"]["grauwolf.io/applied-hash"]
            .clone();

    let again = deploy(&env, &spec, &["--egress"]).await;
    assert!(
        again.out.contains("grauwolf-egress") && again.out.contains(": unchanged"),
        "{}",
        again.out
    );

    let strict = deploy(&env, &spec, &["--egress", "--egress-strict"]).await;
    assert!(
        strict.out.contains("grauwolf-egress") && strict.out.contains(": updated"),
        "{}",
        strict.out
    );

    let off = deploy(&env, &spec, &[]).await;
    assert!(
        off.out
            .contains("NetworkPolicy reviewer/grauwolf-egress: deleted (no --egress)"),
        "{}",
        off.out
    );
    assert!(env.fake.object(POLICY).is_none());
    // The policy is not in the pod template: no restart, the Deployment is the same.
    assert_eq!(
        env.fake.object(DEPLOYMENT).unwrap()["metadata"]["annotations"]["grauwolf.io/applied-hash"],
        hash
    );
    assert!(
        off.out.contains("Deployment reviewer/reviewer: unchanged"),
        "{}",
        off.out
    );
}

#[tokio::test]
async fn undeploy_removes_the_egress_policy_but_keeps_the_callers_policy() {
    let env = added().await;
    let dir = TestDir::new("egress-undeploy");
    let spec = write_spec(&dir);
    deploy(&env, &spec, &["--egress"]).await;
    let run = env.run(&["agent", "undeploy", "reviewer"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out.contains("NetworkPolicy grauwolf-egress"),
        "{}",
        run.out
    );
    assert!(env.fake.object(POLICY).is_none());
    assert!(env.fake.object(CALLERS_POLICY).is_some());
}

#[tokio::test]
async fn status_and_doctor_show_the_policy_and_the_port_restricted_hosts() {
    let env = added().await;
    let dir = TestDir::new("egress-status");
    let spec = write_spec(&dir);
    deploy(&env, &spec, &[]).await;
    let off = env.run(&["agent", "status", "reviewer"], "").await;
    assert!(off.out.contains("egress: off"), "{}", off.out);

    deploy(&env, &spec, &["--egress"]).await;
    let status = env.run(&["agent", "status", "reviewer"], "").await;
    assert!(
        status
            .out
            .contains("egress: 4 destinations, 1 only port-restricted"),
        "{}",
        status.out
    );
    let json = env
        .run(&["agent", "status", "reviewer", "--json"], "")
        .await;
    let value: Value = serde_json::from_str(&json.out).unwrap();
    assert_eq!(value["egress"]["destinations"], 4);
    assert_eq!(
        value["egress"]["port_restricted"],
        json!(["acme.atlassian.net:443"])
    );

    let doctor = env.run(&["doctor", "--json"], "").await;
    let checks: Vec<Value> = serde_json::from_str(&doctor.out).unwrap();
    let line = checks
        .iter()
        .find(|check| check["check"] == "agent reviewer: egress policy")
        .unwrap();
    assert_eq!(line["status"], "warn");
    assert!(
        line["reason"]
            .as_str()
            .unwrap()
            .contains("acme.atlassian.net:443"),
        "{line}"
    );

    deploy(&env, &spec, &["--egress", "--egress-strict"]).await;
    let doctor = env.run(&["doctor", "--json"], "").await;
    let checks: Vec<Value> = serde_json::from_str(&doctor.out).unwrap();
    let line = checks
        .iter()
        .find(|check| check["check"] == "agent reviewer: egress policy")
        .unwrap();
    assert_eq!(line["status"], "ok");
}

/// Found in the live check: the Services `agent deploy` writes name their target ports (`http`,
/// `mtls`), so a fetch host that is another agent of the cluster opened the whole namespace. The
/// names are resolved through the container ports of the Deployment the Service selects.
#[tokio::test]
async fn named_target_ports_are_resolved_through_the_deployment_the_service_selects() {
    let env = added().await;
    let (path, mut object) = service(
        "target",
        "target",
        json!([{"name": "http", "port": 8080, "targetPort": "http"},
            {"name": "mtls", "port": 8443, "targetPort": "mtls"}]),
    );
    object["spec"]["selector"] = json!({"grauwolf.io/agent": "target"});
    env.fake.insert(&path, object);
    env.fake.insert(
        "/apis/apps/v1/namespaces/target/deployments/target",
        json!({"apiVersion": "apps/v1", "kind": "Deployment",
            "metadata": {"name": "target", "namespace": "target"},
            "spec": {"template": {
                "metadata": {"labels": {"grauwolf.io/agent": "target", "app.kubernetes.io/name": "grauwolf"}},
                "spec": {"containers": [{"name": "grauwolf", "ports": [
                    {"name": "http", "containerPort": 8080}, {"name": "mtls", "containerPort": 8443}]}]}}}}),
    );
    let dir = TestDir::new("egress-fetch-named-ports");
    let spec = write_fetch_spec(&dir, &["target.target.svc"]);
    let run = deploy(&env, &spec, &["--egress"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    let rule = namespace_rule(&env.fake.object(POLICY).unwrap(), "target").unwrap();
    assert_eq!(ports_of(&rule), [8080, 8443], "{rule}");
    assert!(!run.err.contains("names its target port"), "{}", run.err);
}

#[tokio::test]
async fn a_fetch_host_behind_an_external_ip_of_a_service_is_allowed_to_its_pods_too() {
    let env = added().await;
    env.dns
        .lock()
        .unwrap()
        .insert("example.com".to_owned(), vec!["10.0.0.7".parse().unwrap()]);
    let (path, mut object) = service(
        "edge",
        "gateway",
        json!([{"port": 443, "targetPort": 8443}]),
    );
    object["spec"]["selector"] = json!({"app": "gateway"});
    object["spec"]["externalIPs"] = json!(["10.0.0.7"]);
    env.fake.insert(&path, object);
    let dir = TestDir::new("egress-fetch-external-ip");
    let spec = write_fetch_spec(&dir, &["example.com"]);
    let run = deploy(&env, &spec, &["--egress"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out.contains(
            "egress: example.com resolves to the LoadBalancer of edge/gateway; allowing its pods (port 8443)"
        ),
        "{}",
        run.out
    );
    let policy = env.fake.object(POLICY).unwrap();
    let pods = rules(&policy)
        .into_iter()
        .find(|rule| rule["to"][0]["podSelector"]["matchLabels"]["app"] == "gateway")
        .expect("a rule for the gateway pods");
    assert_eq!(ports_of(&pods), [8443]);
    // The external rule on 443 stays.
    assert!(
        rules(&policy)
            .iter()
            .any(|rule| rule["to"][0]["ipBlock"]["cidr"] == "0.0.0.0/0"),
        "{policy}"
    );
}
