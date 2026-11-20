//! The real binary: arguments, usage errors, and a kubeconfig that does not exist.

use std::process::Command;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_grauwolf-k8-ops"))
}

#[test]
fn help_lists_the_commands() {
    let output = binary().arg("--help").output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    for command in ["doctor", "oidc", "openbao", "agent", "secret"] {
        assert!(text.contains(command), "{text}");
    }
}

#[test]
fn a_usage_error_exits_with_2() {
    let output = binary().arg("frobnicate").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn a_missing_kubeconfig_is_an_error_not_a_panic() {
    let dir = grauwolf_test::TestDir::new("k8-ops-binary");
    let missing = dir.path().join("none");
    let output = binary()
        .args(["--kubeconfig"])
        .arg(&missing)
        .arg("doctor")
        .env_remove("KUBECONFIG")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(
        text.starts_with("error: cannot read the kubeconfig"),
        "{text}"
    );
}

/// The binary links more than one rustls crypto provider (kube uses ring, other crates bring
/// aws-lc-rs); without a choice rustls panicked at the first TLS connection. Found in the live
/// check against a real cluster (ADR-0120): an unreachable HTTPS server must be an error.
#[test]
fn a_tls_connection_fails_with_an_error_not_a_panic() {
    let dir = grauwolf_test::TestDir::new("k8-ops-tls");
    let kubeconfig = dir.path().join("kubeconfig");
    // Nothing listens on port 9 of the loopback address.
    std::fs::write(
        &kubeconfig,
        "apiVersion: v1\nkind: Config\ncurrent-context: c\n\
         clusters:\n- name: k\n  cluster:\n    server: https://127.0.0.1:9\n    insecure-skip-tls-verify: true\n\
         users:\n- name: u\n  user:\n    token: t\n\
         contexts:\n- name: c\n  context:\n    cluster: k\n    user: u\n",
    )
    .unwrap();
    let output = binary()
        .arg("--kubeconfig")
        .arg(&kubeconfig)
        .arg("doctor")
        .env_remove("OPENBAO_TOKEN")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(!text.contains("panicked"), "{text}");
    assert_eq!(output.status.code(), Some(1), "{text}");
}

const KUBECONFIG: &str = "\
apiVersion: v1
kind: Config
current-context: home
clusters:
- name: c
  cluster: {server: 'https://127.0.0.1:1'}
users:
- name: me
  user: {token: not-a-real-token}
contexts:
- name: home
  context: {cluster: c, user: me}
- name: work
  context: {cluster: c, user: me}
";

#[test]
fn a_context_that_does_not_exist_is_an_error_naming_the_available_ones() {
    let dir = grauwolf_test::TestDir::new("k8-ops-binary-context");
    std::fs::write(dir.join("config"), KUBECONFIG).unwrap();
    let output = binary()
        .arg("--kubeconfig")
        .arg(dir.join("config"))
        .args(["--context", "staging", "doctor"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(
        text.starts_with("error: context 'staging' not found")
            && text.contains("available: home, work"),
        "{text}"
    );
}

#[test]
fn the_context_option_is_global_and_documented() {
    let output = binary().args(["agent", "list", "--help"]).output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("--context <NAME>"), "{text}");
}
