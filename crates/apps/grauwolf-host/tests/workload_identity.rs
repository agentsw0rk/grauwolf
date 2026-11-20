//! Provisioning of the identity-aware tools (ADR-0110, stage 1): `fetch.auth`, `git.auth`,
//! `workload_api` of stdio MCP servers and of `run_command`. The tools themselves are tested in
//! their crates; here: what provisioning hands them and the errors it gives.

use std::path::PathBuf;

use grauwolf_host::config::MapEnv;
use grauwolf_spec::{AgentSpec, AgentSpecExt};
use serde_json::{Value, json};

mod support;

const ID: &str = "spiffe://agentswork.de/ns/git/sa/git";

fn spec(builtin_tools: Value, mcp_servers: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "builtin_tools": builtin_tools,
            "mcp_servers": mcp_servers,
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap()
}

async fn error_of(spec: &AgentSpec, env: MapEnv) -> String {
    support::tools_in(spec, &PathBuf::from("."), env, None)
        .await
        .err()
        .expect("provisioning should fail")
}

#[tokio::test]
async fn fetch_auth_without_a_socket_names_the_host_entry() {
    let spec = spec(
        json!({ "tools": ["fetch"], "fetch": {
            "allowed_hosts": ["git.svc"], "auth": { "git.svc": { "spiffe": { "id": ID } } } } }),
        json!({}),
    );
    let error = error_of(&spec, MapEnv::new([])).await;
    assert!(
        error.contains("builtin_tools.fetch.auth.git.svc"),
        "{error}"
    );
    assert!(error.contains("SPIFFE_ENDPOINT_SOCKET"), "{error}");
}

#[tokio::test]
async fn git_auth_without_a_socket_names_the_host_entry() {
    let spec = spec(
        json!({ "tools": ["git_checkout"], "root": "w", "git": {
            "repo_url": "https://git.svc/team/repo.git",
            "auth": { "git.svc": { "spiffe": { "id": ID } } } } }),
        json!({}),
    );
    let error = error_of(&spec, MapEnv::new([])).await;
    assert!(error.contains("builtin_tools.git.auth.git.svc"), "{error}");
    assert!(error.contains("SPIFFE_ENDPOINT_SOCKET"), "{error}");
}

fn command_spec(workload_api: bool) -> AgentSpec {
    spec(
        json!({ "tools": ["run_command"], "root": "work", "command": {
            "allowed": ["env"], "env": ["PATH"], "workload_api": workload_api } }),
        json!({}),
    )
}

async fn env_of_command(spec: &AgentSpec, env: MapEnv) -> Result<String, String> {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("host-workload-command");
    std::fs::create_dir_all(base.join("work")).unwrap();
    let tools = support::tools_in(spec, &base, env, None).await?;
    tools[0]
        .call_json(json!({ "program": "env" }))
        .await
        .map_err(|error| error.to_string())
}

fn env_with_socket() -> MapEnv {
    MapEnv::new([
        ("PATH", "/usr/bin:/bin"),
        ("SPIFFE_ENDPOINT_SOCKET", "unix:///run/spire/agent.sock"),
        ("SPIFFE_FETCH_TIMEOUT", "7s"),
    ])
}

#[cfg(unix)]
#[tokio::test]
async fn run_command_programs_see_the_socket_only_with_workload_api() {
    let with = env_of_command(&command_spec(true), env_with_socket())
        .await
        .unwrap();
    assert!(
        with.contains("SPIFFE_ENDPOINT_SOCKET=unix:///run/spire/agent.sock"),
        "{with}"
    );
    assert!(with.contains("SPIFFE_FETCH_TIMEOUT=7s"), "{with}");
    let without = env_of_command(&command_spec(false), env_with_socket())
        .await
        .unwrap();
    assert!(!without.contains("SPIFFE"), "{without}");
}

#[cfg(unix)]
#[tokio::test]
async fn run_command_workload_api_without_a_socket_is_a_provisioning_error() {
    let error = env_of_command(
        &command_spec(true),
        MapEnv::new([("PATH", "/usr/bin:/bin")]),
    )
    .await
    .unwrap_err();
    assert!(error.contains("builtin_tools.command"), "{error}");
    assert!(error.contains("SPIFFE_ENDPOINT_SOCKET"), "{error}");
}

fn mcp_spec() -> AgentSpec {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../adapters/grauwolf-mcp/tests/fixtures/env_mcp_server.py")
        .canonicalize()
        .unwrap();
    spec(
        json!({ "tools": ["think"] }),
        json!({ "envdump": { "command": "python3", "args": [script], "tools": ["env"],
                              "workload_api": true } }),
    )
}

fn has_python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok()
}

#[tokio::test]
async fn a_stdio_mcp_server_with_workload_api_sees_the_socket() {
    if !has_python() {
        return;
    }
    let tools = support::tools_in(&mcp_spec(), &PathBuf::from("."), env_with_socket(), None)
        .await
        .unwrap();
    let tool = tools.iter().find(|tool| tool.name() == "env").unwrap();
    let value = tool
        .call_json(json!({ "name": "SPIFFE_ENDPOINT_SOCKET" }))
        .await
        .unwrap();
    assert_eq!(value, "unix:///run/spire/agent.sock");
}

#[tokio::test]
async fn a_stdio_mcp_server_with_workload_api_needs_the_socket_and_the_error_names_it() {
    if !has_python() {
        return;
    }
    let error = error_of(&mcp_spec(), MapEnv::new([])).await;
    assert!(error.contains("MCP server 'envdump'"), "{error}");
    assert!(error.contains("SPIFFE_ENDPOINT_SOCKET"), "{error}");
}
