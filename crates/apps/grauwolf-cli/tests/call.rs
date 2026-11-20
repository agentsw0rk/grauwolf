//! `grauwolf call` (ADR-0111 stage 3) as a process: with the workload identity of a fake
//! Workload API (JWT-SVID over http://) against an agent of the server test kit, with a token, and
//! its exit codes.

use std::process::{Command, Output};
use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_agents::memory::MemoryChatHistoryStore;
use grauwolf_interpreter::RunExtras;
use grauwolf_server::{CallerMap, ServedAgent, ServiceOptions, SpiffeGate, Users};
use grauwolf_spiffe::testing::{FakeIdentity, FakeWorkloadApi};
use grauwolf_test::TestDir;
use serde_json::{Value, json};

const AGENT: &str = "spiffe://agents.local/ns/coordinator/sa/coordinator";
const MORNING: &str = "spiffe://agents.local/ns/schedules/sa/morning";
const SHARED: &str = "shared-secret";

/// Answers with the user and the input it got.
struct Echo;

impl ServedAgent for Echo {
    fn input_schema(&self) -> Option<Value> {
        None
    }

    fn run(
        &self,
        input: Value,
        _extras: RunExtras,
        user: Option<String>,
    ) -> BoxFuture<'_, Result<Value, String>> {
        Box::pin(async move { Ok(json!(format!("{} said {input}", user.unwrap_or_default()))) })
    }
}

fn call(socket: Option<&str>, args: &[&str], stdin: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grauwolf"));
    command
        .arg("call")
        .args(args)
        .env_remove("SPIFFE_ENDPOINT_SOCKET");
    if let Some(socket) = socket {
        command.env("SPIFFE_ENDPOINT_SOCKET", socket);
    }
    match stdin {
        Some(text) => {
            use std::io::Write;
            command.stdin(std::process::Stdio::piped());
            command.stdout(std::process::Stdio::piped());
            command.stderr(std::process::Stdio::piped());
            let mut child = command.spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
            child.wait_with_output().unwrap()
        }
        None => command.output().unwrap(),
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn call_authenticates_with_the_workload_identity_or_a_token_and_reports_refusals() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let dir = TestDir::new("grauwolf-call");
    let socket = dir.path().join("agent.sock");
    let (url, _workload) = runtime.block_on(async {
        let workload = FakeWorkloadApi::start(&socket, MORNING, true);
        let gate = SpiffeGate::new(
            Arc::new(FakeIdentity::new(AGENT)),
            CallerMap::from_json(&json!({ MORNING: "morning" }).to_string()).unwrap(),
            AGENT.into(),
            false,
        );
        let router = grauwolf_server::with_spiffe_auth(
            grauwolf_server::router_for(
                Arc::new(Echo) as Arc<dyn ServedAgent>,
                Arc::new(MemoryChatHistoryStore::default()),
                ServiceOptions::default(),
            ),
            Some(SHARED.into()),
            Users::default(),
            Some(Arc::new(gate)),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (url, workload)
    });
    let endpoint = _workload.endpoint();

    // The workload identity: a JWT-SVID for the agent's ID; the caller is mapped to `morning`.
    let output = call(
        Some(&endpoint),
        &[&url, "--spiffe-id", AGENT, "--input", "hello"],
        None,
    );
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "morning said \"hello\"\n");

    // The input from stdin, as JSON.
    let output = call(
        Some(&endpoint),
        &[&format!("{url}/run"), "--spiffe-id", AGENT, "--json"],
        Some("{\"n\": 1}"),
    );
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "morning said {\"n\":1}\n");

    // Over http:// the workload identity needs --spiffe-id.
    let output = call(Some(&endpoint), &[&url, "--input", "x"], None);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("--spiffe-id"),
        "{}",
        text(&output.stderr)
    );

    // A token instead of the workload identity.
    let output = call(None, &[&url, "--token", SHARED, "--input", "x"], None);
    assert!(output.status.success(), "{}", text(&output.stderr));

    // Refused: a wrong token (401), a grant at an agent without identity service (401).
    let output = call(None, &[&url, "--token", "wrong", "--input", "x"], None);
    assert_eq!(output.status.code(), Some(3), "{}", text(&output.stderr));
    let output = call(
        Some(&endpoint),
        &[
            &url,
            "--spiffe-id",
            AGENT,
            "--grant",
            "grt_0123456789abcdef0123456789abcdef",
            "--input",
            "x",
        ],
        None,
    );
    assert_eq!(output.status.code(), Some(3), "{}", text(&output.stderr));
    assert!(
        text(&output.stderr).contains("HTTP 401"),
        "{}",
        text(&output.stderr)
    );

    // Unreachable: 1.
    let output = call(None, &["http://127.0.0.1:9", "--input", "x"], None);
    assert_eq!(output.status.code(), Some(1));
}
