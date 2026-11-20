//! Txn-Tokens signed by OpenBao Transit (a mocked OpenBao that signs with a test key).

use std::sync::Arc;

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use grauwolf_identity::{TokenSigner, TransitSigner, TxnClaims, check, issue};
use grauwolf_openbao::{Bao, HttpTransport, StaticToken};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Signs what it is sent, like `transit/sign` with JWS marshaling.
struct Signing(Arc<EcdsaKeyPair>);

impl Respond for Signing {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["hash_algorithm"], "sha2-256");
        assert_eq!(body["marshaling_algorithm"], "jws");
        assert_eq!(body["key_version"], 3);
        let input = STANDARD.decode(body["input"].as_str().unwrap()).unwrap();
        let signature = self.0.sign(&SystemRandom::new(), &input).unwrap();
        ResponseTemplate::new(200).set_body_json(json!({ "data": {
            "signature": format!("vault:v3:{}", URL_SAFE_NO_PAD.encode(signature.as_ref())),
            "key_version": 3
        }}))
    }
}

/// The PEM `PUBLIC KEY` of a P-256 point, as OpenBao prints it.
fn pem(point: &[u8]) -> String {
    let mut der = vec![
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08,
        0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    ];
    der.extend_from_slice(point);
    format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
        STANDARD.encode(der)
    )
}

#[tokio::test]
async fn transit_signs_txn_tokens_that_check_out_with_its_published_key() {
    let pair = Arc::new(EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING).unwrap());
    let old = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING).unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/transit/keys/txn-token"))
        .and(header("X-Vault-Token", "bao-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": {
            "type": "ecdsa-p256",
            "latest_version": 3,
            "keys": {
                "2": { "public_key": pem(old.public_key().as_ref()) },
                "3": { "public_key": pem(pair.public_key().as_ref()) }
            }
        }})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/transit/sign/txn-token"))
        .and(header("X-Vault-Token", "bao-token"))
        .respond_with(Signing(Arc::clone(&pair)))
        .mount(&server)
        .await;
    let signer = TransitSigner::new(
        Bao::new(HttpTransport::new(&server.uri())),
        Arc::new(StaticToken("bao-token".into())),
        "transit",
        "txn-token",
    );
    assert_eq!(signer.key_id().await.unwrap(), "txn-token-v3");
    let keys = signer.public_keys().await.unwrap();
    let kids: Vec<_> = keys.iter().map(|key| key.kid.clone().unwrap()).collect();
    assert_eq!(
        kids,
        ["txn-token-v2", "txn-token-v3"],
        "older versions stay for rotation"
    );

    let claims = TxnClaims {
        iss: "https://identity".into(),
        aud: "agents.local".into(),
        sub: "oid-alice".into(),
        name: None,
        upn: None,
        txn: "t".into(),
        purp: None,
        azd: None,
        req_wl: "spiffe://agents.local/a".into(),
        iat: 100,
        exp: 1000,
        chain_exp: 2000,
    };
    let token = issue(&signer, &claims).await.unwrap();
    assert_eq!(
        check(&token, &keys[1], "agents.local", None, 500).unwrap(),
        claims
    );
    assert!(check(&token, &keys[0], "agents.local", None, 500).is_err());
}

#[tokio::test]
async fn an_unreachable_openbao_makes_signing_unavailable() {
    let signer = TransitSigner::new(
        Bao::new(HttpTransport::new("http://127.0.0.1:9")),
        Arc::new(StaticToken("t".into())),
        "transit",
        "txn-token",
    );
    let error = signer.key_id().await.unwrap_err();
    assert_eq!(error.status(), 503);
}
