//! A kubeconfig given as text (pasted into grauwolf studio, ADR-0129 B1).

use grauwolf_k8s::kubeconfig_contexts;

const KUBECONFIG: &str = r#"
apiVersion: v1
kind: Config
current-context: k3s
clusters:
- name: k3s
  cluster: { server: "https://10.0.0.5:6443" }
- name: kind
  cluster: { server: "https://127.0.0.1:52011" }
contexts:
- name: k3s
  context: { cluster: k3s, user: admin }
- name: kind-dev
  context: { cluster: kind, user: dev }
users:
- name: admin
  user: { token: "secret" }
- name: dev
  user: { token: "secret" }
"#;

#[test]
fn the_contexts_of_a_kubeconfig_with_their_servers_and_the_current_one() {
    let contexts = kubeconfig_contexts(KUBECONFIG).unwrap();

    assert_eq!(contexts.current.as_deref(), Some("k3s"));
    assert_eq!(
        contexts.contexts,
        vec![
            ("k3s".to_owned(), Some("https://10.0.0.5:6443".to_owned())),
            (
                "kind-dev".to_owned(),
                Some("https://127.0.0.1:52011".to_owned())
            ),
        ]
    );
    assert!(kubeconfig_contexts("not: [yaml").is_err());
}

#[tokio::test]
async fn connecting_with_a_kubeconfig_text_uses_the_chosen_context() {
    let cluster = grauwolf_k8s::Cluster::connect_text(KUBECONFIG, Some("kind-dev"))
        .await
        .unwrap();
    assert_eq!(cluster.context(), Some("kind-dev"));
    assert_eq!(cluster.server(), Some("https://127.0.0.1:52011"));

    let error = grauwolf_k8s::Cluster::connect_text(KUBECONFIG, Some("elsewhere"))
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(
        error
            .contains("context 'elsewhere' not found in the kubeconfig (available: k3s, kind-dev)"),
        "{error}"
    );
}
