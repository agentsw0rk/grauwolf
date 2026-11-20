//! The Workload API client against a fake Workload API server on a Unix socket.

use std::time::Duration;

use grauwolf_config::MapEnv;
use grauwolf_spiffe::testing::{FakeIdentity, FakeWorkloadApi, test_token_claims};
use grauwolf_spiffe::{SpiffeError, WorkloadApiClient, WorkloadIdentity};
use grauwolf_test::TestDir;

const ID: &str = "spiffe://agentswork.de/ns/agents/sa/reviewer";

#[tokio::test]
async fn fetches_a_jwt_svid_for_an_audience() {
    let dir = TestDir::new("spiffe-fetch");
    let api = FakeWorkloadApi::start(&dir.join("agent.sock"), ID, true);
    let client = WorkloadApiClient::from_endpoint(&api.endpoint()).unwrap();
    let svid = client.jwt_svid("openbao").await.unwrap();
    assert_eq!(svid.spiffe_id(), ID);
    let claims = test_token_claims(svid.token());
    assert_eq!(claims["sub"], ID);
    assert_eq!(claims["aud"][0], "openbao");
    assert_eq!(api.requests(), vec![vec!["openbao".to_owned()]]);
}

#[tokio::test]
async fn a_workload_without_an_entry_is_refused_with_the_status() {
    let dir = TestDir::new("spiffe-refused");
    let api = FakeWorkloadApi::start(&dir.join("agent.sock"), ID, false);
    let client = WorkloadApiClient::from_endpoint(&api.endpoint())
        .unwrap()
        .with_fetch_timeout(Duration::ZERO);
    let error = client.jwt_svid("openbao").await.unwrap_err();
    assert!(
        matches!(error, SpiffeError::Refused { status: 7, .. }),
        "{error}"
    );
}

#[tokio::test]
async fn a_missing_socket_is_unavailable_and_names_it() {
    let dir = TestDir::new("spiffe-missing");
    let client = WorkloadApiClient::new(dir.join("none.sock")).with_fetch_timeout(Duration::ZERO);
    let error = client.jwt_svid("openbao").await.unwrap_err().to_string();
    assert!(error.contains("none.sock"), "{error}");
}

#[test]
fn the_endpoint_comes_from_the_environment() {
    let env = MapEnv::new([(
        "SPIFFE_ENDPOINT_SOCKET",
        "unix:///run/spire/sockets/agent.sock",
    )]);
    let client = WorkloadApiClient::from_env(&env).unwrap().unwrap();
    assert_eq!(
        client.socket(),
        std::path::Path::new("/run/spire/sockets/agent.sock")
    );
    assert!(
        WorkloadApiClient::from_env(&MapEnv::default())
            .unwrap()
            .is_none()
    );
    let bad = MapEnv::new([("SPIFFE_ENDPOINT_SOCKET", "tcp://127.0.0.1:8081")]);
    assert!(matches!(
        WorkloadApiClient::from_env(&bad),
        Err(SpiffeError::BadEndpoint(_))
    ));
}

#[tokio::test]
async fn the_fake_identity_records_audiences_and_hides_the_token_in_debug() {
    let identity = FakeIdentity::new(ID);
    let svid = identity.jwt_svid("a").await.unwrap();
    assert_eq!(identity.audiences(), vec!["a".to_owned()]);
    assert!(!format!("{svid:?}").contains(svid.token()));
    identity.fail_with(Some("down"));
    assert!(identity.jwt_svid("a").await.is_err());
}

#[tokio::test]
async fn waits_for_the_registration_entry_instead_of_failing_at_once() {
    let dir = TestDir::new("spiffe-retry");
    // The first two calls: "no identity issued", as before the agent has synced the entry.
    let api = FakeWorkloadApi::start_denying(&dir.join("agent.sock"), ID, 2);
    let client = WorkloadApiClient::from_endpoint(&api.endpoint())
        .unwrap()
        .with_first_backoff(Duration::from_millis(20))
        .with_fetch_timeout(Duration::from_secs(10));
    let svid = client.jwt_svid("openbao").await.unwrap();
    assert_eq!(svid.spiffe_id(), ID);
    assert_eq!(
        api.requests().len(),
        1,
        "only the answered call is recorded"
    );
}

#[tokio::test]
async fn waits_for_a_socket_that_appears_later() {
    let dir = TestDir::new("spiffe-late-socket");
    let socket = dir.join("agent.sock");
    let client = WorkloadApiClient::new(&socket)
        .with_first_backoff(Duration::from_millis(20))
        .with_fetch_timeout(Duration::from_secs(10));
    let late = socket.clone();
    let start = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        FakeWorkloadApi::start(&late, ID, true)
    });
    let svid = client.jwt_svid("openbao").await.unwrap();
    assert_eq!(svid.spiffe_id(), ID);
    drop(start.await.unwrap());
}

#[tokio::test]
async fn gives_up_at_the_deadline_and_says_how_long_it_waited() {
    let dir = TestDir::new("spiffe-deadline");
    let api = FakeWorkloadApi::start(&dir.join("agent.sock"), ID, false);
    let client = WorkloadApiClient::from_endpoint(&api.endpoint())
        .unwrap()
        .with_first_backoff(Duration::from_millis(20))
        .with_fetch_timeout(Duration::from_millis(300));
    let error = client.jwt_svid("openbao").await.unwrap_err();
    assert!(matches!(error, SpiffeError::GaveUp { .. }), "{error}");
    let text = error.to_string();
    assert!(
        text.contains("status 7") && text.contains("gave up after waiting"),
        "{text}"
    );
}

#[test]
fn the_deadline_comes_from_the_environment() {
    let ok = MapEnv::new([
        ("SPIFFE_ENDPOINT_SOCKET", "unix:///s"),
        ("SPIFFE_FETCH_TIMEOUT", "1m"),
    ]);
    assert!(WorkloadApiClient::from_env(&ok).unwrap().is_some());
    let bad = MapEnv::new([
        ("SPIFFE_ENDPOINT_SOCKET", "unix:///s"),
        ("SPIFFE_FETCH_TIMEOUT", "soon"),
    ]);
    assert!(matches!(
        WorkloadApiClient::from_env(&bad),
        Err(SpiffeError::BadTimeout(_))
    ));
}
