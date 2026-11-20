//! Callers with a SPIFFE identity (ADR-0109): mTLS port, JWT-SVIDs, mapping file.

use grauwolf_spec::AgentSpecExt;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_agents::memory::MemoryChatHistoryStore;
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_server::{CallerMap, SpiffeInbound, Users};
use grauwolf_spec::AgentSpec;
use grauwolf_spiffe::testing::{FakeIdentity, TestCa, test_token};
use grauwolf_spiffe::{SpiffeMatch, TlsIdentity};
use grauwolf_test::MockLlmClient;
use grauwolf_test::TestDir;
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

/// Names the user it was built for – stands in for their memory tools.
struct WhoAmI(String);

impl DynTool for WhoAmI {
    fn name(&self) -> &str {
        "whoami"
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "whoami".into(),
            description: "Names the user".into(),
            parameters: json!({ "type": "object" }),
        }
    }

    fn call_json(&self, _args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        let user = self.0.clone();
        Box::pin(async move { Ok(user) })
    }
}

const SERVICE: &str = "spiffe://agentswork.de/ns/reviewer/sa/reviewer";
const COORDINATOR: &str = "spiffe://agentswork.de/ns/coordinator/sa/coordinator";
const INTRUDER: &str = "spiffe://agentswork.de/ns/intruder/sa/intruder";

struct Service {
    plain: String,
    mtls: String,
    ca: TestCa,
    server: FakeIdentity,
}

/// The agent behind the plain port (JWT-SVIDs) and the mTLS port, with the callers `callers`.
async fn serve(callers: CallerMap, accepted: &[&str]) -> Service {
    serve_with_token(callers, accepted, None).await
}

/// Like [`serve`], with a shared token on the plain port.
async fn serve_with_token(callers: CallerMap, accepted: &[&str], token: Option<&str>) -> Service {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "personal",
            "model": { "provider": "ollama", "id": "m" },
            "mcp_servers": { "people": { "command": "people", "tools": ["whoami"] } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let mock = MockLlmClient::new("?")
        .call_tool_when_contains("Who am I?", "whoami", json!({}))
        .answer_when_contains("coordinator", "You are coordinator.")
        .answer_when_contains("batch-jobs", "You are batch-jobs.")
        .answer_when_contains("nobody", "I do not know that.");
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("ollama", Arc::new(mock))
            .build(),
    ))
    .with_tools(vec![Arc::new(WhoAmI("nobody".into())) as Arc<dyn DynTool>])
    .load(&spec)
    .unwrap();
    let router = grauwolf_server::router_with_user_tools(
        Arc::new(agent),
        Arc::new(MemoryChatHistoryStore::default()),
        Arc::new(|user: &str| vec![Arc::new(WhoAmI(user.to_owned())) as Arc<dyn DynTool>]),
    );
    let ca = TestCa::new("agentswork.de");
    let server = FakeIdentity::with_ca(&ca, SERVICE);
    let accepted: Vec<String> = accepted.iter().map(|d| (*d).to_owned()).collect();
    let inbound =
        SpiffeInbound::with_identity(0, Arc::new(server.clone()), callers, None, &accepted, false)
            .unwrap();
    let router = grauwolf_server::with_spiffe_auth(
        router,
        token.map(str::to_owned),
        Users::default(),
        Some(Arc::clone(&inbound.gate)),
    );
    let plain = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let plain_base = format!("http://{}", plain.local_addr().unwrap());
    let served = router.clone();
    tokio::spawn(async move { axum::serve(plain, served).await.unwrap() });
    let secure = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mtls_base = format!("https://localhost:{}", secure.local_addr().unwrap().port());
    tokio::spawn(async move {
        inbound
            .serve(secure, router, std::future::pending())
            .await
            .unwrap()
    });
    Service {
        plain: plain_base,
        mtls: mtls_base,
        ca,
        server,
    }
}

fn client_for(identity: &FakeIdentity) -> reqwest::Client {
    let tls = identity
        .client_tls(&SpiffeMatch::Id(
            grauwolf_spiffe::SpiffeId::parse(SERVICE).unwrap(),
        ))
        .unwrap();
    reqwest::Client::builder()
        .use_preconfigured_tls(tls)
        .build()
        .unwrap()
}

async fn run(
    client: &reqwest::Client,
    base: &str,
    bearer: Option<&str>,
    body: Value,
) -> (u16, Value) {
    let mut request = client.post(format!("{base}/run")).json(&body);
    if let Some(token) = bearer {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.unwrap();
    (
        response.status().as_u16(),
        response.json().await.unwrap_or(Value::Null),
    )
}

const CALLERS: &str = r#"{ "spiffe://agentswork.de/ns/coordinator/sa/coordinator": "coordinator",
                           "spiffe://agentswork.de/ns/batch/*": "batch-jobs" }"#;

#[tokio::test]
async fn a_caller_on_the_mtls_port_is_the_mapped_user() {
    let service = serve(CallerMap::from_json(CALLERS).unwrap(), &["agentswork.de"]).await;
    let coordinator = FakeIdentity::with_ca(&service.ca, COORDINATOR);
    let client = client_for(&coordinator);
    let (status, body) = run(
        &client,
        &service.mtls,
        None,
        json!({ "input": "Who am I?" }),
    )
    .await;
    assert_eq!(
        (status, body),
        (200, json!({ "output": "You are coordinator." }))
    );
    // The same user may be named in the body, another one may not.
    let same = run(
        &client,
        &service.mtls,
        None,
        json!({ "input": "Who am I?", "user_id": "coordinator" }),
    )
    .await;
    assert_eq!(same.0, 200);
    let other = run(
        &client,
        &service.mtls,
        None,
        json!({ "input": "Who am I?", "user_id": "bob" }),
    )
    .await;
    assert_eq!(other.0, 403, "{:?}", other.1);
    // /health stays open on both ports.
    let health = client
        .get(format!("{}/health", service.mtls))
        .send()
        .await
        .unwrap();
    assert_eq!(health.text().await.unwrap(), "ok");
}

#[tokio::test]
async fn an_authenticated_but_unmapped_id_gets_403_and_a_prefix_maps_a_group() {
    let service = serve(CallerMap::from_json(CALLERS).unwrap(), &["agentswork.de"]).await;
    let intruder = FakeIdentity::with_ca(&service.ca, INTRUDER);
    let (status, body) = run(
        &client_for(&intruder),
        &service.mtls,
        None,
        json!({ "input": "Who am I?" }),
    )
    .await;
    assert_eq!(status, 403);
    assert!(body["error"].as_str().unwrap().contains(INTRUDER), "{body}");
    let batch = FakeIdentity::with_ca(&service.ca, "spiffe://agentswork.de/ns/batch/sa/nightly");
    let (status, body) = run(
        &client_for(&batch),
        &service.mtls,
        None,
        json!({ "input": "Who am I?" }),
    )
    .await;
    assert_eq!(
        (status, body),
        (200, json!({ "output": "You are batch-jobs." }))
    );
}

#[tokio::test]
async fn the_mtls_port_refuses_clients_without_a_valid_certificate() {
    let service = serve(CallerMap::from_json(CALLERS).unwrap(), &["agentswork.de"]).await;
    let bare = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    assert!(
        bare.get(format!("{}/health", service.mtls))
            .send()
            .await
            .is_err()
    );
    // A certificate of another trust domain: not accepted, although its CA is trusted by the client.
    let foreign_ca = TestCa::new("other.test");
    let foreign = FakeIdentity::with_ca(&foreign_ca, "spiffe://other.test/ns/x/sa/x");
    foreign.trust(&service.ca);
    assert!(
        client_for(&foreign)
            .get(format!("{}/health", service.mtls))
            .send()
            .await
            .is_err()
    );
    drop(service.server);
}

#[tokio::test]
async fn a_jwt_svid_on_the_http_port_is_validated_and_mapped() {
    let service = serve(CallerMap::from_json(CALLERS).unwrap(), &["agentswork.de"]).await;
    let plain = reqwest::Client::new();
    let coordinator = FakeIdentity::with_ca(&service.ca, COORDINATOR);
    let good = grauwolf_spiffe::WorkloadIdentity::jwt_svid(&coordinator, SERVICE)
        .await
        .unwrap();
    let ask = json!({ "input": "Who am I?" });
    let (status, body) = run(&plain, &service.plain, Some(good.token()), ask.clone()).await;
    assert_eq!(
        (status, body),
        (200, json!({ "output": "You are coordinator." }))
    );
    let wrong_audience = grauwolf_spiffe::WorkloadIdentity::jwt_svid(&coordinator, "someone-else")
        .await
        .unwrap();
    assert_eq!(
        run(
            &plain,
            &service.plain,
            Some(wrong_audience.token()),
            ask.clone()
        )
        .await
        .0,
        401
    );
    let expired = test_token(COORDINATOR, SERVICE, 1_000, 300);
    assert_eq!(
        run(&plain, &service.plain, Some(&expired), ask.clone())
            .await
            .0,
        401
    );
    let intruder = FakeIdentity::with_ca(&service.ca, INTRUDER);
    let token = grauwolf_spiffe::WorkloadIdentity::jwt_svid(&intruder, SERVICE)
        .await
        .unwrap();
    assert_eq!(
        run(&plain, &service.plain, Some(token.token()), ask)
            .await
            .0,
        403
    );
}

#[tokio::test]
async fn the_mapping_file_is_read_again_after_a_configmap_style_symlink_swap() {
    let dir = TestDir::new("spiffe-callers-reload");
    for (version, user) in [("v1", "coordinator"), ("v2", "batch-jobs")] {
        std::fs::create_dir(dir.join(version)).unwrap();
        std::fs::write(
            dir.join(version).join("callers.json"),
            json!({ COORDINATOR: user }).to_string(),
        )
        .unwrap();
    }
    std::os::unix::fs::symlink("v1", dir.join("..data")).unwrap();
    std::os::unix::fs::symlink("..data/callers.json", dir.join("callers.json")).unwrap();
    let callers = CallerMap::watch(
        &dir.join("callers.json"),
        std::time::Duration::from_millis(50),
    )
    .unwrap();
    let service = serve(callers, &["agentswork.de"]).await;
    let client = client_for(&FakeIdentity::with_ca(&service.ca, COORDINATOR));
    let ask = json!({ "input": "Who am I?" });
    assert_eq!(
        run(&client, &service.mtls, None, ask.clone()).await.1,
        json!({ "output": "You are coordinator." })
    );
    // Kubernetes swaps the `..data` symlink atomically.
    std::os::unix::fs::symlink("v2", dir.join("..data_tmp")).unwrap();
    std::fs::rename(dir.join("..data_tmp"), dir.join("..data")).unwrap();
    let mut answer = Value::Null;
    for _ in 0..60 {
        answer = run(&client, &service.mtls, None, ask.clone()).await.1;
        if answer == json!({ "output": "You are batch-jobs." }) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(answer, json!({ "output": "You are batch-jobs." }));
}

/// The `caller` of the `agent_starting` event in the SSE stream of `/run/stream`.
async fn starting_caller(
    client: &reqwest::Client,
    base: &str,
    bearer: Option<&str>,
) -> Option<String> {
    let mut request = client
        .post(format!("{base}/run/stream"))
        .json(&json!({ "input": "Who am I?" }));
    if let Some(token) = bearer {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body = response.text().await.unwrap();
    let data = body
        .lines()
        .find(|line| line.starts_with("data: {\"event\":\"agent_starting\""))
        .unwrap_or_else(|| panic!("no agent_starting in {body}"));
    let event: Value = serde_json::from_str(data.trim_start_matches("data: ")).unwrap();
    event["caller"].as_str().map(str::to_owned)
}

#[tokio::test]
async fn agent_starting_names_the_spiffe_id_of_the_caller_but_never_for_a_token_call() {
    let service = serve_with_token(
        CallerMap::from_json(CALLERS).unwrap(),
        &["agentswork.de"],
        Some("secret"),
    )
    .await;
    let coordinator = FakeIdentity::with_ca(&service.ca, COORDINATOR);
    // mTLS port: the ID of the client certificate.
    assert_eq!(
        starting_caller(&client_for(&coordinator), &service.mtls, None)
            .await
            .as_deref(),
        Some(COORDINATOR)
    );
    // HTTP port: the ID of the JWT-SVID (the token itself is never recorded).
    let plain = reqwest::Client::new();
    let jwt = grauwolf_spiffe::WorkloadIdentity::jwt_svid(&coordinator, SERVICE)
        .await
        .unwrap();
    assert_eq!(
        starting_caller(&plain, &service.plain, Some(jwt.token()))
            .await
            .as_deref(),
        Some(COORDINATOR)
    );
    // A shared token identifies no workload.
    assert_eq!(
        starting_caller(&plain, &service.plain, Some("secret")).await,
        None
    );
}
