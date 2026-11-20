//! mTLS handshakes with the configurations of the port, against test CAs (no SPIRE).

use std::sync::Arc;
use std::time::Duration;

use grauwolf_spiffe::testing::{FakeIdentity, FakeWorkloadApi, TestCa, test_token};
use grauwolf_spiffe::{
    SpiffeId, SpiffeMatch, SpiffeWorkload, TlsIdentity, WorkloadApiClient, WorkloadIdentity,
    spiffe_id_of_certificate,
};
use grauwolf_test::TestDir;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const SERVER: &str = "spiffe://agentswork.de/ns/reviewer/sa/reviewer";
const CALLER: &str = "spiffe://agentswork.de/ns/coordinator/sa/coordinator";

struct Outcome {
    /// The ID the server saw (None: handshake or exchange failed).
    caller: Option<SpiffeId>,
    /// The certificate the client saw.
    server_certificate: Option<Vec<u8>>,
    error: Option<String>,
}

async fn exchange(
    server: &FakeIdentity,
    accepted: &[&str],
    client: rustls::ClientConfig,
) -> Outcome {
    let accepted: Vec<String> = accepted.iter().map(|d| (*d).to_owned()).collect();
    let acceptor = TlsAcceptor::from(Arc::new(server.server_tls(&accepted).unwrap()));
    let connector = TlsConnector::from(Arc::new(client));
    let (client_io, server_io) = tokio::io::duplex(16 * 1024);
    let serving = tokio::spawn(async move {
        let mut tls = acceptor
            .accept(server_io)
            .await
            .map_err(|e| e.to_string())?;
        let caller = tls
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|chain| chain.first())
            .map(|cert| spiffe_id_of_certificate(cert).unwrap());
        let mut buffer = [0u8; 2];
        tls.read_exact(&mut buffer)
            .await
            .map_err(|e| e.to_string())?;
        tls.write_all(b"ok").await.map_err(|e| e.to_string())?;
        tls.flush().await.map_err(|e| e.to_string())?;
        Ok::<_, String>(caller)
    });
    let name = ServerName::try_from("whatever.invalid").unwrap();
    let result = async {
        let mut tls = connector
            .connect(name, client_io)
            .await
            .map_err(|e| e.to_string())?;
        let certificate = tls.get_ref().1.peer_certificates().unwrap()[0]
            .as_ref()
            .to_vec();
        tls.write_all(b"hi").await.map_err(|e| e.to_string())?;
        let mut buffer = [0u8; 2];
        tls.read_exact(&mut buffer)
            .await
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(certificate)
    }
    .await;
    let served = tokio::time::timeout(Duration::from_secs(5), serving)
        .await
        .unwrap()
        .unwrap();
    match (result, served) {
        (Ok(certificate), Ok(caller)) => Outcome {
            caller,
            server_certificate: Some(certificate),
            error: None,
        },
        (client_result, served) => Outcome {
            caller: None,
            server_certificate: None,
            error: Some(format!(
                "client: {:?}, server: {:?}",
                client_result.err(),
                served.err()
            )),
        },
    }
}

fn expect_id(id: &str) -> SpiffeMatch {
    SpiffeMatch::Id(SpiffeId::parse(id).unwrap())
}

/// Two identities of one trust domain with a shared CA.
fn pair() -> (FakeIdentity, FakeIdentity) {
    let ca = TestCa::new("agentswork.de");
    (
        FakeIdentity::with_ca(&ca, SERVER),
        FakeIdentity::with_ca(&ca, CALLER),
    )
}

#[tokio::test]
async fn mtls_with_the_expected_id_works_and_the_server_sees_the_caller() {
    let (server, caller) = pair();
    let outcome = exchange(
        &server,
        &["agentswork.de"],
        caller.client_tls(&expect_id(SERVER)).unwrap(),
    )
    .await;
    assert_eq!(outcome.error, None);
    assert_eq!(outcome.caller.unwrap().as_str(), CALLER);
}

#[tokio::test]
async fn a_list_or_a_trust_domain_also_matches() {
    let (server, caller) = pair();
    let ids = SpiffeMatch::Ids(vec![
        SpiffeId::parse("spiffe://agentswork.de/ns/other/sa/other").unwrap(),
        SpiffeId::parse(SERVER).unwrap(),
    ]);
    let domain = SpiffeMatch::TrustDomain("agentswork.de".into());
    for expected in [ids, domain] {
        let outcome = exchange(
            &server,
            &["agentswork.de"],
            caller.client_tls(&expected).unwrap(),
        )
        .await;
        assert_eq!(outcome.error, None);
    }
}

#[tokio::test]
async fn a_wrong_expected_id_fails_the_handshake() {
    let (server, caller) = pair();
    let wrong = expect_id("spiffe://agentswork.de/ns/intruder/sa/intruder");
    let outcome = exchange(
        &server,
        &["agentswork.de"],
        caller.client_tls(&wrong).unwrap(),
    )
    .await;
    let error = outcome.error.expect("must fail");
    assert!(
        error.contains("expected spiffe://agentswork.de/ns/intruder"),
        "{error}"
    );
    let other_domain = SpiffeMatch::TrustDomain("other.test".into());
    let outcome = exchange(
        &server,
        &["agentswork.de"],
        caller.client_tls(&other_domain).unwrap(),
    )
    .await;
    assert!(outcome.error.is_some());
}

#[derive(Debug)]
struct AcceptAny;

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[tokio::test]
async fn a_client_without_a_certificate_is_refused() {
    let (server, _) = pair();
    let client = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .unwrap()
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(AcceptAny))
    .with_no_client_auth();
    let outcome = exchange(&server, &["agentswork.de"], client).await;
    assert!(
        outcome.error.is_some(),
        "no client certificate must be refused"
    );
}

#[tokio::test]
async fn a_certificate_of_another_trust_domain_is_refused() {
    let foreign_ca = TestCa::new("other.test");
    let server_ca = TestCa::new("agentswork.de");
    let server = FakeIdentity::with_ca(&server_ca, SERVER);
    let foreign = FakeIdentity::with_ca(&foreign_ca, "spiffe://other.test/ns/x/sa/x");
    foreign.trust(&server_ca);
    let outcome = exchange(
        &server,
        &["agentswork.de"],
        foreign.client_tls(&expect_id(SERVER)).unwrap(),
    )
    .await;
    assert!(
        outcome.error.is_some(),
        "a foreign domain without a bundle must be refused"
    );
    // Even with the bundle, the domain must be accepted: a federated bundle alone is not enough.
    server.trust(&foreign_ca);
    let outcome = exchange(
        &server,
        &["agentswork.de"],
        foreign.client_tls(&expect_id(SERVER)).unwrap(),
    )
    .await;
    assert!(
        outcome.error.is_some(),
        "a domain not in the accepted list must be refused"
    );
    let outcome = exchange(
        &server,
        &["agentswork.de", "other.test"],
        foreign.client_tls(&expect_id(SERVER)).unwrap(),
    )
    .await;
    assert_eq!(outcome.error, None);
    assert_eq!(outcome.caller.unwrap().trust_domain(), "other.test");
}

#[tokio::test]
async fn an_expired_certificate_is_refused() {
    let (server, caller) = pair();
    caller.expire();
    let outcome = exchange(
        &server,
        &["agentswork.de"],
        caller.client_tls(&expect_id(SERVER)).unwrap(),
    )
    .await;
    assert!(outcome.error.is_some());
}

#[tokio::test]
async fn a_rotation_is_served_without_a_restart() {
    let (server, caller) = pair();
    let config = caller.client_tls(&expect_id(SERVER)).unwrap();
    let before = exchange(&server, &["agentswork.de"], config.clone()).await;
    server.rotate();
    let after = exchange(&server, &["agentswork.de"], config).await;
    assert_eq!(before.error, None);
    assert_eq!(after.error, None);
    assert_ne!(before.server_certificate, after.server_certificate);
    assert_eq!(server.certificate(), after.server_certificate.unwrap());
}

#[tokio::test]
async fn the_source_follows_the_workload_api_streams() {
    let dir = TestDir::new("spiffe-source");
    let api = FakeWorkloadApi::start_denying(&dir.join("agent.sock"), SERVER, 0);
    let client = WorkloadApiClient::from_endpoint(&api.endpoint()).unwrap();
    let workload = SpiffeWorkload::connect(client).await.unwrap();
    assert_eq!(workload.id().unwrap().as_str(), SERVER);
    // A caller from the same CA connects to a server that gets its identity from the stream.
    let caller = FakeIdentity::with_ca(api.ca(), CALLER);
    let acceptor = TlsAcceptor::from(Arc::new(
        workload.server_tls(&["agentswork.de".into()]).unwrap(),
    ));
    let connector = TlsConnector::from(Arc::new(caller.client_tls(&expect_id(SERVER)).unwrap()));
    async fn serve_one(acceptor: &TlsAcceptor, connector: &TlsConnector) -> Vec<u8> {
        let (c, s) = tokio::io::duplex(16 * 1024);
        let accept = acceptor.accept(s);
        let connect = connector.connect(ServerName::try_from("x.invalid").unwrap(), c);
        let (served, connected) = tokio::join!(accept, connect);
        served.unwrap();
        connected.unwrap().get_ref().1.peer_certificates().unwrap()[0]
            .as_ref()
            .to_vec()
    }
    let first = serve_one(&acceptor, &connector).await;
    api.rotate();
    let mut second = first.clone();
    for _ in 0..50 {
        second = serve_one(&acceptor, &connector).await;
        if second != first {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_ne!(first, second, "the rotated certificate must be served");
    // A dropped stream is reconnected.
    let opened = api.streams_opened();
    api.disconnect_streams();
    for _ in 0..100 {
        if api.streams_opened() >= opened + 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(api.streams_opened() >= opened + 2, "both streams reconnect");
}

#[tokio::test]
async fn start_up_waits_for_an_identity_that_is_not_there_yet() {
    let dir = TestDir::new("spiffe-source-wait");
    let api = FakeWorkloadApi::start_denying(&dir.join("agent.sock"), SERVER, 3);
    let client = WorkloadApiClient::from_endpoint(&api.endpoint())
        .unwrap()
        .with_first_backoff(Duration::from_millis(10));
    assert!(SpiffeWorkload::connect(client).await.is_ok());
    let missing = WorkloadApiClient::new(dir.join("none.sock")).with_fetch_timeout(Duration::ZERO);
    let error = SpiffeWorkload::connect(missing)
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("none.sock"), "{error}");
}

#[tokio::test]
async fn jwt_svids_are_validated_by_the_workload_api() {
    let dir = TestDir::new("spiffe-validate");
    let api = FakeWorkloadApi::start_denying(&dir.join("agent.sock"), SERVER, 0);
    let client = WorkloadApiClient::from_endpoint(&api.endpoint()).unwrap();
    let caller = FakeIdentity::new(CALLER);
    let token = caller.jwt_svid("reviewer").await.unwrap();
    let id = client
        .validate_jwt_svid(token.token(), "reviewer")
        .await
        .unwrap();
    assert_eq!(id.as_str(), CALLER);
    let wrong_audience = client
        .validate_jwt_svid(token.token(), "other")
        .await
        .unwrap_err();
    assert!(wrong_audience.is_invalid_token(), "{wrong_audience}");
    let expired = test_token(CALLER, "reviewer", 1_000, 300);
    assert!(
        client
            .validate_jwt_svid(&expired, "reviewer")
            .await
            .unwrap_err()
            .is_invalid_token()
    );
    assert!(
        client
            .validate_jwt_svid("garbage", "reviewer")
            .await
            .unwrap_err()
            .is_invalid_token()
    );
    assert_eq!(api.validations().len(), 4);
}

const OTHER: &str = "spiffe://agentswork.de/ns/other/sa/other";

#[tokio::test]
async fn a_certificate_with_two_uri_sans_is_refused_by_both_sides() {
    let ca = TestCa::new("agentswork.de");
    let (server, caller) = (
        FakeIdentity::with_ca(&ca, SERVER),
        FakeIdentity::with_ca(&ca, CALLER),
    );
    // The certificate itself has no single SPIFFE ID.
    let two = FakeIdentity::with_extra_uris(&ca, CALLER, &[OTHER]);
    assert!(
        spiffe_id_of_certificate(&two.certificate())
            .unwrap_err()
            .contains("more than one")
    );
    // The server refuses such a client ...
    let outcome = exchange(
        &server,
        &["agentswork.de"],
        two.client_tls(&expect_id(SERVER)).unwrap(),
    )
    .await;
    assert_eq!(outcome.caller, None);
    let error = outcome.error.expect("the server accepted two URI SANs");
    assert!(error.contains("more than one URI"), "{error}");
    // ... and the client refuses such a server, although the ID it expects is one of the two.
    let two_server = FakeIdentity::with_extra_uris(&ca, SERVER, &[OTHER]);
    let outcome = exchange(
        &two_server,
        &["agentswork.de"],
        caller.client_tls(&expect_id(SERVER)).unwrap(),
    )
    .await;
    let error = outcome.error.expect("the client accepted two URI SANs");
    assert!(error.contains("more than one URI"), "{error}");
}
