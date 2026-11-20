//! `grauwolf login`'s device code flow (RFC 8628) against a mocked token endpoint.

use std::time::Duration;

use grauwolf_identity::{DeviceEndpoints, DeviceLogin};
use serde_json::json;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TENANT: &str = "11111111-1111-1111-1111-111111111111";

fn login(server: &MockServer) -> DeviceLogin {
    DeviceLogin::new(
        DeviceEndpoints::entra(&server.uri(), TENANT),
        "cli-app",
        "api://grauwolf-identity/access_as_user offline_access",
    )
    .unwrap()
    .with_slow_down_step(Duration::from_millis(10))
}

async fn device_code(server: &MockServer, interval: u64) {
    Mock::given(method("POST"))
        .and(path(format!("/{TENANT}/oauth2/v2.0/devicecode")))
        .and(body_string_contains("client_id=cli-app"))
        .and(body_string_contains(
            "scope=api%3A%2F%2Fgrauwolf-identity%2Faccess_as_user+offline_access",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code": "secret-device-code", "user_code": "BCDF-GHJK",
            "verification_uri": "https://login.example/devicecode",
            "expires_in": 60, "interval": interval})))
        .mount(server)
        .await;
}

fn token_path() -> String {
    format!("/{TENANT}/oauth2/v2.0/token")
}

fn oauth_error(code: &str) -> ResponseTemplate {
    ResponseTemplate::new(400)
        .set_body_json(json!({"error": code, "error_description": "AADSTS: details"}))
}

#[tokio::test]
async fn the_tokens_come_after_the_person_signed_in_while_pending_and_slow_down_wait() {
    let server = MockServer::start().await;
    device_code(&server, 0).await;
    Mock::given(method("POST"))
        .and(path(token_path()))
        .respond_with(oauth_error("authorization_pending"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(token_path()))
        .respond_with(oauth_error("slow_down"))
        .up_to_n_times(1)
        .with_priority(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(token_path()))
        .and(body_string_contains(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code",
        ))
        .and(body_string_contains("device_code=secret-device-code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "access-1", "refresh_token": "refresh-1", "expires_in": 3599})))
        .with_priority(3)
        .mount(&server)
        .await;

    let login = login(&server);
    let code = login.start().await.unwrap();
    assert_eq!(code.user_code, "BCDF-GHJK");
    assert_eq!(code.verification_uri, "https://login.example/devicecode");
    assert!(!format!("{code:?}").contains("secret-device-code"));
    let tokens = login.wait(&code).await.unwrap();
    assert_eq!(tokens.access_token, "access-1");
    assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-1"));
    assert_eq!(tokens.expires_in, 3599);
    assert!(!format!("{tokens:?}").contains("access-1"));
    // Three polls: pending, slow down, the tokens.
    let polls = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path() == token_path())
        .count();
    assert_eq!(polls, 3);
}

#[tokio::test]
async fn an_expired_or_declined_code_ends_the_wait_with_an_error() {
    for (code, expected) in [
        ("expired_token", "expired"),
        ("access_denied", "declined"),
        ("invalid_client", "invalid_client"),
    ] {
        let server = MockServer::start().await;
        device_code(&server, 0).await;
        Mock::given(method("POST"))
            .and(path(token_path()))
            .respond_with(oauth_error(code))
            .mount(&server)
            .await;
        let login = login(&server);
        let started = login.start().await.unwrap();
        let error = login.wait(&started).await.unwrap_err();
        assert!(error.contains(expected), "{code}: {error}");
        assert!(!error.contains("secret-device-code"), "{error}");
    }
}

#[tokio::test]
async fn a_refresh_returns_new_tokens_and_keeps_a_refresh_token_that_was_not_rotated() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(token_path()))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=refresh-1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": "access-2", "expires_in": 600})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(token_path()))
        .and(body_string_contains("refresh_token=revoked"))
        .respond_with(oauth_error("invalid_grant"))
        .mount(&server)
        .await;
    let login = login(&server);
    let tokens = login.refresh("refresh-1").await.unwrap();
    assert_eq!(tokens.access_token, "access-2");
    assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-1"));
    let error = login.refresh("revoked").await.unwrap_err();
    assert!(error.contains("invalid_grant"), "{error}");
    assert!(!error.contains("revoked"), "{error}");
}

#[tokio::test]
async fn a_refused_device_authorization_names_the_oauth_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/{TENANT}/oauth2/v2.0/devicecode")))
        .respond_with(oauth_error("invalid_scope"))
        .mount(&server)
        .await;
    let error = login(&server).start().await.unwrap_err();
    assert!(error.contains("invalid_scope"), "{error}");
}
