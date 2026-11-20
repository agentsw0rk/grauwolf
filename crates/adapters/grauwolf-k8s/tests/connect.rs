//! `Cluster::connect` with a kubeconfig file: the context (no server is contacted).

use grauwolf_k8s::Cluster;
use grauwolf_test::TestDir;

const KUBECONFIG: &str = "\
apiVersion: v1
kind: Config
current-context: home
clusters:
- name: home-cluster
  cluster: {server: 'https://home.example:6443'}
- name: work-cluster
  cluster: {server: 'https://work.example:6443'}
users:
- name: me
  user: {token: not-a-real-token}
contexts:
- name: home
  context: {cluster: home-cluster, user: me}
- name: work
  context: {cluster: work-cluster, user: me}
";

fn write(dir: &TestDir) -> std::path::PathBuf {
    std::fs::write(dir.join("config"), KUBECONFIG).unwrap();
    dir.join("config")
}

#[tokio::test]
async fn without_a_context_the_current_one_is_used() {
    let dir = TestDir::new("k8s-context-current");
    let cluster = Cluster::connect(Some(&write(&dir)), None).await.unwrap();
    assert_eq!(cluster.server(), Some("https://home.example:6443"));
    assert_eq!(cluster.context(), Some("home"));
    assert_eq!(
        cluster.describe(),
        "https://home.example:6443 (context home)"
    );
}

#[tokio::test]
async fn a_named_context_selects_its_cluster() {
    let dir = TestDir::new("k8s-context-named");
    let cluster = Cluster::connect(Some(&write(&dir)), Some("work"))
        .await
        .unwrap();
    assert_eq!(cluster.server(), Some("https://work.example:6443"));
    assert_eq!(cluster.context(), Some("work"));
}

#[tokio::test]
async fn a_context_that_does_not_exist_is_an_error_naming_the_available_ones() {
    let dir = TestDir::new("k8s-context-missing");
    let error = Cluster::connect(Some(&write(&dir)), Some("staging"))
        .await
        .err()
        .expect("an error")
        .to_string();
    assert!(error.contains("context 'staging'"), "{error}");
    assert!(error.contains("available: home, work"), "{error}");
}
