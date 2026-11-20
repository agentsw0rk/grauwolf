//! The commands against a fake Kubernetes API server and a fake OpenBao.

mod common;

use common::{DOMAIN, env};
use grauwolf_k8s::testing::JobOutcome;
use serde_json::{Value, json};

const SECRET: &str = "jira-s3cret-value-123";
const NS: &str = "/api/v1/namespaces/reviewer";

fn subject() -> String {
    format!("spiffe://{DOMAIN}/ns/reviewer/sa/reviewer")
}

#[tokio::test]
async fn oidc_install_creates_the_provider_with_the_image_of_the_running_server() {
    let env = env();
    let run = env.run(&["oidc", "install"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out.contains("trust domain agentswork.de, SPIRE 1.15.5"),
        "{}",
        run.out
    );
    let deployment = env
        .fake
        .object("/apis/apps/v1/namespaces/spire-oidc/deployments/spire-oidc-discovery-provider")
        .unwrap();
    assert_eq!(
        deployment["spec"]["template"]["spec"]["containers"][0]["image"],
        "ghcr.io/spiffe/oidc-discovery-provider:1.15.5"
    );
    let config = env
        .fake
        .object("/api/v1/namespaces/spire-oidc/configmaps/spire-oidc-discovery-provider")
        .unwrap();
    assert!(
        config["data"]["oidc-discovery-provider.conf"]
            .as_str()
            .unwrap()
            .contains("trust_domain = \"agentswork.de\"")
    );
    assert!(
        env.fake
            .object("/apis/spire.spiffe.io/v1alpha1/clusterspiffeids/spire-oidc-discovery-provider")
            .is_some()
    );
}

#[tokio::test]
async fn oidc_install_twice_changes_nothing() {
    let env = env();
    env.run(&["oidc", "install"], "").await;
    let writes = env.fake.writes().len();
    let again = env.run(&["oidc", "install"], "").await;
    assert_eq!(again.code, 0);
    assert_eq!(env.fake.writes().len(), writes);
    assert!(
        again
            .out
            .contains("Deployment spire-oidc-discovery-provider: unchanged"),
        "{}",
        again.out
    );
}

#[tokio::test]
async fn oidc_install_adopts_a_provider_made_from_the_example() {
    let env = env();
    let path = "/apis/apps/v1/namespaces/spire-oidc/deployments/spire-oidc-discovery-provider";
    env.fake.insert(
        path,
        json!({"kind": "Deployment", "metadata": {"name": "spire-oidc-discovery-provider"},
        "spec": {"replicas": 1}}),
    );
    let run = env.run(&["oidc", "install"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out
            .contains("Deployment spire-oidc-discovery-provider: updated"),
        "{}",
        run.out
    );
}

#[tokio::test]
async fn oidc_install_without_spire_says_so() {
    let env = env();
    env.fake.insert(
        "/api/v1/namespaces/spire-system/configmaps/spire-server",
        json!({"data": {}}),
    );
    let run = env.run(&["oidc", "install"], "").await;
    assert_eq!(run.code, 1);
    assert!(run.err.contains("spire-server"), "{}", run.err);
}

#[tokio::test]
async fn openbao_setup_enables_jwt_and_kv_and_a_second_run_writes_nothing() {
    let env = env();
    let run = env.run(&["openbao", "setup"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    {
        let bao = env.bao.lock().unwrap();
        assert_eq!(bao.auth["jwt"], "jwt");
        assert_eq!(
            bao.jwt_config["jwt"]["jwks_url"],
            "http://spire-oidc-discovery-provider.spire-oidc.svc/keys"
        );
        assert_eq!(bao.mounts["kv"]["options"]["version"], "2");
    }
    let writes = env.bao_writes();
    let again = env.run(&["openbao", "setup"], "").await;
    assert_eq!(env.bao_writes(), writes);
    assert!(again.out.contains("unchanged"), "{}", again.out);
}

#[tokio::test]
async fn openbao_commands_need_a_token() {
    let mut env = env();
    env.token = None;
    let run = env.run(&["openbao", "setup"], "").await;
    assert_eq!(run.code, 1);
    assert!(run.err.contains("OPENBAO_TOKEN"), "{}", run.err);
}

#[tokio::test]
async fn agent_add_creates_everything_for_the_agent() {
    let env = env();
    env.prepare().await;
    let run = env.run(&["agent", "add", "reviewer"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());

    let namespace = env.fake.object(NS).unwrap();
    let labels = &namespace["metadata"]["labels"];
    assert_eq!(labels["grauwolf.io/agent"], "reviewer");
    assert_eq!(labels["app.kubernetes.io/managed-by"], "grauwolf-k8-ops");
    assert_eq!(labels["pod-security.kubernetes.io/enforce"], "restricted");
    assert!(
        env.fake
            .object(&format!("{NS}/serviceaccounts/reviewer"))
            .is_some()
    );
    let id = env
        .fake
        .object("/apis/spire.spiffe.io/v1alpha1/clusterspiffeids/grauwolf-agents")
        .unwrap();
    assert_eq!(
        id["spec"]["namespaceSelector"]["matchExpressions"][0]["key"],
        "grauwolf.io/agent"
    );
    let config = env
        .fake
        .object(&format!("{NS}/configmaps/grauwolf-config"))
        .unwrap();
    let toml = config["data"]["grauwolf.toml"].as_str().unwrap();
    assert!(toml.contains("role = \"agent-reviewer\""), "{toml}");
    assert!(
        toml.contains("address = \"http://openbao.openbao.svc:8200\""),
        "{toml}"
    );

    let bao = env.bao.lock().unwrap();
    let role = &bao.roles["agent-reviewer"];
    assert_eq!(role["bound_subject"], subject());
    assert_eq!(role["token_policies"], json!(["agent-reviewer"]));
    assert_eq!(role["bound_audiences"], json!(["openbao"]));
    let policy = &bao.policies["agent-reviewer"];
    assert!(
        policy.contains("path \"kv/data/agents/reviewer/*\""),
        "{policy}"
    );
    assert!(!policy.contains("agents/*"), "{policy}");
}

#[tokio::test]
async fn agent_add_twice_changes_nothing() {
    let env = env();
    env.prepare().await;
    env.run(&["agent", "add", "reviewer"], "").await;
    let (cluster_writes, bao_writes) = (env.fake.writes().len(), env.bao_writes());
    let again = env.run(&["agent", "add", "reviewer"], "").await;
    assert_eq!(again.code, 0, "{}", again.all());
    assert_eq!(env.fake.writes().len(), cluster_writes);
    assert_eq!(env.bao_writes(), bao_writes);
    assert!(
        again.out.contains("namespace reviewer: unchanged"),
        "{}",
        again.out
    );
}

#[tokio::test]
async fn agent_add_rejects_a_foreign_namespace_a_bad_name_and_a_missing_setup() {
    let env = env();
    let early = env.run(&["agent", "add", "reviewer"], "").await;
    assert_eq!(early.code, 1);
    assert!(early.err.contains("openbao setup"), "{}", early.err);

    env.prepare().await;
    env.fake.insert(
        "/api/v1/namespaces/default",
        json!({"kind": "Namespace", "metadata": {"name": "default"}}),
    );
    let foreign = env.run(&["agent", "add", "default"], "").await;
    assert_eq!(foreign.code, 1);
    assert!(
        foreign.err.contains("does not belong to agent default"),
        "{}",
        foreign.err
    );
    assert!(
        env.fake.object("/api/v1/namespaces/default").unwrap()["metadata"]
            .get("labels")
            .is_none()
    );

    let bad = env.run(&["agent", "add", "Not_Valid"], "").await;
    assert_eq!(bad.code, 1);
    assert!(bad.err.contains("not a valid agent name"), "{}", bad.err);
}

#[tokio::test]
async fn agent_add_copies_a_pull_secret_without_printing_it() {
    let env = env();
    env.prepare().await;
    env.fake.insert(
        "/api/v1/namespaces/default/secrets/registry",
        json!({"type": "kubernetes.io/dockerconfigjson", "data": {".dockerconfigjson": "c2VjcmV0LXRva2Vu"}}),
    );
    let run = env
        .run(
            &[
                "agent",
                "add",
                "reviewer",
                "--pull-secret",
                "default/registry",
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(!run.all().contains("c2VjcmV0"));
    let copy = env.fake.object(&format!("{NS}/secrets/registry")).unwrap();
    assert_eq!(copy["data"][".dockerconfigjson"], "c2VjcmV0LXRva2Vu");
    let account = env
        .fake
        .object(&format!("{NS}/serviceaccounts/reviewer"))
        .unwrap();
    assert_eq!(account["imagePullSecrets"][0]["name"], "registry");
}

#[tokio::test]
async fn a_pull_secret_that_already_exists_equal_is_not_written_but_attached() {
    // Kyverno clones the secret into every namespace and refuses writes that touch its labels.
    let env = env();
    env.prepare().await;
    let secret = json!({"type": "kubernetes.io/dockerconfigjson",
        "data": {".dockerconfigjson": "c2VjcmV0LXRva2Vu"}});
    env.fake.insert(
        "/api/v1/namespaces/default/secrets/registry",
        secret.clone(),
    );
    let mut clone = secret;
    clone["metadata"] = json!({"name": "registry", "namespace": "reviewer",
        "labels": {"generate.kyverno.io/clone-source": ""}});
    let path = format!("{NS}/secrets/registry");
    env.fake.insert(&path, clone);
    let run = env
        .run(
            &[
                "agent",
                "add",
                "reviewer",
                "--pull-secret",
                "default/registry",
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out
            .contains("pull secret reviewer/registry (copied from default/registry): unchanged"),
        "{}",
        run.out
    );
    assert!(
        !env.fake.writes().contains(&format!("PATCH {path}")),
        "{:?}",
        env.fake.writes()
    );
    let account = env
        .fake
        .object(&format!("{NS}/serviceaccounts/reviewer"))
        .unwrap();
    assert_eq!(account["imagePullSecrets"][0]["name"], "registry");
}

#[tokio::test]
async fn secret_set_reads_stdin_stores_the_value_and_never_prints_it() {
    let env = env();
    env.prepare().await;
    env.run(&["agent", "add", "reviewer"], "").await;
    let run = env
        .run(
            &[
                "secret",
                "set",
                "reviewer",
                "jira",
                "--variable",
                "ATLASSIAN_API_TOKEN",
            ],
            &format!("{SECRET}\n"),
        )
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(!run.all().contains(SECRET), "{}", run.all());
    assert_eq!(
        env.bao.lock().unwrap().kv["kv/agents/reviewer/jira"],
        vec![SECRET.to_owned()]
    );
    let config = env
        .fake
        .object(&format!("{NS}/configmaps/grauwolf-config"))
        .unwrap();
    assert!(
        config["data"]["grauwolf.toml"]
            .as_str()
            .unwrap()
            .contains("ATLASSIAN_API_TOKEN = \"openbao:kv/agents/reviewer/jira#value\"")
    );

    // `agent add` again keeps the mapped variable.
    env.run(&["agent", "add", "reviewer"], "").await;
    let config = env
        .fake
        .object(&format!("{NS}/configmaps/grauwolf-config"))
        .unwrap();
    assert!(
        config["data"]["grauwolf.toml"]
            .as_str()
            .unwrap()
            .contains("ATLASSIAN_API_TOKEN")
    );
}

#[tokio::test]
async fn secret_set_refuses_an_unknown_variable_and_an_empty_value() {
    let env = env();
    env.prepare().await;
    env.run(&["agent", "add", "reviewer"], "").await;
    let unknown = env
        .run(
            &[
                "secret",
                "set",
                "reviewer",
                "jira",
                "--variable",
                "NOT_A_VARIABLE",
            ],
            "x\n",
        )
        .await;
    assert_eq!(unknown.code, 1);
    assert!(env.bao.lock().unwrap().kv.is_empty(), "nothing is stored");
    let empty = env.run(&["secret", "set", "reviewer", "jira"], "\n").await;
    assert_eq!(empty.code, 1);
    assert!(empty.err.contains("empty"), "{}", empty.err);
}

#[tokio::test]
async fn an_error_while_storing_names_agent_and_key_but_not_the_value() {
    let env = env();
    env.prepare().await;
    env.bao.lock().unwrap().deny_kv_writes = true;
    let run = env
        .run(
            &["secret", "set", "reviewer", "jira"],
            &format!("{SECRET}\n"),
        )
        .await;
    assert_eq!(run.code, 1);
    assert!(
        run.err
            .contains("secret reviewer/jira: write: HTTP 403: permission denied"),
        "{}",
        run.err
    );
    assert!(!run.all().contains(SECRET));
}

#[tokio::test]
async fn secret_list_shows_keys_and_versions_and_no_values_and_delete_removes() {
    let env = env();
    env.prepare().await;
    env.run(&["agent", "add", "reviewer"], "").await;
    env.run(
        &["secret", "set", "reviewer", "jira"],
        &format!("{SECRET}\n"),
    )
    .await;
    env.run(&["secret", "set", "reviewer", "jira"], "other-value\n")
        .await;
    env.run(&["secret", "set", "reviewer", "git"], "g\n").await;

    let list = env.run(&["secret", "list", "reviewer"], "").await;
    assert_eq!(list.code, 0, "{}", list.all());
    assert!(list.out.contains("jira  version 2"), "{}", list.out);
    assert!(list.out.contains("git  version 1"), "{}", list.out);
    assert!(!list.all().contains(SECRET) && !list.all().contains("other-value"));

    let json = env.run(&["secret", "list", "reviewer", "--json"], "").await;
    let rows: Value = serde_json::from_str(&json.out).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 2);

    let deleted = env.run(&["secret", "delete", "reviewer", "jira"], "").await;
    assert_eq!(deleted.code, 0, "{}", deleted.all());
    assert!(
        !env.bao
            .lock()
            .unwrap()
            .kv
            .contains_key("kv/agents/reviewer/jira")
    );
    let again = env.run(&["secret", "delete", "reviewer", "jira"], "").await;
    assert_eq!(again.code, 1);
    assert!(
        again.err.contains("reviewer/jira not found"),
        "{}",
        again.err
    );
}

#[tokio::test]
async fn agent_list_shows_the_agents_and_their_identity() {
    let env = env();
    env.prepare().await;
    env.run(&["agent", "add", "reviewer"], "").await;
    env.run(&["agent", "add", "coder", "--namespace", "coding"], "")
        .await;
    let list = env.run(&["agent", "list"], "").await;
    assert!(
        list.out
            .contains(&format!("reviewer  namespace reviewer  {}", subject())),
        "{}",
        list.out
    );
    assert!(
        list.out.contains(&format!(
            "coder  namespace coding  spiffe://{DOMAIN}/ns/coding/sa/coder"
        )),
        "{}",
        list.out
    );
    let json: Value =
        serde_json::from_str(&env.run(&["agent", "list", "--json"], "").await.out).unwrap();
    assert_eq!(json[0]["agent"], "coder");
}

#[tokio::test]
async fn agent_remove_deletes_namespace_role_and_policy_and_keeps_secrets_unless_purged() {
    let env = env();
    env.prepare().await;
    env.run(&["agent", "add", "reviewer"], "").await;
    env.run(&["secret", "set", "reviewer", "jira"], "v\n").await;

    let cancelled = env.run(&["agent", "remove", "reviewer"], "n\n").await;
    assert_eq!(cancelled.code, 1);
    assert!(
        env.fake.object(NS).is_some(),
        "nothing removed without a yes"
    );

    let run = env.run(&["agent", "remove", "reviewer", "--yes"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(run.out.contains("kept: 1 secret(s)"), "{}", run.out);
    assert!(env.fake.object(NS).is_none());
    assert!(env.fake.paths(&format!("{NS}/")).is_empty());
    {
        let bao = env.bao.lock().unwrap();
        assert!(bao.roles.is_empty() && bao.policies.keys().all(|name| name == "grauwolf-k8-ops"));
        assert!(bao.kv.contains_key("kv/agents/reviewer/jira"));
    }

    env.run(&["agent", "add", "reviewer"], "").await;
    let purge = env
        .run(&["agent", "remove", "reviewer", "--purge-secrets"], "yes\n")
        .await;
    assert_eq!(purge.code, 0, "{}", purge.all());
    assert!(env.bao.lock().unwrap().kv.is_empty());
}

#[tokio::test]
async fn agent_remove_leaves_a_foreign_namespace_alone() {
    let env = env();
    env.prepare().await;
    env.fake.insert(
        "/api/v1/namespaces/default",
        json!({"kind": "Namespace", "metadata": {"name": "default"}}),
    );
    let run = env.run(&["agent", "remove", "default", "--yes"], "").await;
    assert_eq!(run.code, 1);
    assert!(env.fake.object("/api/v1/namespaces/default").is_some());
}

async fn ready_agent() -> common::Env {
    let env = env();
    env.prepare().await;
    env.run(&["agent", "add", "reviewer"], "").await;
    env.run(
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
    env
}

#[tokio::test]
async fn agent_check_prints_the_last_log_line_and_deletes_the_job() {
    let env = ready_agent().await;
    env.fake.set_job_outcome(JobOutcome::Succeeds(
        "starting\nresolved ATLASSIAN_API_TOKEN from openbao (1 bytes)\n".into(),
    ));
    let run = env
        .run(
            &[
                "agent",
                "check",
                "reviewer",
                "--image",
                "registry.example/grauwolf:1",
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out
            .contains("resolved ATLASSIAN_API_TOKEN from openbao (1 bytes)"),
        "{}",
        run.out
    );
    let jobs = env
        .fake
        .deleted()
        .into_iter()
        .filter(|path| path.contains("/jobs/grauwolf-check-"))
        .count();
    assert_eq!(jobs, 1);
    assert!(env.fake.paths(&format!("{NS}/jobs/")).is_empty());
}

#[test]
fn the_check_job_fits_the_restricted_standard() {
    let job = grauwolf_k8_ops_app::check_job(
        "grauwolf-check-abc",
        "reviewer",
        "reviewer",
        "img:1",
        &["ATLASSIAN_API_TOKEN".to_owned(), "GIT_TOKEN".to_owned()],
        120,
    );
    assert_eq!(
        job["spec"]["template"]["spec"]["containers"][0]["command"],
        json!([
            "grauwolf",
            "credentials",
            "check",
            "ATLASSIAN_API_TOKEN",
            "GIT_TOKEN",
            "--config",
            "/etc/grauwolf/grauwolf.toml"
        ])
    );
    let pod = &job["spec"]["template"]["spec"];
    assert_eq!(pod["securityContext"]["runAsUser"], 65532);
    assert_eq!(pod["securityContext"]["runAsNonRoot"], true);
    assert_eq!(
        pod["securityContext"]["seccompProfile"]["type"],
        "RuntimeDefault"
    );
    assert_eq!(
        pod["containers"][0]["securityContext"]["capabilities"]["drop"][0],
        "ALL"
    );
    assert_eq!(
        pod["containers"][0]["securityContext"]["allowPrivilegeEscalation"],
        false
    );
    assert_eq!(pod["serviceAccountName"], "reviewer");
}

#[tokio::test]
async fn a_failed_check_is_reported_with_exit_code_1_and_the_job_is_deleted() {
    let env = ready_agent().await;
    env.fake
        .set_job_outcome(JobOutcome::Fails("error: login failed\n".into()));
    let run = env
        .run(&["agent", "check", "reviewer", "--image", "img:1"], "")
        .await;
    assert_eq!(run.code, 1);
    assert!(
        run.out.contains("error: login failed") && run.out.contains("the check failed"),
        "{}",
        run.out
    );
    assert!(env.fake.paths(&format!("{NS}/jobs/")).is_empty());
}

#[tokio::test]
async fn a_check_that_does_not_finish_times_out_and_still_deletes_the_job() {
    let env = ready_agent().await;
    env.fake.set_job_outcome(JobOutcome::Never);
    let run = env
        .run(
            &[
                "agent",
                "check",
                "reviewer",
                "--image",
                "img:1",
                "--timeout",
                "1",
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 1);
    assert!(run.out.contains("did not finish within 1 s"), "{}", run.out);
    assert!(env.fake.paths(&format!("{NS}/jobs/")).is_empty());
}

async fn map_variable(env: &common::Env, key: &str, variable: &str) {
    let run = env
        .run(
            &["secret", "set", "reviewer", key, "--variable", variable],
            "v\n",
        )
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
}

fn check_job_command(env: &common::Env) -> Vec<String> {
    let jobs = env.fake.created_jobs();
    jobs[0]["spec"]["template"]["spec"]["containers"][0]["command"]
        .as_array()
        .unwrap()
        .iter()
        .map(|part| part.as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn agent_check_checks_every_mapped_variable_in_one_job_and_shows_each_line() {
    let env = ready_agent().await;
    map_variable(&env, "git", "GIT_TOKEN").await;
    env.fake.set_job_outcome(JobOutcome::Succeeds(
        "resolved ATLASSIAN_API_TOKEN from openbao (1 bytes)\nresolved GIT_TOKEN from openbao (1 bytes)\n".into(),
    ));

    let run = env
        .run(&["agent", "check", "reviewer", "--image", "img:1"], "")
        .await;

    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out.contains("check of ATLASSIAN_API_TOKEN, GIT_TOKEN"),
        "{}",
        run.out
    );
    assert!(
        run.out
            .contains("resolved ATLASSIAN_API_TOKEN from openbao"),
        "{}",
        run.out
    );
    assert!(
        run.out.contains("resolved GIT_TOKEN from openbao"),
        "{}",
        run.out
    );
    assert_eq!(
        check_job_command(&env)[3..5],
        ["ATLASSIAN_API_TOKEN".to_owned(), "GIT_TOKEN".to_owned()]
    );
    assert_eq!(env.fake.created_jobs().len(), 1);
}

#[tokio::test]
async fn agent_check_variable_narrows_the_check_and_refuses_an_unmapped_name() {
    let env = ready_agent().await;
    map_variable(&env, "git", "GIT_TOKEN").await;
    env.fake.set_job_outcome(JobOutcome::Succeeds(
        "resolved GIT_TOKEN from openbao (1 bytes)\n".into(),
    ));

    let run = env
        .run(
            &[
                "agent",
                "check",
                "reviewer",
                "--image",
                "img:1",
                "--variable",
                "GIT_TOKEN",
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(check_job_command(&env)[3], "GIT_TOKEN");
    assert_eq!(check_job_command(&env).len(), 6);

    let run = env
        .run(
            &[
                "agent",
                "check",
                "reviewer",
                "--image",
                "img:1",
                "--variable",
                "OPENAI_API_KEY",
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 1);
    assert!(
        run.err.contains("OPENAI_API_KEY is not mapped"),
        "{}",
        run.err
    );
}

#[tokio::test]
async fn a_mixed_check_exits_1_and_a_permission_denied_gets_the_hint_with_the_tried_path() {
    let env = ready_agent().await;
    // A variable that maps outside the agent's prefix, as written by hand.
    let mut config = env
        .fake
        .object("/api/v1/namespaces/reviewer/configmaps/grauwolf-config")
        .unwrap();
    let text = config["data"]["grauwolf.toml"].as_str().unwrap().to_owned();
    config["data"]["grauwolf.toml"] = json!(format!(
        "{text}\nGIT_TOKEN = \"openbao:kv/agents/other/jira#value\"\n"
    ));
    env.fake.insert(
        "/api/v1/namespaces/reviewer/configmaps/grauwolf-config",
        config,
    );
    env.fake.set_job_outcome(JobOutcome::Fails(
        "resolved ATLASSIAN_API_TOKEN from openbao (1 bytes)\ncredentials: GIT_TOKEN from openbao:kv/agents/other/jira#value: read: HTTP 403: permission denied\n".into(),
    ));

    let run = env
        .run(&["agent", "check", "reviewer", "--image", "img:1"], "")
        .await;

    assert_eq!(run.code, 1, "{}", run.all());
    assert!(
        run.out.contains("resolved ATLASSIAN_API_TOKEN"),
        "{}",
        run.out
    );
    assert!(
        run.out
            .contains("GIT_TOKEN from openbao:kv/agents/other/jira#value"),
        "{}",
        run.out
    );
    assert!(
        run.out.contains(
            "hint: the policy agent-reviewer of the agent only reads kv/data/agents/reviewer/*"
        ) && run.out.contains("reads kv/data/agents/other/jira"),
        "{}",
        run.out
    );
    assert_eq!(
        run.out.matches("hint:").count(),
        1,
        "no hint for the line that resolved"
    );
}

#[tokio::test]
async fn agent_check_needs_a_mapped_variable() {
    let env = env();
    env.prepare().await;
    env.run(&["agent", "add", "reviewer"], "").await;
    let run = env
        .run(&["agent", "check", "reviewer", "--image", "img:1"], "")
        .await;
    assert_eq!(run.code, 1);
    assert!(run.err.contains("maps no variable"), "{}", run.err);
    assert!(env.fake.paths(&format!("{NS}/jobs/")).is_empty());
}

/// Found in the live check: right after `agent remove` the namespace is still terminating and
/// `agent list` showed the removed agent.
#[tokio::test]
async fn agent_list_leaves_out_agents_whose_namespace_is_being_deleted() {
    let env = env();
    env.prepare().await;
    env.run(&["agent", "add", "reviewer"], "").await;
    env.fake.insert(
        "/api/v1/namespaces/writer",
        json!({"kind": "Namespace", "metadata": {
            "name": "writer",
            "deletionTimestamp": "2026-11-14T17:00:00Z",
            "labels": {"grauwolf.io/agent": "writer", "app.kubernetes.io/managed-by": "grauwolf-k8-ops"}
        }}),
    );
    let list = env.run(&["agent", "list"], "").await;
    assert!(list.out.contains("reviewer"), "{}", list.out);
    assert!(!list.out.contains("writer"), "{}", list.out);
}

#[tokio::test]
async fn secret_get_refuses_without_reveal_and_prints_only_the_value_with_it() {
    let env = ready_agent().await;
    env.run(
        &["secret", "set", "reviewer", "pw"],
        "  s3cret with spaces \n",
    )
    .await;

    let refused = env.run(&["secret", "get", "reviewer", "pw"], "").await;
    assert_eq!(refused.code, 1);
    assert!(refused.out.is_empty(), "{}", refused.out);
    assert!(
        refused.err.contains("--reveal") && !refused.all().contains("s3cret"),
        "{}",
        refused.err
    );

    let run = env
        .run(&["secret", "get", "reviewer", "pw", "--reveal"], "")
        .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(run.out, "  s3cret with spaces \n");
    assert!(run.err.is_empty(), "{}", run.err);

    let missing = env
        .run(&["secret", "get", "reviewer", "nope", "--reveal"], "")
        .await;
    assert_eq!(missing.code, 1);
    assert!(missing.err.contains("reviewer/nope"), "{}", missing.err);
    assert!(missing.out.is_empty());
}
