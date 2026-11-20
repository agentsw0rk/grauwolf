//! `schedule add | list | remove | run-now` (ADR-0111 stage 3, Q7) against a fake Kubernetes API
//! server: the CronJob with its own identity, egress and the agent admitting it as a caller.

mod common;

use common::{Env, env};
use serde_json::{Value, json};

const IMAGE: &str = "registry.example.com/grauwolf:1.0";
const GRANT: &str = "grt_0123456789abcdef0123456789abcdef";
const CRON_JOB: &str = "/apis/batch/v1/namespaces/schedules/cronjobs/morning";
const CALLERS: &str = "/api/v1/namespaces/coordinator/configmaps/grauwolf-callers";
const MORNING: &str = "spiffe://agentswork.de/ns/schedules/sa/morning";
const COORDINATOR: &str = "spiffe://agentswork.de/ns/coordinator/sa/coordinator";

async fn prepared() -> Env {
    let env = env();
    env.prepare().await;
    let run = env.run(&["agent", "add", "coordinator"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    env
}

async fn add(env: &Env, extra: &[&str]) -> common::Run {
    let mut args = vec![
        "schedule",
        "add",
        "morning",
        "--agent",
        "coordinator",
        "--grant",
        GRANT,
        "--cron",
        "0 7 * * 1-5",
        "--input",
        "Summarize the night.",
        "--image",
        IMAGE,
    ];
    args.extend(extra);
    env.run(&args, "").await
}

#[tokio::test]
async fn add_creates_a_cron_job_with_its_own_identity_egress_and_a_place_among_the_callers() {
    let env = prepared().await;
    let run = add(&env, &[]).await;
    assert_eq!(run.code, 0, "{}", run.all());

    let namespace = env.fake.object("/api/v1/namespaces/schedules").unwrap();
    assert_eq!(
        namespace["metadata"]["labels"]["pod-security.kubernetes.io/enforce"],
        "restricted"
    );
    assert!(
        env.fake
            .object("/api/v1/namespaces/schedules/serviceaccounts/morning")
            .is_some()
    );
    let identity = env
        .fake
        .object("/apis/spire.spiffe.io/v1alpha1/clusterspiffeids/grauwolf-schedule-morning")
        .unwrap();
    assert_eq!(
        identity["spec"]["podSelector"]["matchLabels"]["grauwolf.io/schedule"],
        "morning"
    );
    assert_eq!(
        identity["spec"]["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"],
        "schedules"
    );

    let job = env.fake.object(CRON_JOB).unwrap();
    assert_eq!(job["spec"]["schedule"], "0 7 * * 1-5");
    assert_eq!(job["spec"]["concurrencyPolicy"], "Forbid");
    let pod = &job["spec"]["jobTemplate"]["spec"]["template"]["spec"];
    assert_eq!(pod["serviceAccountName"], "morning");
    assert_eq!(pod["securityContext"]["runAsNonRoot"], true);
    let container = &pod["containers"][0];
    assert_eq!(container["image"], IMAGE);
    assert_eq!(
        container["command"],
        json!([
            "grauwolf",
            "call",
            "https://coordinator.coordinator.svc:8443/run",
            "--spiffe-id",
            COORDINATOR,
            "--grant",
            GRANT,
            "--input",
            "Summarize the night."
        ])
    );
    assert_eq!(
        container["securityContext"]["allowPrivilegeEscalation"],
        false
    );
    assert_eq!(pod["volumes"][0]["csi"]["driver"], "csi.spiffe.io");

    // Egress: DNS and the agent's mTLS port only.
    let egress = env
        .fake
        .object("/apis/networking.k8s.io/v1/namespaces/schedules/networkpolicies/grauwolf-schedule-morning")
        .unwrap();
    let rules = egress["spec"]["egress"].as_array().unwrap();
    assert_eq!(rules.len(), 2);
    assert_eq!(
        rules[1]["ports"],
        json!([{"protocol": "TCP", "port": 8443}])
    );
    assert_eq!(
        rules[1]["to"][0]["podSelector"]["matchLabels"]["grauwolf.io/agent"],
        "coordinator"
    );

    // The coordinator admits the schedule: callers file and the NetworkPolicy of its mTLS port.
    let callers = env.fake.object(CALLERS).unwrap();
    let callers: Value =
        serde_json::from_str(callers["data"]["callers.json"].as_str().unwrap()).unwrap();
    assert_eq!(callers[MORNING], "schedule-morning");
    let ingress = env
        .fake
        .object("/apis/networking.k8s.io/v1/namespaces/coordinator/networkpolicies/grauwolf-mtls-callers")
        .unwrap();
    assert!(ingress.to_string().contains("\"schedules\""), "{ingress}");

    // The identity service is not installed: the output says what runs will lack.
    assert!(
        run.out.contains("identity service is not installed"),
        "{}",
        run.out
    );

    // Repeatable.
    let again = add(&env, &[]).await;
    assert_eq!(again.code, 0, "{}", again.all());
    assert!(!again.out.contains(": created"), "{}", again.out);
    assert!(!again.out.contains(": updated"), "{}", again.out);
}

#[tokio::test]
async fn add_refuses_bad_input_before_anything_is_written() {
    let env = prepared().await;
    for (extra, expected) in [
        (vec!["--grant", "grt_x"], "not a grant ID"),
        (vec!["--cron", "every morning"], "not a cron schedule"),
        (vec!["--agent", "nobody"], "agent nobody not found"),
    ] {
        let mut args = vec![
            "schedule",
            "add",
            "morning",
            "--agent",
            "coordinator",
            "--grant",
            GRANT,
            "--cron",
            "0 7 * * *",
            "--input",
            "x",
            "--image",
            IMAGE,
        ];
        for pair in extra.chunks(2) {
            let at = args.iter().position(|arg| *arg == pair[0]).unwrap();
            args[at + 1] = pair[1];
        }
        let run = env.run(&args, "").await;
        assert_eq!(run.code, 1, "{}", run.all());
        assert!(run.err.contains(expected), "{expected}: {}", run.err);
    }
    assert!(env.fake.object("/api/v1/namespaces/schedules").is_none());
}

#[tokio::test]
async fn list_run_now_and_remove() {
    let env = prepared().await;
    add(&env, &[]).await;
    let run = env.run(&["schedule", "list"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(
        run.out.contains(&format!(
            "morning  \"0 7 * * 1-5\"  agent coordinator  grant {GRANT}"
        )),
        "{}",
        run.out
    );
    let listed = env.run(&["schedule", "list", "--json"], "").await;
    let listed: Value = serde_json::from_str(listed.out.trim()).unwrap();
    assert_eq!(listed["schedules"][0]["grant"], GRANT);

    let run = env.run(&["schedule", "run-now", "morning"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    let jobs: Vec<Value> = env
        .fake
        .paths("/apis/batch/v1/namespaces/schedules/jobs/morning-manual-")
        .iter()
        .filter_map(|path| env.fake.object(path))
        .collect();
    assert_eq!(jobs.len(), 1, "{}", run.out);
    assert_eq!(
        jobs[0]["spec"]["template"]["spec"]["containers"][0]["command"][1],
        "call"
    );

    let run = env.run(&["schedule", "remove", "morning"], "").await;
    assert_eq!(run.code, 0, "{}", run.all());
    assert!(env.fake.object(CRON_JOB).is_none());
    assert!(
        env.fake
            .object("/apis/spire.spiffe.io/v1alpha1/clusterspiffeids/grauwolf-schedule-morning")
            .is_none()
    );
    let callers = env.fake.object(CALLERS).unwrap();
    assert!(
        !callers["data"]["callers.json"]
            .as_str()
            .unwrap()
            .contains(MORNING)
    );
    let missing = env.run(&["schedule", "remove", "morning"], "").await;
    assert_eq!(missing.code, 1);
    assert!(
        missing.err.contains("schedule morning not found"),
        "{}",
        missing.err
    );
}

#[tokio::test]
async fn add_names_what_the_identity_policy_still_lacks() {
    let env = prepared().await;
    env.fake
        .set_deployment_outcome(grauwolf_k8s::testing::DeploymentOutcome::Ready(
            "grauwolf identity service\n".into(),
        ));
    let install = env
        .run(
            &[
                "identity",
                "install",
                "--tenant",
                "t",
                "--issuer",
                "https://entra.example/t/v2.0",
                "--audience",
                "a",
                "--jwks-url",
                "https://entra.example/keys",
                "--image",
                IMAGE,
            ],
            "",
        )
        .await;
    assert_eq!(install.code, 0, "{}", install.all());
    let run = add(&env, &[]).await;
    assert!(
        run.out.contains("identity allow <agent> --grants"),
        "{}",
        run.out
    );
    assert!(run.out.contains("identity allow --trigger"), "{}", run.out);
    env.run(&["identity", "allow", "coordinator", "--grants"], "")
        .await;
    env.run(&["identity", "allow", "--trigger", "morning"], "")
        .await;
    let run = add(&env, &[]).await;
    assert!(!run.out.contains("identity allow"), "{}", run.out);
}
