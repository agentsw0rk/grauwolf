//! Workload identity towards an A2A agent (ADR-0109).

use std::sync::Arc;

use grauwolf_a2a::{A2aClient, AuthMode, ClientAuth, DEFAULT_READ_TIMEOUT};
use grauwolf_spiffe::SpiffeMatch;
use grauwolf_spiffe::testing::{FakeIdentity, test_token_claims};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ME: &str = "spiffe://agentswork.de/ns/coordinator/sa/coordinator";

async fn agent() -> MockServer {
    let server = MockServer::start().await;
    let url = server.uri();
    Mock::given(method("GET"))
        .and(path("/.well-known/agent-card.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "reviewer", "description": "d",
            "supportedInterfaces": [ { "protocolBinding": "JSONRPC", "url": format!("{url}/a2a") } ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/a2a"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "message": { "parts": [ { "text": "hi" } ] } }
        })))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn a_jwt_svid_goes_as_bearer_on_every_request() {
    let server = agent().await;
    let identity = FakeIdentity::new(ME);
    let auth = ClientAuth::new(
        Arc::new(identity.clone()),
        AuthMode::Jwt,
        None,
        Some("reviewer".into()),
    )
    .unwrap();
    let client = A2aClient::connect_with_auth(
        &server.uri(),
        &[],
        DEFAULT_READ_TIMEOUT,
        Some(Arc::new(auth)),
    )
    .await
    .unwrap();
    assert_eq!(client.send("hello").await.unwrap(), "hi");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2, "card and message");
    for request in &requests {
        let header = request
            .headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap();
        let token = header.strip_prefix("Bearer ").unwrap();
        let claims = test_token_claims(token);
        assert_eq!(claims["sub"], ME);
        assert_eq!(claims["aud"][0], "reviewer");
    }
    assert_eq!(identity.audiences(), ["reviewer", "reviewer"]);
}

#[tokio::test]
async fn mtls_needs_an_https_url() {
    let server = agent().await;
    let expected = SpiffeMatch::TrustDomain("agentswork.de".into());
    let auth = ClientAuth::new(
        Arc::new(FakeIdentity::new(ME)),
        AuthMode::Mtls,
        Some(expected),
        None,
    )
    .unwrap();
    let error = A2aClient::connect_with_auth(
        &server.uri(),
        &[],
        DEFAULT_READ_TIMEOUT,
        Some(Arc::new(auth)),
    )
    .await
    .err()
    .unwrap()
    .to_string();
    assert!(error.contains("https://"), "{error}");
}

/// A run acting for a person, with a fixed Txn-Token (ADR-0111).
struct Person;

impl grauwolf_tools::Delegation for Person {
    fn txn_token(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
    {
        Box::pin(async { Ok("txn-alice".to_owned()) })
    }
}

fn person() -> Option<Arc<dyn grauwolf_tools::Delegation>> {
    Some(Arc::new(Person))
}

#[tokio::test]
async fn the_txn_token_goes_only_to_an_agent_with_a_workload_identity() {
    let server = agent().await;
    let auth = ClientAuth::new(
        Arc::new(FakeIdentity::new(ME)),
        AuthMode::Jwt,
        None,
        Some("reviewer".into()),
    )
    .unwrap();
    let with_auth = A2aClient::connect_with_auth(
        &server.uri(),
        &[],
        DEFAULT_READ_TIMEOUT,
        Some(Arc::new(auth)),
    )
    .await
    .unwrap();
    let without = A2aClient::connect(&server.uri(), &[]).await.unwrap();
    grauwolf_tools::with_delegation(person(), async {
        with_auth.send("hello").await.unwrap();
        without.send("hello").await.unwrap();
    })
    .await;
    let posts: Vec<_> = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.method.as_str() == "POST")
        .map(|request| {
            request
                .headers
                .get("txn-token")
                .map(|value| value.to_str().unwrap().to_owned())
        })
        .collect();
    assert_eq!(posts, [Some("txn-alice".to_owned()), None]);
}
