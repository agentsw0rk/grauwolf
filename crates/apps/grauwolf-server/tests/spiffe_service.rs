//! The routes `service::serve` builds for the main port and the mTLS port (ADR-0109): A2A is
//! reachable over mTLS on the main mTLS port, and its agent card names that port.
#![cfg(feature = "service")]

use std::sync::Arc;

use grauwolf_a2a::{A2aClient, AuthMode, ClientAuth, DEFAULT_READ_TIMEOUT};
use grauwolf_agents::memory::MemoryChatHistoryStore;
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_server::service::{ServiceConfig, routers_with_history};
use grauwolf_server::{CallerMap, SpiffeInbound};
use grauwolf_spec::{AgentSpec, AgentSpecExt};
use grauwolf_spiffe::SpiffeMatch;
use grauwolf_spiffe::testing::{FakeIdentity, TestCa};
use grauwolf_test::MockLlmClient;
use serde_json::{Value, json};

const REVIEWER: &str = "spiffe://agentswork.de/ns/reviewer/sa/reviewer";
const COORDINATOR: &str = "spiffe://agentswork.de/ns/coordinator/sa/coordinator";
const INTRUDER: &str = "spiffe://agentswork.de/ns/intruder/sa/intruder";

struct Running {
    plain: String,
    mtls: String,
    ca: TestCa,
}

/// A service as `serve` builds it with `GRAUWOLF_SPIFFE=1`, listening on ephemeral ports.
async fn running(env: &[(&str, &str)]) -> Running {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "reviewer",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("ollama", Arc::new(MockLlmClient::new("Looks good.")))
            .build(),
    ))
    .load(&spec)
    .unwrap();
    let ca = TestCa::new("agentswork.de");
    let callers = CallerMap::from_json(&json!({ COORDINATOR: "coordinator" }).to_string()).unwrap();
    let plain = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let secure = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = secure.local_addr().unwrap().port();
    let inbound = Arc::new(
        SpiffeInbound::with_identity(
            port,
            Arc::new(FakeIdentity::with_ca(&ca, REVIEWER)),
            callers,
            None,
            &["agentswork.de".to_owned()],
            false,
        )
        .unwrap(),
    );
    let plain_base = format!("http://{}", plain.local_addr().unwrap());
    let mtls_base = format!("https://localhost:{port}");
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .chain([
            ("GRAUWOLF_SPIFFE".to_owned(), "1".to_owned()),
            ("GRAUWOLF_PUBLIC_URL".to_owned(), plain_base.clone()),
        ])
        .collect();
    let mut config = ServiceConfig::from_env_with(|name| {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    })
    .unwrap();
    config.spiffe_inbound = Some(Arc::clone(&inbound));
    let routers = routers_with_history(
        Arc::new(agent),
        &spec,
        &config,
        Arc::new(MemoryChatHistoryStore::default()),
    )
    .await
    .unwrap();
    let main = routers.main;
    tokio::spawn(async move { axum::serve(plain, main).await.unwrap() });
    let mtls = routers.mtls.expect("routes for the mTLS port");
    tokio::spawn(async move {
        inbound
            .serve(secure, mtls, std::future::pending())
            .await
            .unwrap()
    });
    Running {
        plain: plain_base,
        mtls: mtls_base,
        ca,
    }
}

fn auth(identity: &FakeIdentity) -> Arc<ClientAuth> {
    let expected = SpiffeMatch::Id(grauwolf_spiffe::SpiffeId::parse(REVIEWER).unwrap());
    Arc::new(
        ClientAuth::new(
            Arc::new(identity.clone()),
            AuthMode::Mtls,
            Some(expected),
            None,
        )
        .unwrap(),
    )
}

async fn endpoint_of_card(base: &str) -> String {
    let card: Value = reqwest::get(format!("{base}/.well-known/agent-card.json"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    card["supportedInterfaces"][0]["url"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn a2a_is_reachable_over_mtls_and_the_cards_of_both_ports_name_their_own_endpoint() {
    let service = running(&[]).await;
    assert_eq!(
        endpoint_of_card(&service.plain).await,
        format!("{}/a2a", service.plain)
    );
    let coordinator = FakeIdentity::with_ca(&service.ca, COORDINATOR);
    // Over mTLS the client follows the card to the mTLS endpoint, not to the plain port.
    let client = A2aClient::connect_with_auth(
        &service.mtls,
        &[],
        DEFAULT_READ_TIMEOUT,
        Some(auth(&coordinator)),
    )
    .await
    .unwrap();
    assert_eq!(client.send("Review this.").await.unwrap(), "Looks good.");
    // An authenticated caller that is not mapped is refused on this port too.
    let intruder = FakeIdentity::with_ca(&service.ca, INTRUDER);
    let client = A2aClient::connect_with_auth(
        &service.mtls,
        &[],
        DEFAULT_READ_TIMEOUT,
        Some(auth(&intruder)),
    )
    .await
    .unwrap();
    let error = client.send("Review this.").await.unwrap_err().to_string();
    assert!(error.contains("403"), "{error}");
}

#[tokio::test]
async fn the_public_url_of_the_mtls_port_comes_from_the_environment() {
    // The card is fetched over the real mTLS port, but names the address callers use.
    let service = running(&[(
        "GRAUWOLF_SPIFFE_PUBLIC_URL",
        "https://reviewer.example:8443",
    )])
    .await;
    let coordinator = FakeIdentity::with_ca(&service.ca, COORDINATOR);
    let tls = grauwolf_spiffe::TlsIdentity::client_tls(
        &coordinator,
        &SpiffeMatch::Id(grauwolf_spiffe::SpiffeId::parse(REVIEWER).unwrap()),
    )
    .unwrap();
    let client = reqwest::Client::builder()
        .use_preconfigured_tls(tls)
        .build()
        .unwrap();
    let card: Value = client
        .get(format!("{}/.well-known/agent-card.json", service.mtls))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        card["supportedInterfaces"][0]["url"],
        "https://reviewer.example:8443/a2a"
    );
}
