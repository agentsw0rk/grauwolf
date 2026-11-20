//! `fetch` with a workload identity per host (ADR-0110): mTLS with the SPIFFE ID of the server
//! checked, a JWT-SVID as bearer, hosts without identity untouched, no fall back to plain HTTP.

use std::sync::{Arc, Mutex};

use grauwolf_spiffe::testing::{FakeIdentity, TestCa};
use grauwolf_spiffe::{AuthMode, ClientAuth, SpiffeId, SpiffeMatch, TlsIdentity};
use grauwolf_tools::DynTool;
use grauwolf_tools_web::{FetchConfig, fetch_tool};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SERVER: &str = "spiffe://agentswork.de/ns/git/sa/git";
const CLIENT: &str = "spiffe://agentswork.de/ns/reviewer/sa/reviewer";

/// An HTTPS server (client certificate required) that answers every request with `hello` and
/// records the requests (header text).
async fn https_server(ca: &TestCa) -> (u16, Arc<Mutex<Vec<String>>>) {
    let identity = FakeIdentity::with_ca(ca, SERVER);
    let config = identity.server_tls(&["agentswork.de".to_owned()]).unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (acceptor, record) = (acceptor.clone(), Arc::clone(&record));
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                let mut buffer = vec![0; 8192];
                let Ok(read) = tls.read(&mut buffer).await else {
                    return;
                };
                record
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buffer[..read]).into_owned());
                let _ = tls
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello",
                    )
                    .await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (port, seen)
}

fn auth(ca: &TestCa, mode: AuthMode, expected: &str, audience: Option<&str>) -> Arc<ClientAuth> {
    let identity = FakeIdentity::with_ca(ca, CLIENT);
    let expected = SpiffeMatch::Id(SpiffeId::parse(expected).unwrap());
    Arc::new(
        ClientAuth::new(
            Arc::new(identity),
            mode,
            Some(expected),
            audience.map(str::to_owned),
        )
        .unwrap(),
    )
}

fn tool(
    hosts: &[&str],
    allow_http: bool,
    entries: Vec<(&str, Arc<ClientAuth>)>,
) -> Arc<dyn DynTool> {
    fetch_tool(FetchConfig {
        allowed_hosts: hosts.iter().map(|h| (*h).to_owned()).collect(),
        allow_http,
        auth: entries
            .into_iter()
            .map(|(host, auth)| (host.to_owned(), auth))
            .collect(),
    })
}

async fn fetch(tool: &Arc<dyn DynTool>, url: &str) -> String {
    match tool.call_json(json!({ "url": url })).await {
        Ok(text) => text,
        Err(error) => error.to_string(),
    }
}

#[tokio::test]
async fn an_mtls_host_with_the_right_id_is_fetched() {
    let ca = TestCa::new("agentswork.de");
    let (port, _) = https_server(&ca).await;
    let tool = tool(
        &["127.0.0.1"],
        false,
        vec![("127.0.0.1", auth(&ca, AuthMode::Mtls, SERVER, None))],
    );
    let text = fetch(&tool, &format!("https://127.0.0.1:{port}/x")).await;
    assert!(text.starts_with("HTTP 200 OK"), "{text}");
    assert!(text.ends_with("hello"), "{text}");
}

#[tokio::test]
async fn a_wrong_server_id_is_reported_as_a_tls_refusal() {
    let ca = TestCa::new("agentswork.de");
    let (port, _) = https_server(&ca).await;
    let wrong = "spiffe://agentswork.de/ns/x/sa/x";
    let tool = tool(
        &["127.0.0.1"],
        false,
        vec![("127.0.0.1", auth(&ca, AuthMode::Mtls, wrong, None))],
    );
    let text = fetch(&tool, &format!("https://127.0.0.1:{port}/x")).await;
    assert!(
        text.contains("TLS handshake with 127.0.0.1 failed"),
        "{text}"
    );
    assert!(
        text.contains(&format!("the server is {SERVER}, expected {wrong}")),
        "{text}"
    );
}

#[tokio::test]
async fn jwt_mode_sends_a_bearer_token_and_never_shows_it_to_the_model() {
    let ca = TestCa::new("agentswork.de");
    let (port, seen) = https_server(&ca).await;
    let tool = tool(
        &["127.0.0.1"],
        false,
        vec![("127.0.0.1", auth(&ca, AuthMode::Jwt, SERVER, Some("git")))],
    );
    let text = fetch(&tool, &format!("https://127.0.0.1:{port}/x")).await;
    assert!(text.starts_with("HTTP 200 OK"), "{text}");
    let request = seen.lock().unwrap()[0].to_ascii_lowercase();
    assert!(request.contains("authorization: bearer "), "{request}");
    assert!(!text.to_ascii_lowercase().contains("bearer"), "{text}");
}

#[tokio::test]
async fn a_host_without_auth_sends_no_authorization() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("plain"))
        .mount(&server)
        .await;
    let ca = TestCa::new("agentswork.de");
    let tool = tool(
        &["127.0.0.1", "example.test"],
        true,
        vec![(
            "example.test",
            auth(&ca, AuthMode::Jwt, SERVER, Some("git")),
        )],
    );
    let text = fetch(&tool, &server.uri()).await;
    assert!(text.ends_with("plain"), "{text}");
    let requests = server.received_requests().await.unwrap();
    assert!(!requests[0].headers.contains_key("authorization"));
}

#[tokio::test]
async fn an_identity_host_is_never_called_over_plain_http() {
    let ca = TestCa::new("agentswork.de");
    let tool = tool(
        &["127.0.0.1"],
        true,
        vec![("127.0.0.1", auth(&ca, AuthMode::Mtls, SERVER, None))],
    );
    let text = fetch(&tool, "http://127.0.0.1:9/x").await;
    assert!(text.contains("only works over https://"), "{text}");
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

/// Every host is the loopback address.
struct Loopback;

impl grauwolf_tools_web::Resolver for Loopback {
    fn resolve<'a>(
        &'a self,
        _host: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<std::net::IpAddr>, String>> + Send + 'a>,
    > {
        Box::pin(async { Ok(vec![std::net::Ipv4Addr::LOCALHOST.into()]) })
    }
}

#[tokio::test]
async fn the_txn_token_goes_to_an_identity_host_and_never_to_an_external_one() {
    let ca = TestCa::new("agentswork.de");
    let (port, seen) = https_server(&ca).await;
    let external = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("plain"))
        .mount(&external)
        .await;
    let tool = grauwolf_tools_web::fetch_tool_with(
        FetchConfig {
            allowed_hosts: vec!["git.internal".into(), "external.test".into()],
            allow_http: true,
            auth: vec![(
                "git.internal".into(),
                auth(&ca, AuthMode::Mtls, SERVER, None),
            )],
        },
        Arc::new(Loopback),
    );
    let external_port = external.address().port();
    let person: Option<Arc<dyn grauwolf_tools::Delegation>> = Some(Arc::new(Person));
    let (internal, outside) = grauwolf_tools::with_delegation(person, async {
        (
            fetch(&tool, &format!("https://git.internal:{port}/x")).await,
            fetch(&tool, &format!("http://external.test:{external_port}/x")).await,
        )
    })
    .await;
    assert!(internal.starts_with("HTTP 200 OK"), "{internal}");
    assert!(outside.ends_with("plain"), "{outside}");
    let request = seen.lock().unwrap()[0].to_ascii_lowercase();
    assert!(request.contains("txn-token: txn-alice"), "{request}");
    assert!(!internal.contains("txn-alice"), "never shown to the model");
    let requests = external.received_requests().await.unwrap();
    assert!(!requests[0].headers.contains_key("txn-token"));
}
