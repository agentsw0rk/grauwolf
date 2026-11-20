//! An MCP server over HTTPS whose SPIFFE ID is not the expected one: the client says why the
//! handshake failed instead of reporting a missing connection (found in the live check, ADR-0109).

use std::sync::Arc;

use grauwolf_mcp::McpClient;
use grauwolf_spiffe::testing::{FakeIdentity, TestCa};
use grauwolf_spiffe::{AuthMode, ClientAuth, SpiffeId, SpiffeMatch, TlsIdentity};
use tokio::net::TcpListener;

const SERVER: &str = "spiffe://agentswork.de/ns/tickets/sa/tickets";
const CLIENT: &str = "spiffe://agentswork.de/ns/coordinator/sa/coordinator";

#[tokio::test]
async fn a_wrong_expected_id_names_the_identities_of_the_failed_handshake() {
    let ca = TestCa::new("agentswork.de");
    let server = FakeIdentity::with_ca(&ca, SERVER);
    let config = server.server_tls(&["agentswork.de".to_owned()]).unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            // The client refuses the certificate; the handshake fails on this side too.
            let _ = acceptor.accept(stream).await;
        }
    });

    let client = FakeIdentity::with_ca(&ca, CLIENT);
    let expected = SpiffeMatch::Id(SpiffeId::parse("spiffe://agentswork.de/ns/x/sa/x").unwrap());
    let auth = ClientAuth::new(Arc::new(client), AuthMode::Mtls, Some(expected), None).unwrap();
    let error = McpClient::connect_http_with_auth(
        &format!("https://localhost:{port}/mcp"),
        &[],
        std::time::Duration::from_secs(5),
        Some(Arc::new(auth)),
    )
    .await
    .err()
    .unwrap()
    .to_string();
    assert!(error.contains("TLS handshake"), "{error}");
    assert!(
        error.contains(&format!(
            "the server is {SERVER}, expected spiffe://agentswork.de/ns/x/sa/x"
        )),
        "{error}"
    );
    assert!(!error.contains("refused or given up"), "{error}");
}
