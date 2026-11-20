//! Agent to agent with SPIFFE through the full stack (ADR-0109): the A2A client of the
//! coordinator (mTLS or JWT-SVID, test identity) against the A2A server of the reviewer behind
//! the mTLS listener and the SPIFFE gate.

use std::sync::{Arc, Mutex};

use grauwolf_a2a::{A2aClient, AuthMode, ClientAuth, DEFAULT_READ_TIMEOUT};
use grauwolf_agents::events::{AgentEvent, Feature};
use grauwolf_agents::memory::{ChatHistoryStore, MemoryChatHistoryStore};
use grauwolf_host::config::{MapEnv, SpiffeServiceConfig};
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_server::a2a::AgentCardInfo;
use grauwolf_server::{CallerMap, SpiffeInbound, Users};
use grauwolf_spec::{AgentSpec, AgentSpecExt};
use grauwolf_spiffe::SpiffeMatch;
use grauwolf_spiffe::testing::{FakeIdentity, TestCa};
use grauwolf_test::MockLlmClient;
use serde_json::json;

const REVIEWER: &str = "spiffe://agentswork.de/ns/reviewer/sa/reviewer";
const COORDINATOR: &str = "spiffe://agentswork.de/ns/coordinator/sa/coordinator";
const INTRUDER: &str = "spiffe://agentswork.de/ns/intruder/sa/intruder";

/// Remembers the `caller` of every `agent_starting`.
#[derive(Default)]
struct Callers(Mutex<Vec<Option<String>>>);

impl Feature for Callers {
    fn on_event(&self, event: &AgentEvent) {
        if let AgentEvent::AgentStarting { caller, .. } = event {
            self.0.lock().unwrap().push(caller.clone());
        }
    }
}

/// The reviewer: its A2A routes on a plain and an mTLS port. Returns both base URLs, the CA and the callers its runs saw.
async fn reviewer() -> (String, String, TestCa, Arc<Callers>) {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "reviewer",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let executor = PromptExecutor::builder()
        .client("ollama", Arc::new(MockLlmClient::new("Looks good.")))
        .build();
    let seen = Arc::new(Callers::default());
    let agent = Arc::new(
        Interpreter::new(Arc::new(executor))
            .with_feature(seen.clone())
            .load(&spec)
            .unwrap(),
    );
    let ca = TestCa::new("agentswork.de");
    let identity = FakeIdentity::with_ca(&ca, REVIEWER);
    let callers = CallerMap::from_json(&json!({ COORDINATOR: "coordinator" }).to_string()).unwrap();
    let inbound = SpiffeInbound::with_identity(
        0,
        Arc::new(identity),
        callers,
        None,
        &["agentswork.de".to_owned()],
        false,
    )
    .unwrap();
    let plain = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let secure = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let plain_base = format!("http://{}", plain.local_addr().unwrap());
    let mtls_base = format!("https://localhost:{}", secure.local_addr().unwrap().port());
    // The agent card of each port names that port as its endpoint.
    let history = Arc::new(MemoryChatHistoryStore::default()) as Arc<dyn ChatHistoryStore>;
    let build = |base: &str| {
        grauwolf_server::with_spiffe_auth(
            grauwolf_server::a2a::router(
                Arc::clone(&agent),
                Arc::clone(&history),
                AgentCardInfo::from_spec(&spec, base),
            ),
            None,
            Users::default(),
            Some(Arc::clone(&inbound.gate)),
        )
    };
    let (plain_router, mtls_router) = (build(&plain_base), build(&mtls_base));
    tokio::spawn(async move { axum::serve(plain, plain_router).await.unwrap() });
    tokio::spawn(async move {
        inbound
            .serve(secure, mtls_router, std::future::pending())
            .await
            .unwrap()
    });
    (plain_base, mtls_base, ca, seen)
}

fn auth(
    identity: &FakeIdentity,
    mode: AuthMode,
    expected: Option<&str>,
    audience: Option<&str>,
) -> Arc<ClientAuth> {
    let expected =
        expected.map(|id| SpiffeMatch::Id(grauwolf_spiffe::SpiffeId::parse(id).unwrap()));
    Arc::new(
        ClientAuth::new(
            Arc::new(identity.clone()),
            mode,
            expected,
            audience.map(str::to_owned),
        )
        .unwrap(),
    )
}

#[tokio::test]
async fn the_coordinator_calls_the_reviewer_over_mtls() {
    let (_, mtls, ca, _) = reviewer().await;
    let coordinator = FakeIdentity::with_ca(&ca, COORDINATOR);
    let client = A2aClient::connect_with_auth(
        &mtls,
        &[],
        DEFAULT_READ_TIMEOUT,
        Some(auth(&coordinator, AuthMode::Mtls, Some(REVIEWER), None)),
    )
    .await
    .unwrap();
    assert_eq!(client.send("Review this.").await.unwrap(), "Looks good.");
}

#[tokio::test]
async fn a_wrong_expected_id_fails_the_handshake_and_an_unmapped_caller_gets_403() {
    let (_, mtls, ca, _) = reviewer().await;
    let coordinator = FakeIdentity::with_ca(&ca, COORDINATOR);
    let wrong = auth(
        &coordinator,
        AuthMode::Mtls,
        Some("spiffe://agentswork.de/ns/x/sa/x"),
        None,
    );
    let error = A2aClient::connect_with_auth(&mtls, &[], DEFAULT_READ_TIMEOUT, Some(wrong))
        .await
        .err()
        .unwrap();
    let message = error.to_string();
    assert!(message.contains("A2A over HTTP"), "{message}");
    // The message names the cause, not just a missing connection (found in the live check).
    assert!(message.contains("TLS handshake"), "{message}");
    assert!(
        message.contains(&format!(
            "the server is {REVIEWER}, expected spiffe://agentswork.de/ns/x/sa/x"
        )),
        "{message}"
    );
    assert!(!message.contains("refused or given up"), "{message}");
    // The agent card is open, the message is not: the intruder gets 403 on `send`.
    let intruder = FakeIdentity::with_ca(&ca, INTRUDER);
    let client = A2aClient::connect_with_auth(
        &mtls,
        &[],
        DEFAULT_READ_TIMEOUT,
        Some(auth(&intruder, AuthMode::Mtls, Some(REVIEWER), None)),
    )
    .await
    .unwrap();
    let error = client.send("Review this.").await.unwrap_err().to_string();
    assert!(error.contains("403"), "{error}");
}

#[tokio::test]
async fn the_reviewers_run_knows_which_agent_called_it() {
    let (_, mtls, ca, seen) = reviewer().await;
    let coordinator = FakeIdentity::with_ca(&ca, COORDINATOR);
    let client = A2aClient::connect_with_auth(
        &mtls,
        &[],
        DEFAULT_READ_TIMEOUT,
        Some(auth(&coordinator, AuthMode::Mtls, Some(REVIEWER), None)),
    )
    .await
    .unwrap();
    client.send("Review this.").await.unwrap();
    assert_eq!(*seen.0.lock().unwrap(), [Some(COORDINATOR.to_owned())]);
}

#[tokio::test]
async fn jwt_mode_reaches_the_plain_port() {
    let (plain, _, ca, _) = reviewer().await;
    let coordinator = FakeIdentity::with_ca(&ca, COORDINATOR);
    let client = A2aClient::connect_with_auth(
        &plain,
        &[],
        DEFAULT_READ_TIMEOUT,
        Some(auth(&coordinator, AuthMode::Jwt, None, Some(REVIEWER))),
    )
    .await
    .unwrap();
    assert_eq!(client.send("Review this.").await.unwrap(), "Looks good.");
}

#[tokio::test]
async fn start_up_without_a_workload_api_socket_fails_with_a_clear_message() {
    let settings = SpiffeServiceConfig::from_env(&MapEnv::new([("GRAUWOLF_SPIFFE", "1")])).unwrap();
    assert!(settings.enabled);
    let error = SpiffeInbound::start(&settings, &MapEnv::default(), false)
        .await
        .err()
        .unwrap();
    assert!(error.contains("SPIFFE_ENDPOINT_SOCKET"), "{error}");
}
