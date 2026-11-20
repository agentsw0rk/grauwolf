//! MCP over stdio: server as a process (F09.2, ADR-0036).

use std::path::PathBuf;

use grauwolf_mcp::McpClient;
use serde_json::json;

#[tokio::test]
async fn talks_to_a_server_process_over_stdio() {
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipped: python3 is missing");
        return;
    }
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_mcp_server.py");

    let client = McpClient::spawn("python3", &[script.to_str().unwrap().to_owned()])
        .await
        .unwrap();

    assert_eq!(client.server_name(), "fake-stdio");
    assert_eq!(
        client
            .call_tool("upper", json!({ "text": "hallo" }))
            .await
            .unwrap(),
        "HALLO"
    );
}

async fn env_value(set: &[(String, String)], name: &str) -> Option<String> {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/env_mcp_server.py");
    let client = McpClient::spawn_with_env("python3", &[script.to_str().unwrap().to_owned()], set)
        .await
        .ok()?;
    client.call_tool("env", json!({ "name": name })).await.ok()
}

#[tokio::test]
async fn the_child_gets_the_variables_it_is_given_and_never_the_spiffe_ones_otherwise() {
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipped: python3 is missing");
        return;
    }
    // The agent's own environment has a Workload API socket (ADR-0110).
    // SAFETY: no other test of this binary reads or writes this variable concurrently.
    unsafe { std::env::set_var("SPIFFE_ENDPOINT_SOCKET", "unix:///inherited.sock") };

    let without = env_value(&[], "SPIFFE_ENDPOINT_SOCKET").await;
    let with = env_value(
        &[(
            "SPIFFE_ENDPOINT_SOCKET".to_owned(),
            "unix:///run/spire/agent.sock".to_owned(),
        )],
        "SPIFFE_ENDPOINT_SOCKET",
    )
    .await;

    assert_eq!(without.as_deref(), Some("<unset>"));
    assert_eq!(with.as_deref(), Some("unix:///run/spire/agent.sock"));
}
