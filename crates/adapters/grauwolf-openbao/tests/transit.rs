//! Transit signing and public keys against a mocked OpenBao API (ADR-0111).

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use grauwolf_openbao::{Bao, HttpTransport};
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A P-256 public key as OpenBao prints it (SPKI, PEM).
const PEM: &str = "-----BEGIN PUBLIC KEY-----
MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE54LF5TOl4Z12QtJW6Mlr156fy5fh
z+B2pvq82EU7Z8x6wWJ1odYDvfIHT4rJ+tON5sXO5Z8EQvz39k9USn08OQ==
-----END PUBLIC KEY-----
";

#[tokio::test]
async fn the_public_keys_of_every_version_come_as_points() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/transit/keys/txn"))
        .and(header("X-Vault-Token", "t"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": {
            "type": "ecdsa-p256",
            "latest_version": 2,
            "keys": {
                "1": { "public_key": PEM, "name": "P-256" },
                "2": { "public_key": PEM, "name": "P-256" }
            }
        }})))
        .mount(&server)
        .await;
    let key = Bao::new(HttpTransport::new(&server.uri()))
        .transit_key("t", "transit", "txn")
        .await
        .unwrap();
    assert_eq!(key.latest_version, 2);
    assert_eq!(
        key.public_keys.iter().map(|(v, _)| *v).collect::<Vec<_>>(),
        [1, 2]
    );
    let point = &key.public_keys[0].1;
    assert_eq!((point.len(), point[0]), (65, 4));
}

#[tokio::test]
async fn a_key_of_another_type_is_refused() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/transit/keys/txn"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": {
            "type": "aes256-gcm96", "latest_version": 1, "keys": { "1": 1 }
        }})))
        .mount(&server)
        .await;
    let error = Bao::new(HttpTransport::new(&server.uri()))
        .transit_key("t", "transit", "txn")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("ecdsa-p256"), "{error}");
}

#[tokio::test]
async fn signing_sends_the_input_with_sha256_and_jws_marshaling_and_returns_the_raw_signature() {
    let server = MockServer::start().await;
    let raw = [7_u8; 64];
    Mock::given(method("POST"))
        .and(path("/v1/transit/sign/txn"))
        .and(header("X-Vault-Token", "t"))
        .and(body_partial_json(json!({
            "input": STANDARD.encode(b"header.claims"),
            "hash_algorithm": "sha2-256",
            "marshaling_algorithm": "jws",
            "key_version": 2
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": {
            "signature": format!("vault:v2:{}", URL_SAFE_NO_PAD.encode(raw)),
            "key_version": 2
        }})))
        .expect(1)
        .mount(&server)
        .await;
    let signature = Bao::new(HttpTransport::new(&server.uri()))
        .transit_sign("t", "transit", "txn", 2, b"header.claims")
        .await
        .unwrap();
    assert_eq!(signature, raw);
}

#[tokio::test]
async fn a_refused_signing_names_the_status_and_not_the_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/transit/sign/txn"))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(json!({ "errors": ["permission denied"] })),
        )
        .mount(&server)
        .await;
    let error = Bao::new(HttpTransport::new(&server.uri()))
        .transit_sign("secret-token", "transit", "txn", 1, b"x")
        .await
        .unwrap_err();
    assert_eq!(error.status, Some(403));
    assert!(!error.to_string().contains("secret-token"));
}
