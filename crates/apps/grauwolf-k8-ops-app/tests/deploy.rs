//! `agent deploy | status | logs | undeploy` and the deployed agents in `doctor` (ADR-0121),
//! against a fake Kubernetes API server and a fake OpenBao.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use common::{Env, Run, env};
use grauwolf_k8s::testing::DeploymentOutcome;
use grauwolf_test::TestDir;
use serde_json::{Value, json};

const NS: &str = "/api/v1/namespaces/reviewer";
const DEPLOYMENT: &str = "/apis/apps/v1/namespaces/reviewer/deployments/reviewer";
const IMAGE: &str = "registry.example.com/grauwolf:1.0";

const SKILL: &str = "---\nname: greeter\ndescription: Greets people. Use it when someone says hello.\n---\nSay hello.\n";

/// A spec with a prompt in its folder, a skill directory and a sub-agent.
fn write_tree(dir: &TestDir) -> PathBuf {
    std::fs::create_dir_all(dir.join("skills/greeter")).unwrap();
    std::fs::write(dir.join("skills/greeter/SKILL.md"), SKILL).unwrap();
    std::fs::write(
        dir.join("reviewer.json"),
        json!({"spec_version": 1, "name": "reviewer",
            "model": {"provider": "ollama", "id": "qwen3.5:9b"},
            "system_prompt": "You review.", "skills": [{"dir": "skills"}],
            "strategy": {"kind": "single_run"}})
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
    env
}

async fn deploy(env: &Env, extra: &[&str]) -> Run {
    let mut args = vec!["agent", "deploy", "reviewer", "--image", IMAGE];
    args.extend(extra);
    env.run(&args, "").await
}

fn spec_arg(file: &Path) -> String {
    file.display().to_string()
}

fn pod_spec(env: &Env) -> Value {
    env.fake.object(DEPLOYMENT).expect("deployment")["spec"]["template"]["spec"].clone()
}

fn env_of(pod: &Value, name: &str) -> Option<String> {
    pod["containers"][0]["env"]
        .as_array()?
        .iter()
        .find(|entry| entry["name"] == name)
        .and_then(|entry| entry["value"].as_str().map(str::to_owned))
}

#[tokio::test]
async fn deploy_with_a_spec_makes_the_deployment_service_and_files_and_waits_for_health() {
    let env = added().await;
    let dir = TestDir::new("deploy-spec");
    let file = write_tree(&dir);
    let run = deploy(
        &env,
        &[
            "--spec",
            &spec_arg(&file),
            "--env",
            "OLLAMA_HOST=http://ollama.ollama.svc:11434",
        ],
    )
    .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out.contains("Deployment reviewer/reviewer: created"),
        "{}",
        run.out
    );
    assert!(run.out.contains("ready, /health answers ok"), "{}", run.out);
    assert!(
        run.out
            .contains("endpoint http://reviewer.reviewer.svc:8080"),
        "{}",
        run.out
    );

    let deployment = env.fake.object(DEPLOYMENT).unwrap();
    assert_eq!(deployment["spec"]["replicas"], 1);
    assert_eq!(deployment["spec"]["strategy"]["type"], "Recreate");
    assert_eq!(
        deployment["metadata"]["labels"]["app.kubernetes.io/managed-by"],
        "grauwolf-k8-ops"
    );
    assert_eq!(
        deployment["spec"]["selector"]["matchLabels"]["grauwolf.io/agent"],
        "reviewer"
    );
    let pod = pod_spec(&env);
    assert_eq!(pod["serviceAccountName"], "reviewer");
    // The restricted Pod Security Standard of ADR-0120.
    assert_eq!(pod["securityContext"]["runAsNonRoot"], true);
    assert_eq!(pod["securityContext"]["runAsUser"], 65532);
    assert_eq!(
        pod["securityContext"]["seccompProfile"]["type"],
        "RuntimeDefault"
    );
    let container = &pod["containers"][0];
    assert_eq!(
        container["securityContext"]["allowPrivilegeEscalation"],
        false
    );
    assert_eq!(
        container["securityContext"]["capabilities"]["drop"],
        json!(["ALL"])
    );
    assert_eq!(container["image"], IMAGE);
    assert_eq!(
        container["command"],
        json!([
            "grauwolf",
            "spec",
            "serve",
            "/app/spec/reviewer.json",
            "--addr",
            "0.0.0.0:8080"
        ])
    );
    assert_eq!(container["ports"][0]["containerPort"], 8080);
    assert_eq!(container["readinessProbe"]["httpGet"]["path"], "/health");
    assert_eq!(container["livenessProbe"]["httpGet"]["path"], "/health");
    assert!(container.get("resources").is_none());
    // The identity, the configuration and the environment.
    assert!(pod["volumes"].as_array().unwrap().contains(&json!({
        "name": "spiffe-workload-api", "csi": {"driver": "csi.spiffe.io", "readOnly": true}})));
    assert_eq!(
        env_of(&pod, "SPIFFE_ENDPOINT_SOCKET").as_deref(),
        Some("unix:///spiffe-workload-api/agent.sock")
    );
    assert_eq!(
        env_of(&pod, "GRAUWOLF_CREDENTIALS").as_deref(),
        Some("/etc/grauwolf/grauwolf.toml")
    );
    assert_eq!(
        env_of(&pod, "OLLAMA_HOST").as_deref(),
        Some("http://ollama.ollama.svc:11434")
    );
    let mounts = container["volumeMounts"].as_array().unwrap();
    assert!(
        mounts.contains(&json!({"name": "config", "mountPath": "/etc/grauwolf", "readOnly": true}))
    );
    assert!(
        pod["volumes"]
            .as_array()
            .unwrap()
            .contains(&json!({"name": "config", "configMap": {"name": "grauwolf-config"}}))
    );
    // The agent's files, one plain file mount each, read-only.
    let paths: Vec<&str> = mounts
        .iter()
        .filter(|mount| mount["name"] == "agent")
        .map(|mount| mount["mountPath"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths,
        [
            "/app/spec/reviewer.json",
            "/app/spec/skills/greeter/SKILL.md"
        ]
    );
    assert!(
        mounts
            .iter()
            .filter(|m| m["name"] == "agent")
            .all(|m| m["readOnly"] == true
                && m["subPath"].as_str().is_some_and(|key| key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))))
    );
    // The values that are not secret are in the Deployment, no Secret was made for them.
    assert!(env.fake.paths(&format!("{NS}/secrets")).is_empty());

    let files = env
        .fake
        .object(&format!("{NS}/configmaps/grauwolf-agent"))
        .unwrap();
    let data = files["data"].as_object().unwrap();
    assert_eq!(data.len(), 2);
    assert!(data.values().any(|text| text.as_str() == Some(SKILL)));

    let service = env.fake.object(&format!("{NS}/services/reviewer")).unwrap();
    assert_eq!(service["spec"]["type"], "ClusterIP");
    assert_eq!(service["spec"]["ports"][0]["port"], 8080);
    assert_eq!(service["spec"]["selector"]["grauwolf.io/agent"], "reviewer");
    assert!(
        env.fake
            .object("/apis/networking.k8s.io/v1/namespaces/reviewer/ingresses/reviewer")
            .is_none()
    );
}

#[tokio::test]
async fn deploy_without_a_spec_runs_the_image_as_it_is() {
    let env = added().await;
    let run = deploy(&env, &["--port", "9000"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    let pod = pod_spec(&env);
    let container = &pod["containers"][0];
    assert!(
        container.get("command").is_none(),
        "the entrypoint of the image runs"
    );
    assert_eq!(env_of(&pod, "SERVER_PORT").as_deref(), Some("9000"));
    assert_eq!(container["ports"][0]["containerPort"], 9000);
    assert!(
        env.fake
            .object(&format!("{NS}/configmaps/grauwolf-agent"))
            .is_none()
    );
    let service = env.fake.object(&format!("{NS}/services/reviewer")).unwrap();
    assert_eq!(service["spec"]["ports"][0]["port"], 9000);
}

#[tokio::test]
async fn a_repeated_deploy_writes_nothing_and_a_changed_spec_changes_the_template_hash() {
    let env = added().await;
    let dir = TestDir::new("deploy-repeat");
    let file = write_tree(&dir);
    let args = ["--spec", &spec_arg(&file)];
    assert_eq!(deploy(&env, &args).await.code, 0);
    let hash = |env: &Env| {
        env.fake.object(DEPLOYMENT).unwrap()["spec"]["template"]["metadata"]["annotations"]
            ["grauwolf.io/config-hash"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let first = hash(&env);
    let (writes, bao_writes) = (env.fake.writes().len(), env.bao_writes());

    let again = deploy(&env, &args).await;
    assert_eq!(again.code, 0, "{}", again.all());
    assert_eq!(env.fake.writes().len(), writes, "{:?}", env.fake.writes());
    assert_eq!(env.bao_writes(), bao_writes);
    assert!(
        again
            .out
            .contains("Deployment reviewer/reviewer: unchanged"),
        "{}",
        again.out
    );
    assert_eq!(hash(&env), first);

    std::fs::write(
        dir.join("skills/greeter/SKILL.md"),
        SKILL.replace("Say hello.", "Say hi."),
    )
    .unwrap();
    let changed = deploy(&env, &args).await;
    assert!(
        changed
            .out
            .contains("Deployment reviewer/reviewer: updated"),
        "{}",
        changed.out
    );
    assert_ne!(hash(&env), first);

    // The environment is part of what restarts the pod.
    let second = hash(&env);
    deploy(
        &env,
        &["--spec", &spec_arg(&file), "--env", "OLLAMA_THINK=false"],
    )
    .await;
    assert_ne!(hash(&env), second);
}

#[tokio::test]
async fn deploy_refuses_before_it_writes_anything() {
    let env = added().await;
    let dir = TestDir::new("deploy-refuse");
    let file = write_tree(&dir);
    let spec = spec_arg(&file);
    let refused = |run: &Run, text: &str| {
        assert_eq!(run.code, 1, "{}", run.all());
        assert!(run.err.contains(text), "wanted '{text}' in: {}", run.err);
    };

    // A variable that is mapped to OpenBao.
    env.run(
        &[
            "secret",
            "set",
            "reviewer",
            "key",
            "--variable",
            "OPENAI_API_KEY",
        ],
        "v\n",
    )
    .await;
    let (writes, bao_writes) = (env.fake.writes().len(), env.bao_writes());
    refused(
        &deploy(&env, &["--env", "OPENAI_API_KEY=sk-123"]).await,
        "OPENAI_API_KEY is mapped to a secret in OpenBao",
    );
    refused(
        &deploy(&env, &["--env", "SERVER_PORT=1"]).await,
        "set by `agent deploy`",
    );
    refused(&deploy(&env, &["--env", "NOVALUE"]).await, "NAME=VALUE");
    refused(
        &deploy(&env, &["--data-size", "lots"]).await,
        "not a quantity",
    );

    // A tool with code needs a build.
    std::fs::write(
        dir.join("tool.json"),
        json!({"spec_version": 1, "name": "tool",
            "model": {"provider": "ollama", "id": "qwen3.5:9b"},
            "tools": [{"name": "add", "description": "adds", "args": {"type": "object"},
                "code": "Ok(String::new())"}],
            "strategy": {"kind": "single_run"}})
        .to_string(),
    )
    .unwrap();
    let run = deploy(
        &env,
        &["--spec", &dir.join("tool.json").display().to_string()],
    )
    .await;
    refused(&run, "spec generate --service --dockerfile");
    assert!(run.err.contains("without --spec"), "{}", run.err);

    // A tree that does not fit into a ConfigMap.
    std::fs::create_dir_all(dir.join("files")).unwrap();
    std::fs::write(dir.join("files/big.txt"), "x".repeat(1_100_000)).unwrap();
    std::fs::write(
        dir.join("big.json"),
        json!({"spec_version": 1, "name": "big",
            "model": {"provider": "ollama", "id": "qwen3.5:9b"},
            "builtin_tools": {"tools": ["read_file"], "root": "files"},
            "strategy": {"kind": "single_run"}})
        .to_string(),
    )
    .unwrap();
    let run = deploy(
        &env,
        &["--spec", &dir.join("big.json").display().to_string()],
    )
    .await;
    refused(&run, "a ConfigMap holds about 1 MiB at most");
    assert!(
        run.err.contains("spec generate --service --dockerfile"),
        "{}",
        run.err
    );

    // A file the agent writes into the spec's own folder: that folder cannot be a writable place.
    std::fs::write(
        dir.join("memory.json"),
        json!({"spec_version": 1, "name": "memory",
            "model": {"provider": "ollama", "id": "qwen3.5:9b"},
            "long_term_memory": {"store": "memory-store.json",
                "embedding_model": {"provider": "ollama", "id": "nomic-embed-text"}},
            "strategy": {"kind": "single_run"}})
        .to_string(),
    )
    .unwrap();
    let run = deploy(
        &env,
        &["--spec", &dir.join("memory.json").display().to_string()],
    )
    .await;
    refused(&run, "sub folder");

    // An agent that was not added.
    let run = env
        .run(
            &[
                "agent", "deploy", "stranger", "--image", IMAGE, "--spec", &spec,
            ],
            "",
        )
        .await;
    refused(&run, "agent stranger not found");

    assert_eq!(env.fake.writes().len(), writes, "{:?}", env.fake.writes());
    assert_eq!(env.bao_writes(), bao_writes);
}

#[tokio::test]
async fn an_unknown_environment_variable_is_a_warning_and_a_known_one_is_not() {
    let env = added().await;
    let run = deploy(
        &env,
        &["--env", "OLLAMA_HOST=http://o:1", "--env", "MY_OWN=1"],
    )
    .await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.err.contains("MY_OWN is not a configuration variable"),
        "{}",
        run.err
    );
    assert!(!run.err.contains("OLLAMA_HOST"), "{}", run.err);
    assert_eq!(env_of(&pod_spec(&env), "MY_OWN").as_deref(), Some("1"));
}

#[tokio::test]
async fn the_server_token_is_made_once_mapped_and_kept_on_every_redeploy() {
    let env = added().await;
    let first = deploy(&env, &[]).await;
    assert!(
        first
            .out
            .contains("server token kv/agents/reviewer/server-token: created"),
        "{}",
        first.out
    );
    let token = {
        let bao = env.bao.lock().unwrap();
        let versions = &bao.kv["kv/agents/reviewer/server-token"];
        assert_eq!(versions.len(), 1);
        versions[0].clone()
    };
    assert_eq!(token.len(), 64);
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(!first.all().contains(&token));
    let config = env
        .fake
        .object(&format!("{NS}/configmaps/grauwolf-config"))
        .unwrap();
    assert!(
        config["data"]["grauwolf.toml"]
            .as_str()
            .unwrap()
            .contains("GRAUWOLF_SERVER_TOKEN = \"openbao:kv/agents/reviewer/server-token#value\"")
    );

    let again = deploy(&env, &[]).await;
    assert!(again.out.contains("server-token: kept"), "{}", again.out);
    assert_eq!(
        env.bao.lock().unwrap().kv["kv/agents/reviewer/server-token"],
        vec![token.clone()]
    );

    // `agent add` keeps the mapping, and the token can be read.
    env.run(&["agent", "add", "reviewer"], "").await;
    let config = env
        .fake
        .object(&format!("{NS}/configmaps/grauwolf-config"))
        .unwrap();
    assert!(
        config["data"]["grauwolf.toml"]
            .as_str()
            .unwrap()
            .contains("GRAUWOLF_SERVER_TOKEN")
    );
    let shown = env
        .run(
            &["secret", "get", "reviewer", "server-token", "--reveal"],
            "",
        )
        .await;
    assert_eq!(shown.out, format!("{token}\n"));
}

#[tokio::test]
async fn a_host_gives_an_ingress_without_tls_and_class_and_dropping_it_removes_the_ingress() {
    let env = added().await;
    let run = deploy(&env, &["--host", "reviewer.example.com"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    let path = "/apis/networking.k8s.io/v1/namespaces/reviewer/ingresses/reviewer";
    let ingress = env.fake.object(path).unwrap();
    assert_eq!(ingress["spec"]["rules"][0]["host"], "reviewer.example.com");
    assert_eq!(
        ingress["spec"]["rules"][0]["http"]["paths"][0]["backend"]["service"],
        json!({"name": "reviewer", "port": {"number": 8080}})
    );
    assert!(ingress["spec"].get("tls").is_none());
    assert!(ingress["spec"].get("ingressClassName").is_none());
    assert!(run.out.contains("host reviewer.example.com"), "{}", run.out);

    let run = deploy(&env, &[]).await;
    assert!(
        run.out.contains("Ingress reviewer/reviewer: deleted"),
        "{}",
        run.out
    );
    assert!(env.fake.object(path).is_none());
}

#[tokio::test]
async fn a_data_size_gives_a_claim_for_the_data_and_resources_are_only_set_when_asked() {
    let env = added().await;
    let run = deploy(
        &env,
        &["--data-size", "2Gi", "--cpu", "500m", "--memory", "512Mi"],
    )
    .await;
    assert_eq!(run.code, 0, "{}", run.all());
    let claim = env
        .fake
        .object(&format!("{NS}/persistentvolumeclaims/reviewer-data"))
        .unwrap();
    assert_eq!(claim["spec"]["resources"]["requests"]["storage"], "2Gi");
    let pod = pod_spec(&env);
    assert!(pod["volumes"].as_array().unwrap().contains(
        &json!({"name": "data", "persistentVolumeClaim": {"claimName": "reviewer-data"}})
    ));
    assert_eq!(pod["securityContext"]["fsGroup"], 65532);
    let resources = &pod["containers"][0]["resources"];
    assert_eq!(
        resources["requests"],
        json!({"cpu": "500m", "memory": "512Mi"})
    );
    assert_eq!(
        resources["limits"],
        json!({"cpu": "500m", "memory": "512Mi"})
    );
    assert_eq!(
        env_of(&pod, "GRAUWOLF_CONVERSATIONS_DIR").as_deref(),
        Some("/data/conversations")
    );
}

#[tokio::test]
async fn a_crash_loop_prints_the_log_and_the_reason_and_exits_with_1() {
    let env = added().await;
    env.fake
        .set_deployment_outcome(DeploymentOutcome::CrashLoop(
            "starting\nerror: no identity issued\n".into(),
        ));
    let run = deploy(&env, &[]).await;
    assert_eq!(run.code, 1, "{}", run.all());
    assert!(run.err.contains("CrashLoopBackOff"), "{}", run.err);
    assert!(run.err.contains("error: no identity issued"), "{}", run.err);
    assert!(run.err.contains("agent logs reviewer"), "{}", run.err);
}

#[tokio::test]
async fn a_pod_that_never_gets_ready_or_a_health_check_that_fails_times_out() {
    let env = added().await;
    env.fake
        .set_deployment_outcome(DeploymentOutcome::Starting("booting\n".into()));
    let run = deploy(&env, &["--timeout", "1"]).await;
    assert_eq!(run.code, 1, "{}", run.all());
    assert!(run.err.contains("not ready after 1 s"), "{}", run.err);
    assert!(run.err.contains("booting"), "{}", run.err);

    env.fake
        .set_deployment_outcome(DeploymentOutcome::Ready("up\n".into()));
    env.health.store(503, Ordering::SeqCst);
    // A changed image makes a new rollout whose pod is ready but not healthy.
    let run = env
        .run(
            &[
                "agent",
                "deploy",
                "reviewer",
                "--image",
                "other:2",
                "--timeout",
                "1",
            ],
            "",
        )
        .await;
    assert_eq!(run.code, 1, "{}", run.all());
    assert!(run.err.contains("/health answers HTTP 503"), "{}", run.err);
}

#[tokio::test]
async fn status_and_logs_show_the_pod_the_endpoint_and_never_the_token() {
    let env = added().await;
    let nothing = env.run(&["agent", "status", "reviewer"], "").await;
    assert!(nothing.out.contains("not deployed"), "{}", nothing.out);

    deploy(&env, &["--host", "reviewer.example.com"]).await;
    let run = env.run(&["agent", "status", "reviewer"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(run.out.contains("deployed, ready"), "{}", run.out);
    assert!(run.out.contains("0 restart(s)"), "{}", run.out);
    assert!(run.out.contains(&format!("image {IMAGE}")), "{}", run.out);
    assert!(
        run.out
            .contains("endpoint http://reviewer.reviewer.svc:8080"),
        "{}",
        run.out
    );
    assert!(run.out.contains("host reviewer.example.com"), "{}", run.out);
    assert!(run.out.contains("server token: exists"), "{}", run.out);
    let token = env.bao.lock().unwrap().kv["kv/agents/reviewer/server-token"][0].clone();
    assert!(!run.all().contains(&token));

    let run = env
        .run(&["agent", "status", "reviewer", "--json"], "")
        .await;
    let status: Value = serde_json::from_str(&run.out).unwrap();
    assert_eq!(status["deployed"], true);
    assert_eq!(status["ready"], true);
    assert_eq!(status["image"], IMAGE);
    assert_eq!(status["pod"]["restarts"], 0);
    assert_eq!(status["host"], "reviewer.example.com");
    assert_eq!(status["server_token"], true);

    let logs = env
        .run(&["agent", "logs", "reviewer", "--tail", "5"], "")
        .await;
    assert_eq!(logs.code, 0, "{}", logs.all());
    assert_eq!(logs.out, "listening on 0.0.0.0:8080\n");
    let followed = env
        .run(&["agent", "logs", "reviewer", "--follow"], "")
        .await;
    assert_eq!(followed.out, "listening on 0.0.0.0:8080\n");
}

#[tokio::test]
async fn undeploy_removes_what_deploy_made_and_keeps_identity_secrets_and_data() {
    let env = added().await;
    let dir = TestDir::new("deploy-undeploy");
    let file = write_tree(&dir);
    deploy(
        &env,
        &[
            "--spec",
            &spec_arg(&file),
            "--host",
            "reviewer.example.com",
            "--data-size",
            "1Gi",
        ],
    )
    .await;
    let run = env.run(&["agent", "undeploy", "reviewer"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    for line in [
        "Deployment reviewer/reviewer: deleted",
        "Service reviewer/reviewer: deleted",
        "Ingress reviewer/reviewer: deleted",
        "ConfigMap reviewer/grauwolf-agent: deleted",
        "kept: PersistentVolumeClaim reviewer-data",
        "kept: identity, role, policy, secrets and the callers",
    ] {
        assert!(run.out.contains(line), "wanted '{line}' in: {}", run.out);
    }
    assert!(env.fake.object(DEPLOYMENT).is_none());
    assert!(
        env.fake
            .object(&format!("{NS}/persistentvolumeclaims/reviewer-data"))
            .is_some()
    );
    assert!(
        env.fake
            .object(&format!("{NS}/serviceaccounts/reviewer"))
            .is_some()
    );
    assert!(
        env.fake
            .object(&format!("{NS}/configmaps/grauwolf-config"))
            .is_some()
    );
    assert!(
        env.bao
            .lock()
            .unwrap()
            .kv
            .contains_key("kv/agents/reviewer/server-token")
    );
    assert!(env.bao.lock().unwrap().roles.contains_key("agent-reviewer"));
    let status = env.run(&["agent", "status", "reviewer"], "").await;
    assert!(status.out.contains("not deployed"), "{}", status.out);

    // The token survives, so a deploy again keeps it.
    let again = deploy(&env, &["--data-size", "1Gi"]).await;
    assert!(again.out.contains("server-token: kept"), "{}", again.out);

    // The data goes only with --purge-data, and that asks first.
    let cancelled = env
        .run(&["agent", "undeploy", "reviewer", "--purge-data"], "n\n")
        .await;
    assert_eq!(cancelled.code, 1);
    assert!(env.fake.object(DEPLOYMENT).is_some());
    let purged = env
        .run(
            &["agent", "undeploy", "reviewer", "--purge-data", "--yes"],
            "",
        )
        .await;
    assert_eq!(purged.code, 0, "{}", purged.all());
    assert!(
        env.fake
            .object(&format!("{NS}/persistentvolumeclaims/reviewer-data"))
            .is_none()
    );

    let nothing = env.run(&["agent", "undeploy", "reviewer"], "").await;
    assert_eq!(nothing.code, 1);
    assert!(nothing.err.contains("not deployed"), "{}", nothing.err);
}

#[tokio::test]
async fn agent_remove_lists_the_deployed_agent_and_still_removes_everything() {
    let env = added().await;
    deploy(&env, &["--data-size", "1Gi"]).await;
    let run = env.run(&["agent", "remove", "reviewer", "--yes"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out.contains("namespace reviewer with everything in it"),
        "{}",
        run.out
    );
    assert!(run.out.contains("deployed agent"), "{}", run.out);
    assert!(env.fake.object(DEPLOYMENT).is_none());
    assert!(
        env.fake
            .object(&format!("{NS}/persistentvolumeclaims/reviewer-data"))
            .is_none()
    );
    assert!(!env.bao.lock().unwrap().roles.contains_key("agent-reviewer"));
}

#[tokio::test]
async fn doctor_reports_a_deployed_agent_with_its_image_and_fails_when_it_is_not_ready() {
    let env = added().await;
    let checks = |run: Run| -> Vec<Value> { serde_json::from_str(&run.out).unwrap() };
    let before = checks(env.run(&["doctor", "--json"], "").await);
    assert!(
        before
            .iter()
            .all(|check| check["check"] != "agent reviewer: deployment")
    );

    deploy(&env, &[]).await;
    let run = env.run(&["doctor", "--json"], "").await;
    let found = checks(run);
    let line = found
        .iter()
        .find(|check| check["check"] == "agent reviewer: deployment")
        .unwrap();
    assert_eq!(line["status"], "ok");
    assert!(line["reason"].as_str().unwrap().contains(IMAGE), "{line}");

    env.fake
        .set_deployment_outcome(DeploymentOutcome::Starting(String::new()));
    // A new image is a new rollout, and its pod never gets ready.
    env.run(
        &[
            "agent",
            "deploy",
            "reviewer",
            "--image",
            "other:2",
            "--timeout",
            "1",
        ],
        "",
    )
    .await;
    let run = env.run(&["doctor", "--json"], "").await;
    assert_eq!(run.code, 1);
    let found = checks(run);
    let line = found
        .iter()
        .find(|check| check["check"] == "agent reviewer: deployment")
        .unwrap();
    assert_eq!(line["status"], "fail");
    assert!(
        line["reason"].as_str().unwrap().contains("not ready"),
        "{line}"
    );
    assert!(
        line["hint"]
            .as_str()
            .unwrap()
            .contains("agent status reviewer"),
        "{line}"
    );
}

const CALLERS: &str = "/api/v1/namespaces/reviewer/configmaps/grauwolf-callers";
const POLICY: &str =
    "/apis/networking.k8s.io/v1/namespaces/reviewer/networkpolicies/grauwolf-mtls-callers";

fn callers_text(env: &Env) -> String {
    env.fake
        .object(CALLERS)
        .expect("ConfigMap grauwolf-callers")["data"]["callers.json"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn template_hash(env: &Env) -> String {
    env.fake.object(DEPLOYMENT).unwrap()["spec"]["template"]["metadata"]["annotations"]
        ["grauwolf.io/config-hash"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn deploy_turns_on_spiffe_with_the_mtls_port_and_the_callers_as_a_directory() {
    let env = added().await;
    let run = deploy(&env, &[]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    let pod = pod_spec(&env);
    assert_eq!(env_of(&pod, "GRAUWOLF_SPIFFE").as_deref(), Some("1"));
    assert_eq!(
        env_of(&pod, "GRAUWOLF_SPIFFE_PORT").as_deref(),
        Some("8443")
    );
    assert_eq!(
        env_of(&pod, "GRAUWOLF_SPIFFE_USERS").as_deref(),
        Some("/etc/grauwolf/callers/callers.json")
    );
    assert_eq!(
        env_of(&pod, "GRAUWOLF_SPIFFE_PUBLIC_URL").as_deref(),
        Some("https://reviewer.reviewer.svc:8443")
    );
    let container = &pod["containers"][0];
    assert_eq!(
        container["ports"],
        json!([
            {"name": "http", "containerPort": 8080},
            {"name": "mtls", "containerPort": 8443},
        ])
    );
    // A directory, not a `subPath`: only a directory sees the swap of a ConfigMap's update.
    let mount = container["volumeMounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|mount| mount["name"] == "callers")
        .expect("a mount of the callers");
    assert_eq!(
        *mount,
        json!({"name": "callers", "mountPath": "/etc/grauwolf/callers", "readOnly": true})
    );
    assert!(
        pod["volumes"]
            .as_array()
            .unwrap()
            .contains(&json!({"name": "callers", "configMap": {"name": "grauwolf-callers"}}))
    );
    // The probes stay on the HTTP port.
    assert_eq!(container["readinessProbe"]["httpGet"]["port"], "http");
    assert_eq!(container["livenessProbe"]["httpGet"]["port"], "http");
    assert_eq!(container["startupProbe"]["httpGet"]["port"], "http");
    let service = env.fake.object(&format!("{NS}/services/reviewer")).unwrap();
    assert_eq!(
        service["spec"]["ports"],
        json!([
            {"name": "http", "port": 8080, "targetPort": "http"},
            {"name": "mtls", "port": 8443, "targetPort": "mtls"},
        ])
    );
    assert!(
        run.out
            .contains("mTLS endpoint https://reviewer.reviewer.svc:8443"),
        "{}",
        run.out
    );
}

#[tokio::test]
async fn mtls_port_moves_the_port_of_the_pod_the_service_the_url_and_the_policy() {
    let env = added().await;
    let run = deploy(&env, &["--mtls-port", "9443"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    let pod = pod_spec(&env);
    assert_eq!(
        env_of(&pod, "GRAUWOLF_SPIFFE_PORT").as_deref(),
        Some("9443")
    );
    assert_eq!(
        env_of(&pod, "GRAUWOLF_SPIFFE_PUBLIC_URL").as_deref(),
        Some("https://reviewer.reviewer.svc:9443")
    );
    assert_eq!(pod["containers"][0]["ports"][1]["containerPort"], 9443);
    let service = env.fake.object(&format!("{NS}/services/reviewer")).unwrap();
    assert_eq!(service["spec"]["ports"][1]["port"], 9443);

    // `allow` after the deploy uses the port of the pod.
    env.run(&["agent", "add", "coordinator"], "").await;
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let policy = env.fake.object(POLICY).unwrap();
    assert_eq!(policy["spec"]["ingress"][1]["ports"][0]["port"], 9443);

    // A new port at the next deploy moves the policy too.
    deploy(&env, &["--mtls-port", "9444"]).await;
    let policy = env.fake.object(POLICY).unwrap();
    assert_eq!(policy["spec"]["ingress"][1]["ports"][0]["port"], 9444);
}

#[tokio::test]
async fn deploy_refuses_an_mtls_port_that_cannot_work_and_the_variables_it_sets_itself() {
    let env = added().await;
    for (args, text) in [
        (vec!["--mtls-port", "0"], "--mtls-port must not be 0"),
        (
            vec!["--mtls-port", "8080"],
            "--mtls-port must differ from --port",
        ),
        (
            vec!["--env", "GRAUWOLF_SPIFFE_PORT=1"],
            "set by `agent deploy`",
        ),
        (vec!["--env", "GRAUWOLF_SPIFFE=0"], "set by `agent deploy`"),
        (
            vec!["--env", "GRAUWOLF_SPIFFE_USERS=/x"],
            "set by `agent deploy`",
        ),
        (
            vec!["--env", "GRAUWOLF_SPIFFE_PUBLIC_URL=https://x"],
            "set by `agent deploy`",
        ),
    ] {
        let run = deploy(&env, &args).await;
        assert_eq!(run.code, 1, "{}", run.all());
        assert!(run.err.contains(text), "{args:?}: {}", run.err);
    }
    assert!(env.fake.object(DEPLOYMENT).is_none());
    assert!(env.fake.object(CALLERS).is_none());
}

#[tokio::test]
async fn deploy_makes_an_empty_callers_map_and_a_policy_that_admits_nobody_on_the_mtls_port() {
    let env = added().await;
    deploy(&env, &[]).await;
    assert_eq!(
        serde_json::from_str::<Value>(&callers_text(&env)).unwrap(),
        json!({})
    );
    assert_eq!(
        env.fake.object(POLICY).unwrap()["spec"]["ingress"],
        json!([{"ports": [{"protocol": "TCP", "port": 8080}]}])
    );
}

#[tokio::test]
async fn a_redeploy_keeps_the_callers_that_allow_wrote_and_so_does_undeploy() {
    let env = added().await;
    env.run(&["agent", "add", "coordinator"], "").await;
    // Allowed before the first deploy: deploy must not replace the map with an empty one.
    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let before = callers_text(&env);
    deploy(&env, &[]).await;
    assert_eq!(callers_text(&env), before);
    deploy(&env, &["--env", "OLLAMA_THINK=false"]).await;
    assert_eq!(callers_text(&env), before);
    assert!(before.contains("spiffe://agentswork.de/ns/coordinator/sa/coordinator"));
    assert_eq!(
        env.fake.object(POLICY).unwrap()["spec"]["ingress"][1]["from"][0]["namespaceSelector"]["matchLabels"]
            ["kubernetes.io/metadata.name"],
        "coordinator"
    );

    env.run(&["agent", "undeploy", "reviewer"], "").await;
    assert_eq!(callers_text(&env), before);
    assert!(env.fake.object(POLICY).is_some());
}

#[tokio::test]
async fn changing_only_the_callers_does_not_restart_the_pod() {
    let env = added().await;
    env.run(&["agent", "add", "coordinator"], "").await;
    deploy(&env, &[]).await;
    let hash = template_hash(&env);
    let template = pod_spec(&env);

    env.run(&["agent", "allow", "reviewer", "--from", "coordinator"], "")
        .await;
    let again = deploy(&env, &[]).await;
    assert_eq!(again.code, 0, "{}", again.all());
    assert!(
        again
            .out
            .contains("Deployment reviewer/reviewer: unchanged"),
        "{}",
        again.out
    );
    assert_eq!(template_hash(&env), hash);
    assert_eq!(pod_spec(&env), template);

    env.run(&["agent", "deny", "reviewer", "--from", "coordinator"], "")
        .await;
    assert_eq!(template_hash(&env), hash);
}

const LGTM_ENDPOINT: &str = "http://lgtm.lgtm.svc:4318";

fn command_of(env: &Env) -> Vec<String> {
    serde_json::from_value(pod_spec(env)["containers"][0]["command"].clone()).unwrap_or_default()
}

#[tokio::test]
async fn otel_runs_spec_serve_with_tracing_and_the_default_endpoint() {
    let env = added().await;
    let dir = TestDir::new("deploy-otel");
    let file = write_tree(&dir);
    let run = deploy(&env, &["--spec", &spec_arg(&file), "--otel"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(run.out.contains("tracing: on"), "{}", run.out);
    assert_eq!(
        command_of(&env),
        [
            "grauwolf",
            "spec",
            "serve",
            "/app/spec/reviewer.json",
            "--addr",
            "0.0.0.0:8080",
            "--otel"
        ]
    );
    let pod = pod_spec(&env);
    assert_eq!(
        env_of(&pod, "OTEL_EXPORTER_OTLP_ENDPOINT").as_deref(),
        Some(LGTM_ENDPOINT)
    );
    assert_eq!(
        env_of(&pod, "OTEL_SERVICE_NAME").as_deref(),
        Some("reviewer")
    );
}

/// ADR-0123: the service of an image has no flags, tracing is switched on by its environment.
#[tokio::test]
async fn otel_without_a_spec_sets_the_switches_of_the_generated_service() {
    let env = added().await;
    let run = deploy(&env, &["--otel"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(run.out.contains("tracing: on"), "{}", run.out);
    let pod = pod_spec(&env);
    assert_eq!(env_of(&pod, "GRAUWOLF_OTEL").as_deref(), Some("1"));
    assert_eq!(env_of(&pod, "GRAUWOLF_OTEL_CONTENT"), None);
    assert_eq!(
        env_of(&pod, "OTEL_EXPORTER_OTLP_ENDPOINT").as_deref(),
        Some(LGTM_ENDPOINT)
    );
    assert_eq!(
        env_of(&pod, "OTEL_SERVICE_NAME").as_deref(),
        Some("reviewer")
    );
    assert!(command_of(&env).is_empty(), "the image runs as it is");

    let run = deploy(
        &env,
        &[
            "--otel",
            "--otel-endpoint",
            "http://collector:4318",
            "--otel-content",
        ],
    )
    .await;
    assert_eq!(run.code, 0, "{}", run.all());
    let pod = pod_spec(&env);
    assert_eq!(env_of(&pod, "GRAUWOLF_OTEL_CONTENT").as_deref(), Some("1"));
    assert_eq!(
        env_of(&pod, "OTEL_EXPORTER_OTLP_ENDPOINT").as_deref(),
        Some("http://collector:4318")
    );

    // Like every option, tracing is on only while it is given.
    deploy(&env, &[]).await;
    let pod = pod_spec(&env);
    for name in [
        "GRAUWOLF_OTEL",
        "GRAUWOLF_OTEL_CONTENT",
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "OTEL_SERVICE_NAME",
    ] {
        assert_eq!(env_of(&pod, name), None, "{name}");
    }
}

#[tokio::test]
async fn a_deploy_with_a_spec_does_not_set_the_switches_of_the_service() {
    let env = added().await;
    let dir = TestDir::new("deploy-otel-no-switch");
    let file = write_tree(&dir);
    deploy(&env, &["--spec", &spec_arg(&file), "--otel"]).await;
    assert_eq!(env_of(&pod_spec(&env), "GRAUWOLF_OTEL"), None);
}

#[tokio::test]
async fn status_shows_the_tracing_of_an_image() {
    let env = added().await;
    deploy(&env, &["--otel", "--otel-content"]).await;
    let on = env.run(&["agent", "status", "reviewer"], "").await;
    assert!(
        on.out
            .contains(&format!("tracing: on ({LGTM_ENDPOINT}, with contents)")),
        "{}",
        on.out
    );
    deploy(&env, &[]).await;
    let off = env.run(&["agent", "status", "reviewer"], "").await;
    assert!(off.out.contains("tracing: off"), "{}", off.out);
}

#[tokio::test]
async fn otel_endpoint_and_content_are_passed_on() {
    let env = added().await;
    let dir = TestDir::new("deploy-otel-content");
    let file = write_tree(&dir);
    let run = deploy(
        &env,
        &[
            "--spec",
            &spec_arg(&file),
            "--otel",
            "--otel-endpoint",
            "https://collector.example.com:4318",
            "--otel-content",
        ],
    )
    .await;
    assert_eq!(run.code, 0, "{}", run.all());
    let command = command_of(&env);
    assert_eq!(command[command.len() - 2..], ["--otel", "--otel-content"]);
    assert_eq!(
        env_of(&pod_spec(&env), "OTEL_EXPORTER_OTLP_ENDPOINT").as_deref(),
        Some("https://collector.example.com:4318")
    );
}

#[tokio::test]
async fn otel_needs_a_url_and_writes_nothing_when_refused() {
    let env = added().await;
    let dir = TestDir::new("deploy-otel-refuse");
    let file = write_tree(&dir);
    let spec = spec_arg(&file);
    for (args, text) in [
        (
            vec!["--spec", &spec, "--otel", "--otel-endpoint", "lgtm:4318"],
            "is not a URL",
        ),
        (
            vec!["--spec", &spec, "--otel", "--otel-endpoint", "http://"],
            "is not a URL",
        ),
    ] {
        let run = deploy(&env, &args).await;
        assert_ne!(run.code, 0, "{args:?}: {}", run.all());
        assert!(run.err.contains(text), "{args:?}: {}", run.err);
    }
    assert!(env.fake.object(DEPLOYMENT).is_none());
}

#[tokio::test]
async fn env_cannot_set_the_otel_variables_with_otel_but_can_without() {
    let env = added().await;
    let dir = TestDir::new("deploy-otel-env");
    let file = write_tree(&dir);
    let spec = spec_arg(&file);
    for name in [
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "OTEL_SERVICE_NAME",
        "GRAUWOLF_OTEL",
        "GRAUWOLF_OTEL_CONTENT",
    ] {
        let value = format!("{name}=x");
        let run = deploy(&env, &["--spec", &spec, "--otel", "--env", &value]).await;
        assert_eq!(run.code, 1, "{}", run.all());
        assert!(
            run.err.contains("set by `agent deploy --otel`"),
            "{}",
            run.err
        );
    }
    assert!(env.fake.object(DEPLOYMENT).is_none());
    let run = deploy(&env, &["--spec", &spec, "--env", "OTEL_SERVICE_NAME=mine"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(
        env_of(&pod_spec(&env), "OTEL_SERVICE_NAME").as_deref(),
        Some("mine")
    );
}

#[tokio::test]
async fn tracing_changes_the_hash_and_a_deploy_without_otel_switches_it_off() {
    let env = added().await;
    let dir = TestDir::new("deploy-otel-hash");
    let file = write_tree(&dir);
    let spec = spec_arg(&file);
    deploy(&env, &["--spec", &spec]).await;
    let plain = template_hash(&env);

    deploy(&env, &["--spec", &spec, "--otel"]).await;
    let traced = template_hash(&env);
    assert_ne!(traced, plain);
    let (writes, _) = (env.fake.writes().len(), 0);
    let again = deploy(&env, &["--spec", &spec, "--otel"]).await;
    assert!(
        again
            .out
            .contains("Deployment reviewer/reviewer: unchanged"),
        "{}",
        again.out
    );
    assert_eq!(env.fake.writes().len(), writes);

    deploy(
        &env,
        &[
            "--spec",
            &spec,
            "--otel",
            "--otel-endpoint",
            "http://x.y.svc:4318",
        ],
    )
    .await;
    let moved = template_hash(&env);
    assert_ne!(moved, traced);
    deploy(
        &env,
        &[
            "--spec",
            &spec,
            "--otel",
            "--otel-endpoint",
            "http://x.y.svc:4318",
            "--otel-content",
        ],
    )
    .await;
    assert_ne!(template_hash(&env), moved);

    deploy(&env, &["--spec", &spec]).await;
    assert_eq!(template_hash(&env), plain);
    let pod = pod_spec(&env);
    assert_eq!(env_of(&pod, "OTEL_EXPORTER_OTLP_ENDPOINT"), None);
    assert_eq!(env_of(&pod, "OTEL_SERVICE_NAME"), None);
    assert!(
        !command_of(&env)
            .iter()
            .any(|part| part.starts_with("--otel"))
    );
}

#[tokio::test]
async fn status_shows_whether_tracing_is_on() {
    let env = added().await;
    let dir = TestDir::new("deploy-otel-status");
    let file = write_tree(&dir);
    let spec = spec_arg(&file);
    deploy(&env, &["--spec", &spec]).await;
    let off = env.run(&["agent", "status", "reviewer"], "").await;
    assert!(off.out.contains("tracing: off"), "{}", off.out);
    let json = env
        .run(&["agent", "status", "reviewer", "--json"], "")
        .await;
    assert_eq!(
        serde_json::from_str::<Value>(&json.out).unwrap()["tracing"],
        Value::Null
    );

    deploy(&env, &["--spec", &spec, "--otel"]).await;
    let on = env.run(&["agent", "status", "reviewer"], "").await;
    assert!(
        on.out.contains(&format!("tracing: on ({LGTM_ENDPOINT})")),
        "{}",
        on.out
    );
    let json = env
        .run(&["agent", "status", "reviewer", "--json"], "")
        .await;
    let status: Value = serde_json::from_str(&json.out).unwrap();
    assert_eq!(
        status["tracing"],
        json!({"endpoint": LGTM_ENDPOINT, "content": false})
    );

    deploy(&env, &["--spec", &spec, "--otel", "--otel-content"]).await;
    let on = env.run(&["agent", "status", "reviewer"], "").await;
    assert!(
        on.out
            .contains(&format!("tracing: on ({LGTM_ENDPOINT}, with contents)")),
        "{}",
        on.out
    );
}

#[tokio::test]
async fn doctor_checks_that_the_service_of_the_collector_exists_but_never_calls_it() {
    let env = added().await;
    let dir = TestDir::new("deploy-otel-doctor");
    let file = write_tree(&dir);
    let spec = spec_arg(&file);
    let line = |run: Run| -> Option<Value> {
        serde_json::from_str::<Vec<Value>>(&run.out)
            .unwrap()
            .into_iter()
            .find(|check| check["check"] == "agent reviewer: tracing")
    };
    deploy(&env, &["--spec", &spec]).await;
    assert!(line(env.run(&["doctor", "--json"], "").await).is_none());

    deploy(&env, &["--spec", &spec, "--otel"]).await;
    let missing = line(env.run(&["doctor", "--json"], "").await).unwrap();
    assert_eq!(missing["status"], "warn");
    assert!(
        missing["reason"]
            .as_str()
            .unwrap()
            .contains("Service lgtm/lgtm not found"),
        "{missing}"
    );

    env.fake.insert(
        "/api/v1/namespaces/lgtm/services/lgtm",
        json!({"apiVersion": "v1", "kind": "Service", "metadata": {"name": "lgtm", "namespace": "lgtm"}}),
    );
    let found = line(env.run(&["doctor", "--json"], "").await).unwrap();
    assert_eq!(found["status"], "ok");
    assert!(
        found["reason"]
            .as_str()
            .unwrap()
            .contains("Service lgtm/lgtm exists"),
        "{found}"
    );

    deploy(
        &env,
        &[
            "--spec",
            &spec,
            "--otel",
            "--otel-endpoint",
            "https://tempo.example.com",
        ],
    )
    .await;
    let outside = line(env.run(&["doctor", "--json"], "").await).unwrap();
    assert_eq!(outside["status"], "ok");
    assert!(
        outside["reason"].as_str().unwrap().contains("not checked"),
        "{outside}"
    );
}

#[tokio::test]
async fn events_with_a_spec_run_spec_serve_with_the_events_flag() {
    let env = added().await;
    let dir = TestDir::new("deploy-events-spec");
    let file = write_tree(&dir);
    let run = deploy(&env, &["--spec", &spec_arg(&file), "--events"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(run.out.contains("events: on"), "{}", run.out);
    assert_eq!(
        command_of(&env),
        [
            "grauwolf",
            "spec",
            "serve",
            "/app/spec/reviewer.json",
            "--addr",
            "0.0.0.0:8080",
            "--events"
        ]
    );
    assert_eq!(env_of(&pod_spec(&env), "GRAUWOLF_EVENTS"), None);
}

#[tokio::test]
async fn events_without_a_spec_set_the_switch_of_the_generated_service() {
    let env = added().await;
    let run = deploy(&env, &["--events"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(
        env_of(&pod_spec(&env), "GRAUWOLF_EVENTS").as_deref(),
        Some("1")
    );
    assert!(command_of(&env).is_empty(), "the image runs as it is");
}

#[tokio::test]
async fn events_change_the_hash_and_a_deploy_without_the_flag_turns_them_off() {
    let env = added().await;
    let dir = TestDir::new("deploy-events-hash");
    let file = write_tree(&dir);
    let spec = spec_arg(&file);
    deploy(&env, &["--spec", &spec]).await;
    let plain = template_hash(&env);

    deploy(&env, &["--spec", &spec, "--events"]).await;
    let logged = template_hash(&env);
    assert_ne!(logged, plain);
    let writes = env.fake.writes().len();
    let again = deploy(&env, &["--spec", &spec, "--events"]).await;
    assert!(
        again
            .out
            .contains("Deployment reviewer/reviewer: unchanged"),
        "{}",
        again.out
    );
    assert_eq!(env.fake.writes().len(), writes);

    deploy(&env, &["--spec", &spec]).await;
    assert_eq!(template_hash(&env), plain);
    assert!(!command_of(&env).contains(&"--events".to_owned()));

    // The same for the generated service of an image.
    deploy(&env, &["--events"]).await;
    assert_eq!(
        env_of(&pod_spec(&env), "GRAUWOLF_EVENTS").as_deref(),
        Some("1")
    );
    deploy(&env, &[]).await;
    assert_eq!(env_of(&pod_spec(&env), "GRAUWOLF_EVENTS"), None);
}

#[tokio::test]
async fn env_cannot_set_the_events_switch_with_events_but_can_without() {
    let env = added().await;
    let refused = deploy(&env, &["--events", "--env", "GRAUWOLF_EVENTS=0"]).await;
    assert_eq!(refused.code, 1);
    assert!(
        refused.err.contains("GRAUWOLF_EVENTS") && refused.err.contains("--events"),
        "{}",
        refused.err
    );
    assert!(env.fake.object(DEPLOYMENT).is_none(), "nothing was written");

    let run = deploy(&env, &["--env", "GRAUWOLF_EVENTS=1"]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(
        env_of(&pod_spec(&env), "GRAUWOLF_EVENTS").as_deref(),
        Some("1")
    );
}

#[tokio::test]
async fn status_shows_whether_events_are_on() {
    let env = added().await;
    let dir = TestDir::new("deploy-events-status");
    let file = write_tree(&dir);
    let spec = spec_arg(&file);
    for (extra, on) in [
        (&["--spec", &spec][..], false),
        (&["--spec", &spec, "--events"], true),
    ] {
        deploy(&env, extra).await;
        let text = env.run(&["agent", "status", "reviewer"], "").await;
        assert!(
            text.out
                .contains(if on { "events: on" } else { "events: off" }),
            "{}",
            text.out
        );
        let json = env
            .run(&["agent", "status", "reviewer", "--json"], "")
            .await;
        assert_eq!(
            serde_json::from_str::<Value>(&json.out).unwrap()["events"],
            on
        );
    }
    // An image: the switch of the service.
    deploy(&env, &["--events"]).await;
    let text = env.run(&["agent", "status", "reviewer"], "").await;
    assert!(text.out.contains("events: on"), "{}", text.out);
}

#[tokio::test]
async fn logs_events_only_prints_just_the_json_event_lines() {
    let env = added().await;
    env.fake.set_deployment_outcome(DeploymentOutcome::Ready(
        "listening on 0.0.0.0:8080\n{\"event\":\"agent_starting\",\"input\":\"hi\"}\nnoise\n{\"event\":\"agent_completed\",\"output\":\"ok\"}\n".into(),
    ));
    deploy(&env, &["--events"]).await;

    let all = env.run(&["agent", "logs", "reviewer"], "").await;
    assert!(
        all.out.contains("listening") && all.out.contains("noise"),
        "{}",
        all.out
    );

    let only = env
        .run(&["agent", "logs", "reviewer", "--events-only"], "")
        .await;
    assert_eq!(only.code, 0, "{}", only.all());
    assert_eq!(
        only.out,
        "{\"event\":\"agent_starting\",\"input\":\"hi\"}\n{\"event\":\"agent_completed\",\"output\":\"ok\"}\n"
    );
}

// ---- writable places of the spec tree (ADR-0121) ----

/// A spec `file` in `dir` with the given extra fields.
fn write_spec(dir: &TestDir, file: &str, extra: Value) -> PathBuf {
    let mut spec = json!({"spec_version": 1, "name": "reviewer",
        "model": {"provider": "ollama", "id": "qwen3.5:9b"},
        "strategy": {"kind": "single_run"}});
    for (key, value) in extra.as_object().unwrap() {
        spec[key] = value.clone();
    }
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, spec.to_string()).unwrap();
    path
}

fn files_root(root: &str) -> Value {
    json!({"builtin_tools": {"tools": ["read_file", "write_file"], "root": root}})
}

/// The mounts of the data volume under the spec folder: `(mountPath, subPath)`.
fn data_places(env: &Env) -> Vec<(String, String)> {
    let pod = pod_spec(env);
    pod["containers"][0]["volumeMounts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|mount| mount["name"] == "data" && mount.get("subPath").is_some())
        .map(|mount| {
            assert!(mount.get("readOnly").is_none(), "{mount}");
            (
                mount["mountPath"].as_str().unwrap().to_owned(),
                mount["subPath"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn places(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

#[tokio::test]
async fn a_relative_root_is_a_directory_of_the_data_volume_at_its_place_in_the_spec_folder() {
    let env = added().await;
    let dir = TestDir::new("deploy-root");
    let file = write_spec(&dir, "reviewer.json", files_root("work"));
    let run = deploy(&env, &["--spec", &spec_arg(&file)]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(
        data_places(&env),
        places(&[("/app/spec/work", "files/work")])
    );
    // The directory exists before the agent starts: an init container with the same restrictions
    // makes it as the agent's user (a `subPath` directory that kubelet makes belongs to root).
    let pod = pod_spec(&env);
    let init = &pod["initContainers"][0];
    assert_eq!(init["image"], IMAGE);
    assert_eq!(init["command"], json!(["mkdir", "-p", "/data/files/work"]));
    assert_eq!(init["securityContext"]["allowPrivilegeEscalation"], false);
    assert_eq!(
        init["securityContext"]["capabilities"]["drop"],
        json!(["ALL"])
    );
    assert_eq!(
        init["volumeMounts"],
        json!([{"name": "data", "mountPath": "/data"}])
    );
    assert!(run.out.contains("writable: /app/spec/work"), "{}", run.out);
    // `status` lists it.
    let status = env.run(&["agent", "status", "reviewer"], "").await;
    assert!(
        status.out.contains("writable /app/spec/work"),
        "{}",
        status.out
    );
    let json = env
        .run(&["agent", "status", "reviewer", "--json"], "")
        .await;
    let json: Value = serde_json::from_str(json.out.trim()).unwrap();
    assert_eq!(json["writable"], json!(["/app/spec/work"]));
}

#[tokio::test]
async fn without_a_writable_place_the_pod_has_no_init_container() {
    let env = added().await;
    let dir = TestDir::new("deploy-no-init");
    let file = write_tree(&dir);
    let run = deploy(&env, &["--spec", &spec_arg(&file)]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(pod_spec(&env).get("initContainers").is_none());
    assert!(data_places(&env).is_empty());
}

#[tokio::test]
async fn the_tools_dir_is_a_writable_place_too() {
    let env = added().await;
    let dir = TestDir::new("deploy-tools-dir");
    let file = write_spec(
        &dir,
        "reviewer.json",
        json!({"tools_dir": "./tools/", "self_tools": {"save": true}}),
    );
    let run = deploy(&env, &["--spec", &spec_arg(&file)]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(
        data_places(&env),
        places(&[("/app/spec/tools", "files/tools")])
    );
}

#[tokio::test]
async fn a_sub_agent_has_its_writable_places_at_its_own_folder() {
    let env = added().await;
    let dir = TestDir::new("deploy-sub-root");
    write_spec(&dir, "team/helper.json", files_root("scratch"));
    let file = write_spec(
        &dir,
        "reviewer.json",
        json!({"builtin_tools": {"tools": ["write_file"], "root": "work"},
            "agent_tools": {"helper": {"spec": "team/helper.json", "description": "helps"}}}),
    );
    let run = deploy(&env, &["--spec", &spec_arg(&file)]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(
        data_places(&env),
        places(&[
            ("/app/spec/team/scratch", "files/team/scratch"),
            ("/app/spec/work", "files/work"),
        ])
    );
    assert_eq!(
        pod_spec(&env)["initContainers"][0]["command"],
        json!([
            "mkdir",
            "-p",
            "/data/files/team/scratch",
            "/data/files/work"
        ])
    );
}

#[tokio::test]
async fn an_absolute_root_stays_as_it_is_and_draws_a_warning_unless_it_is_under_data() {
    let env = added().await;
    let dir = TestDir::new("deploy-abs-root");
    let file = write_spec(&dir, "reviewer.json", files_root("/data/work"));
    let run = deploy(&env, &["--spec", &spec_arg(&file)]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(data_places(&env).is_empty());
    assert!(pod_spec(&env).get("initContainers").is_none());
    assert!(!run.err.contains("warning"), "{}", run.err);

    let file = write_spec(&dir, "reviewer.json", files_root("/srv/work"));
    let run = deploy(&env, &["--spec", &spec_arg(&file)]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(data_places(&env).is_empty());
    assert!(
        run.err.contains("warning: builtin_tools.root '/srv/work'")
            && run.err.contains("under /data"),
        "{}",
        run.err
    );
}

#[tokio::test]
async fn a_path_that_leaves_the_spec_folder_is_refused_before_anything_is_written() {
    let env = added().await;
    let dir = TestDir::new("deploy-dotdot");
    let (writes, bao_writes) = (env.fake.writes().len(), env.bao_writes());
    for field in [
        files_root("../work"),
        files_root("a/../../work"),
        json!({"tools_dir": "../tools"}),
    ] {
        let file = write_spec(&dir, "reviewer.json", field);
        let run = deploy(&env, &["--spec", &spec_arg(&file)]).await;
        assert_eq!(run.code, 1, "{}", run.all());
        assert!(
            run.err.contains("leaves the spec folder") && run.err.contains("relative path"),
            "{}",
            run.err
        );
    }
    assert_eq!(env.fake.writes().len(), writes);
    assert_eq!(env.bao_writes(), bao_writes);
    // Inside the folder it is fine.
    let file = write_spec(&dir, "reviewer.json", files_root("a/../work"));
    let run = deploy(&env, &["--spec", &spec_arg(&file)]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(
        data_places(&env),
        places(&[("/app/spec/work", "files/work")])
    );
}

#[tokio::test]
async fn a_new_root_changes_the_template_hash() {
    let env = added().await;
    let dir = TestDir::new("deploy-root-hash");
    let file = write_spec(&dir, "reviewer.json", files_root("work"));
    deploy(&env, &["--spec", &spec_arg(&file)]).await;
    let first = template_hash(&env);
    let again = deploy(&env, &["--spec", &spec_arg(&file)]).await;
    assert!(
        again
            .out
            .contains("Deployment reviewer/reviewer: unchanged")
    );
    assert_eq!(template_hash(&env), first);
    let file = write_spec(&dir, "reviewer.json", files_root("scratch"));
    let run = deploy(&env, &["--spec", &spec_arg(&file)]).await;
    assert!(
        run.out.contains("Deployment reviewer/reviewer: updated"),
        "{}",
        run.out
    );
    assert_ne!(template_hash(&env), first);
}

#[tokio::test]
async fn a_root_that_holds_spec_files_stays_read_only_with_a_warning() {
    let env = added().await;
    let dir = TestDir::new("deploy-root-files");
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::write(dir.join("docs/guide.txt"), "read me").unwrap();
    let file = write_spec(&dir, "reviewer.json", files_root("docs"));
    let run = deploy(&env, &["--spec", &spec_arg(&file)]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    // The file is where it was, nothing is hidden, and nothing is writable.
    assert!(data_places(&env).is_empty());
    assert!(
        pod_spec(&env)["containers"][0]["volumeMounts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|mount| mount["mountPath"] == "/app/spec/docs/guide.txt")
    );
    assert!(
        run.err.contains("warning: builtin_tools.root 'docs'")
            && run.err.contains("read-only")
            && run.err.contains("docs/guide.txt"),
        "{}",
        run.err
    );
}

#[tokio::test]
async fn a_tools_dir_with_tool_files_is_refused_as_before() {
    let env = added().await;
    let dir = TestDir::new("deploy-tools-dir-files");
    std::fs::create_dir_all(dir.join("tools")).unwrap();
    std::fs::write(dir.join("tools/upper.json"), "{}").unwrap();
    let file = write_spec(&dir, "reviewer.json", json!({"tools_dir": "tools"}));
    let run = deploy(&env, &["--spec", &spec_arg(&file)]).await;
    assert_eq!(run.code, 1, "{}", run.all());
    assert!(
        run.err.contains("tools/upper.json") && run.err.contains("spec generate --service"),
        "{}",
        run.err
    );
}

fn memory_in(store: &str) -> Value {
    json!({"long_term_memory": {"store": store,
        "embedding_model": {"provider": "ollama", "id": "nomic-embed-text"}}})
}

fn knowledge_in(index: &str) -> Value {
    json!({"knowledge": {"sources": ["docs"], "index": index,
        "embedding_model": {"provider": "ollama", "id": "nomic-embed-text"}}})
}

#[tokio::test]
async fn the_folder_of_the_memory_store_is_a_writable_place_and_a_local_copy_is_not_shipped() {
    let env = added().await;
    let dir = TestDir::new("deploy-store");
    std::fs::create_dir_all(dir.join("state")).unwrap();
    std::fs::write(dir.join("state/memory.json"), "[\"local\"]").unwrap();
    let file = write_spec(&dir, "reviewer.json", memory_in("state/memory.json"));
    let run = deploy(&env, &["--spec", &spec_arg(&file)]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(
        data_places(&env),
        places(&[("/app/spec/state", "files/state")])
    );
    assert!(
        run.out.contains("long_term_memory.store of reviewer.json"),
        "{}",
        run.out
    );
    // The memory of a local run is not part of the agent: only the spec is in the ConfigMap.
    let files = env
        .fake
        .object(&format!("{NS}/configmaps/grauwolf-agent"))
        .unwrap();
    assert_eq!(files["data"].as_object().unwrap().len(), 1);
}

#[tokio::test]
async fn the_folder_of_the_knowledge_index_is_a_writable_place_next_to_the_sources() {
    let env = added().await;
    let dir = TestDir::new("deploy-index");
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::write(dir.join("docs/guide.txt"), "read me").unwrap();
    std::fs::create_dir_all(dir.join("cache")).unwrap();
    std::fs::write(dir.join("cache/index.json"), "{}").unwrap();
    let file = write_spec(&dir, "reviewer.json", knowledge_in("cache/index.json"));
    let run = deploy(&env, &["--spec", &spec_arg(&file)]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(
        data_places(&env),
        places(&[("/app/spec/cache", "files/cache")])
    );
    // The sources are shipped, the index is not.
    let files = env
        .fake
        .object(&format!("{NS}/configmaps/grauwolf-agent"))
        .unwrap();
    assert_eq!(files["data"].as_object().unwrap().len(), 2);
}

#[tokio::test]
async fn a_store_or_index_that_cannot_have_a_folder_of_its_own_is_refused_with_a_hint() {
    let env = added().await;
    let dir = TestDir::new("deploy-store-refuse");
    let (writes, bao_writes) = (env.fake.writes().len(), env.bao_writes());
    // In the spec's own folder: that folder cannot be mounted over.
    let file = write_spec(&dir, "reviewer.json", memory_in("memory.json"));
    let run = deploy(&env, &["--spec", &spec_arg(&file)]).await;
    assert_eq!(run.code, 1, "{}", run.all());
    assert!(
        run.err.contains("long_term_memory.store 'memory.json'")
            && run.err.contains("sub folder")
            && run.err.contains("state/memory.json"),
        "{}",
        run.err
    );
    // In a folder with other files of the spec: they would be hidden.
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::write(dir.join("docs/guide.txt"), "read me").unwrap();
    let file = write_spec(&dir, "reviewer.json", knowledge_in("docs/index.json"));
    let run = deploy(&env, &["--spec", &spec_arg(&file)]).await;
    assert_eq!(run.code, 1, "{}", run.all());
    assert!(
        run.err.contains("knowledge.index 'docs/index.json'")
            && run.err.contains("docs/guide.txt")
            && run.err.contains("hide"),
        "{}",
        run.err
    );
    assert_eq!(env.fake.writes().len(), writes);
    assert_eq!(env.bao_writes(), bao_writes);
}

#[tokio::test]
async fn an_absolute_store_is_left_alone_with_a_warning_unless_it_is_under_data() {
    let env = added().await;
    let dir = TestDir::new("deploy-store-abs");
    let file = write_spec(&dir, "reviewer.json", memory_in("/data/memory.json"));
    let run = deploy(&env, &["--spec", &spec_arg(&file)]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(data_places(&env).is_empty());
    assert!(!run.err.contains("warning"), "{}", run.err);
    let file = write_spec(&dir, "reviewer.json", memory_in("/srv/memory.json"));
    let run = deploy(&env, &["--spec", &spec_arg(&file)]).await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.err
            .contains("warning: long_term_memory.store '/srv/memory.json'"),
        "{}",
        run.err
    );
}

/// ADR-0128 decision 6: a harness file is the input of `agent deploy`.
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

#[tokio::test]
async fn deploy_with_a_harness_takes_the_agent_s_name_from_it() {
    let env = added().await;
    let dir = TestDir::new("deploy-harness-name");
    let harness = write_harness(&dir, json!({}));

    let run = env
        .run(
            &["agent", "deploy", "--harness", &harness, "--image", IMAGE],
            "",
        )
        .await;

    assert_eq!(run.code, 0, "{}", run.all());
    let deployment = env.fake.object(DEPLOYMENT).expect("deployment");
    assert_eq!(
        deployment["spec"]["template"]["spec"]["containers"][0]["image"],
        IMAGE
    );
}

#[tokio::test]
async fn deploy_with_a_harness_takes_the_image_from_it() {
    let env = added().await;
    let dir = TestDir::new("deploy-harness-image");
    let harness = write_harness(&dir, json!({"image": "registry.example.com/reviewer:2.0"}));

    let run = env
        .run(&["agent", "deploy", "--harness", &harness], "")
        .await;

    assert_eq!(run.code, 0, "{}", run.all());
    let deployment = env.fake.object(DEPLOYMENT).expect("deployment");
    assert_eq!(
        deployment["spec"]["template"]["spec"]["containers"][0]["image"],
        "registry.example.com/reviewer:2.0"
    );
}

#[tokio::test]
async fn deploy_refuses_an_image_given_both_in_the_harness_and_as_a_flag() {
    let env = added().await;
    let dir = TestDir::new("deploy-harness-two-images");
    let harness = write_harness(&dir, json!({"image": "registry.example.com/reviewer:2.0"}));

    let run = env
        .run(
            &["agent", "deploy", "--harness", &harness, "--image", IMAGE],
            "",
        )
        .await;

    assert_ne!(run.code, 0);
    assert!(
        run.err
            .contains("--image: the harness file sets `image` already"),
        "{}",
        run.all()
    );
    assert!(env.fake.object(DEPLOYMENT).is_none());
}

#[tokio::test]
async fn deploy_with_a_harness_takes_the_port_from_it() {
    let env = added().await;
    let dir = TestDir::new("deploy-harness-port");
    let harness = write_harness(&dir, json!({"image": IMAGE, "port": 9000}));

    let run = env
        .run(&["agent", "deploy", "--harness", &harness], "")
        .await;

    assert_eq!(run.code, 0, "{}", run.all());
    let pod = pod_spec(&env);
    assert_eq!(env_of(&pod, "SERVER_PORT").as_deref(), Some("9000"));
    assert_eq!(pod["containers"][0]["ports"][0]["containerPort"], 9000);
    let service = env.fake.object(&format!("{NS}/services/reviewer")).unwrap();
    assert_eq!(service["spec"]["ports"][0]["port"], 9000);
}

#[tokio::test]
async fn deploy_with_a_harness_takes_the_environment_from_it() {
    let env = added().await;
    let dir = TestDir::new("deploy-harness-env");
    let harness = write_harness(
        &dir,
        json!({"image": IMAGE, "env": {"OLLAMA_HOST": "http://ollama.ollama.svc:11434"}}),
    );

    let run = env
        .run(&["agent", "deploy", "--harness", &harness], "")
        .await;

    assert_eq!(run.code, 0, "{}", run.all());
    assert_eq!(
        env_of(&pod_spec(&env), "OLLAMA_HOST").as_deref(),
        Some("http://ollama.ollama.svc:11434")
    );
}

#[tokio::test]
async fn deploy_with_a_harness_without_an_image_takes_the_default_image_of_the_workspace_registry()
{
    let env = added().await;
    let dir = TestDir::new("deploy-harness-default-image");
    std::fs::write(
        dir.join("grauwolf.toml"),
        "[harness]\nregistry = \"ghcr.io/agentsw0rk\"\n",
    )
    .unwrap();
    let harness = write_harness(&dir, json!({}));

    let run = env
        .run(&["agent", "deploy", "--harness", &harness], "")
        .await;

    assert_eq!(run.code, 0, "{}", run.all());
    let deployment = env.fake.object(DEPLOYMENT).expect("deployment");
    assert_eq!(
        deployment["spec"]["template"]["spec"]["containers"][0]["image"],
        format!("ghcr.io/agentsw0rk/reviewer:{}", env!("CARGO_PKG_VERSION"))
    );
}
