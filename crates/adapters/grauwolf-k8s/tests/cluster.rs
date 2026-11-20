//! `Cluster` against the in-memory API server.

use grauwolf_k8s::testing::{DeploymentOutcome, FakeCluster, JobOutcome};
use grauwolf_k8s::{Outcome, kinds};
use serde_json::json;

fn config_map(value: &str) -> serde_json::Value {
    json!({"apiVersion": "v1", "kind": "ConfigMap",
        "metadata": {"name": "c", "namespace": "a"}, "data": {"k": value}})
}

#[tokio::test]
async fn ensure_creates_then_leaves_an_unchanged_object_alone_then_updates() {
    let fake = FakeCluster::new();
    let cluster = fake.cluster();
    let kind = kinds::CONFIG_MAP;
    assert_eq!(
        cluster
            .ensure(&kind, Some("a"), &config_map("1"))
            .await
            .unwrap(),
        Outcome::Created
    );
    assert_eq!(fake.writes().len(), 1);
    assert_eq!(
        cluster
            .ensure(&kind, Some("a"), &config_map("1"))
            .await
            .unwrap(),
        Outcome::Unchanged
    );
    assert_eq!(fake.writes().len(), 1, "a repeated run writes nothing");
    assert_eq!(
        cluster
            .ensure(&kind, Some("a"), &config_map("2"))
            .await
            .unwrap(),
        Outcome::Updated
    );
    let stored = cluster.get(&kind, Some("a"), "c").await.unwrap().unwrap();
    assert_eq!(stored["data"]["k"], "2");
}

#[tokio::test]
async fn get_of_a_missing_object_is_none_and_list_filters_by_label() {
    let fake = FakeCluster::new();
    let cluster = fake.cluster();
    assert!(
        cluster
            .get(&kinds::NAMESPACE, None, "nope")
            .await
            .unwrap()
            .is_none()
    );
    for (name, agent) in [("x", "x"), ("y", "y")] {
        let ns = json!({"apiVersion": "v1", "kind": "Namespace",
            "metadata": {"name": name, "labels": {"grauwolf.io/agent": agent}}});
        cluster.ensure(&kinds::NAMESPACE, None, &ns).await.unwrap();
    }
    let all = cluster
        .list(&kinds::NAMESPACE, None, Some("grauwolf.io/agent"))
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    let one = cluster
        .list(&kinds::NAMESPACE, None, Some("grauwolf.io/agent=y"))
        .await
        .unwrap();
    assert_eq!(one.len(), 1);
}

#[tokio::test]
async fn deleting_a_namespace_deletes_its_objects() {
    let fake = FakeCluster::new();
    let cluster = fake.cluster();
    let ns = json!({"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": "a"}});
    cluster.ensure(&kinds::NAMESPACE, None, &ns).await.unwrap();
    cluster
        .ensure(&kinds::CONFIG_MAP, Some("a"), &config_map("1"))
        .await
        .unwrap();
    assert!(cluster.delete(&kinds::NAMESPACE, None, "a").await.unwrap());
    assert!(fake.paths("/api/v1/namespaces/a").is_empty());
    assert!(!cluster.delete(&kinds::NAMESPACE, None, "a").await.unwrap());
}

#[tokio::test]
async fn access_reviews_and_version_and_logs() {
    let fake = FakeCluster::new();
    fake.deny("create", "jobs");
    let cluster = fake.cluster();
    assert!(
        cluster
            .can_i("create", "", "configmaps", Some("a"))
            .await
            .unwrap()
    );
    assert!(
        !cluster
            .can_i("create", "batch", "jobs", Some("a"))
            .await
            .unwrap()
    );
    assert_eq!(cluster.version().await.unwrap(), "v1.34.1+fake");

    fake.set_job_outcome(JobOutcome::Succeeds("resolved X\n".into()));
    let job = json!({"apiVersion": "batch/v1", "kind": "Job", "metadata": {"name": "j", "namespace": "a"}});
    cluster.ensure(&kinds::JOB, Some("a"), &job).await.unwrap();
    let pods = cluster
        .list(&kinds::POD, Some("a"), Some("job-name=j"))
        .await
        .unwrap();
    let pod = pods[0]["metadata"]["name"].as_str().unwrap();
    assert_eq!(cluster.pod_log("a", pod, 1).await.unwrap(), "resolved X\n");
}

#[tokio::test]
async fn raw_requests_reach_the_service_proxy() {
    let fake = FakeCluster::new();
    fake.on_proxy(|method, service, path, _| (200, format!("{method} {service} {path}")));
    let (status, text) = fake
        .cluster()
        .raw(
            "GET",
            "/api/v1/namespaces/openbao/services/http:openbao:8200/proxy/v1/sys/health",
            &[],
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        (status, text.as_str()),
        (200, "GET http:openbao:8200 /v1/sys/health")
    );
}

#[tokio::test]
async fn patch_merges_into_an_existing_object_and_refuses_a_missing_one() {
    let fake = FakeCluster::new();
    let cluster = fake.cluster();
    let kind = kinds::STATEFUL_SET;
    fake.insert(
        "/apis/apps/v1/namespaces/a/statefulsets/s",
        json!({"kind": "StatefulSet", "metadata": {"name": "s", "annotations": {"x": "1"}},
            "spec": {"replicas": 1, "serviceName": "s"}}),
    );
    cluster
        .patch(
            &kind,
            Some("a"),
            "s",
            &json!({"metadata": {"annotations": {"y": "2", "x": null}}, "spec": {"replicas": 0}}),
        )
        .await
        .unwrap();
    let stored = cluster.get(&kind, Some("a"), "s").await.unwrap().unwrap();
    assert_eq!(stored["spec"], json!({"replicas": 0, "serviceName": "s"}));
    assert_eq!(stored["metadata"]["annotations"], json!({"y": "2"}));
    assert_eq!(
        fake.writes(),
        ["PATCH /apis/apps/v1/namespaces/a/statefulsets/s"]
    );

    let error = cluster
        .patch(&kind, Some("a"), "missing", &json!({"spec": {}}))
        .await
        .unwrap_err();
    assert_eq!(error.status, Some(404));
    assert_eq!(fake.writes().len(), 1, "a patch never creates an object");
}

#[tokio::test]
async fn a_change_hook_sees_every_patch_and_delete_and_may_change_the_fake() {
    let fake = FakeCluster::new();
    let cluster = fake.cluster();
    fake.on_change(|fake, method, path| {
        if method == "DELETE" && path.ends_with("/persistentvolumeclaims/data") {
            fake.insert(
                "/api/v1/namespaces/a/configmaps/seen",
                json!({"data": {"path": path}}),
            );
        }
        if method == "PATCH" {
            fake.remove("/api/v1/namespaces/a/configmaps/gone");
        }
    });
    fake.insert(
        "/api/v1/namespaces/a/persistentvolumeclaims/data",
        json!({"kind": "PersistentVolumeClaim"}),
    );
    fake.insert("/api/v1/namespaces/a/configmaps/gone", json!({}));
    fake.insert("/api/v1/namespaces/a/configmaps/c", json!({"data": {}}));
    cluster
        .delete(&kinds::PERSISTENT_VOLUME_CLAIM, Some("a"), "data")
        .await
        .unwrap();
    assert!(
        fake.object("/api/v1/namespaces/a/configmaps/seen")
            .is_some()
    );
    cluster
        .patch(
            &kinds::CONFIG_MAP,
            Some("a"),
            "c",
            &json!({"data": {"k": "v"}}),
        )
        .await
        .unwrap();
    assert!(
        fake.object("/api/v1/namespaces/a/configmaps/gone")
            .is_none()
    );
}

fn deployment(image: &str) -> serde_json::Value {
    json!({"apiVersion": "apps/v1", "kind": "Deployment",
        "metadata": {"name": "d", "namespace": "a"},
        "spec": {"template": {"metadata": {"labels": {"app": "d"}},
            "spec": {"containers": [{"name": "main", "image": image}]}}}})
}

#[tokio::test]
async fn a_deployment_gets_a_ready_pod_with_a_log_and_a_new_generation_per_change() {
    let fake = FakeCluster::new();
    fake.set_deployment_outcome(DeploymentOutcome::Ready("one\ntwo\n".into()));
    let cluster = fake.cluster();
    cluster
        .ensure(&kinds::DEPLOYMENT, Some("a"), &deployment("x:1"))
        .await
        .unwrap();
    let object = cluster
        .get(&kinds::DEPLOYMENT, Some("a"), "d")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(object["metadata"]["generation"], 1);
    assert_eq!(object["status"]["observedGeneration"], 1);
    assert_eq!(object["status"]["readyReplicas"], 1);
    let pods = cluster
        .list(&kinds::POD, Some("a"), Some("app=d"))
        .await
        .unwrap();
    assert_eq!(pods[0]["status"]["containerStatuses"][0]["ready"], true);
    cluster
        .ensure(&kinds::DEPLOYMENT, Some("a"), &deployment("x:2"))
        .await
        .unwrap();
    let object = cluster
        .get(&kinds::DEPLOYMENT, Some("a"), "d")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(object["metadata"]["generation"], 2);
}

#[tokio::test]
async fn a_pod_log_can_be_streamed_in_lines_and_a_crash_loop_has_a_waiting_reason() {
    let fake = FakeCluster::new();
    fake.set_deployment_outcome(DeploymentOutcome::CrashLoop("boom\nlast words\n".into()));
    let cluster = fake.cluster();
    cluster
        .ensure(&kinds::DEPLOYMENT, Some("a"), &deployment("x:1"))
        .await
        .unwrap();
    let pods = cluster
        .list(&kinds::POD, Some("a"), Some("app=d"))
        .await
        .unwrap();
    assert_eq!(
        pods[0]["status"]["containerStatuses"][0]["state"]["waiting"]["reason"],
        "CrashLoopBackOff"
    );
    let mut seen = String::new();
    cluster
        .pod_log_stream("a", "d-x1", 10, true, |chunk| seen.push_str(chunk))
        .await
        .unwrap();
    assert_eq!(seen, "boom\nlast words\n");
    let error = cluster
        .pod_log_stream("a", "missing", 10, false, |_| {})
        .await
        .unwrap_err();
    assert_eq!(error.status, Some(404));
}
