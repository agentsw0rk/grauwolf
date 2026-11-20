//! The agent's client of the identity service against a mocked service.

use std::sync::Arc;

use grauwolf_identity::{
    IdentityClient, IdentityError, MemorySigner, RunDelegation, TxnClaims, TxnToken, issue,
};
use grauwolf_tools::Delegation;
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PERSON_ISSUER: &str = "https://entra.example/v2.0";

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn claims(exp: u64) -> TxnClaims {
    TxnClaims {
        iss: "https://identity".into(),
        aud: "agents.local".into(),
        sub: "oid-alice".into(),
        name: Some("Alice".into()),
        upn: None,
        txn: "chain-1".into(),
        purp: None,
        azd: None,
        req_wl: "spiffe://agents.local/ns/agents/sa/coordinator".into(),
        iat: now(),
        exp,
        chain_exp: now() + 3600,
    }
}

async fn identity_service(signer: &MemorySigner) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/identity/jwks.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "keys": [signer.jwk()] })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/identity/config"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": "https://identity", "trust_domain": "agents.local",
            "person_issuer": PERSON_ISSUER
        })))
        .mount(&server)
        .await;
    server
}

fn jwt(claims: serde_json::Value) -> String {
    use base64::Engine;
    let b64 = |text: String| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text);
    format!(
        "{}.{}.c2ln",
        b64(json!({ "alg": "RS256" }).to_string()),
        b64(claims.to_string())
    )
}

#[tokio::test]
async fn a_person_token_is_told_from_a_jwt_svid_by_its_issuer() {
    let signer = MemorySigner::generate("k1");
    let server = identity_service(&signer).await;
    let client = IdentityClient::new(&server.uri(), "agents.local", None).unwrap();
    let entra = jwt(json!({ "iss": PERSON_ISSUER, "sub": "x", "oid": "oid-alice" }));
    let svid = jwt(json!({ "iss": PERSON_ISSUER, "sub": "spiffe://agents.local/a" }));
    let other = jwt(json!({ "iss": "https://elsewhere", "sub": "x" }));
    assert!(client.is_person_token(&entra).await.unwrap());
    assert!(!client.is_person_token(&svid).await.unwrap());
    assert!(!client.is_person_token(&other).await.unwrap());
    assert!(!client.is_person_token("gw_123").await.unwrap());
}

#[tokio::test]
async fn the_exchange_returns_a_verified_token_and_maps_refusals() {
    let signer = MemorySigner::generate("k1");
    let server = identity_service(&signer).await;
    let token = issue(&signer, &claims(now() + 900)).await.unwrap();
    Mock::given(method("POST"))
        .and(path("/api/identity/token"))
        .and(body_partial_json(json!({ "subject_token": "good" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": token, "issued_token_type": "urn:ietf:params:oauth:token-type:txn_token",
            "token_type": "N_A", "expires_in": 900
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/identity/token"))
        .and(body_partial_json(json!({ "subject_token": "bad" })))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({ "error": "expired" })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/identity/token"))
        .and(body_partial_json(json!({ "subject_token": "not-allowed" })))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(json!({ "error": "agent may not delegate" })),
        )
        .mount(&server)
        .await;
    let client = IdentityClient::new(&server.uri(), "agents.local", None).unwrap();
    let exchanged = client.exchange("good").await.unwrap();
    assert_eq!(exchanged.claims.sub, "oid-alice");
    assert_eq!(exchanged.token(), token);
    let refused = client.exchange("bad").await.unwrap_err();
    assert!(
        matches!(refused, IdentityError::Unauthorized(_)),
        "{refused}"
    );
    let forbidden = client.exchange("not-allowed").await.unwrap_err();
    assert!(
        matches!(&forbidden, IdentityError::Forbidden(reason) if reason.contains("agent may not delegate")),
        "{forbidden}"
    );
    let down = IdentityClient::new("http://127.0.0.1:9", "agents.local", None).unwrap();
    assert_eq!(down.exchange("good").await.unwrap_err().status(), 503);
}

#[tokio::test]
async fn a_token_for_another_trust_domain_is_not_accepted() {
    let signer = MemorySigner::generate("k1");
    let server = identity_service(&signer).await;
    let client = IdentityClient::new(&server.uri(), "other.local", None).unwrap();
    let token = issue(&signer, &claims(now() + 900)).await.unwrap();
    assert_eq!(client.verify(&token).await.unwrap_err().status(), 401);
}

#[tokio::test]
async fn the_run_renews_a_token_that_is_about_to_expire() {
    let signer = MemorySigner::generate("k1");
    let server = identity_service(&signer).await;
    let soon = issue(&signer, &claims(now() + 30)).await.unwrap();
    let fresh = issue(&signer, &claims(now() + 900)).await.unwrap();
    Mock::given(method("POST"))
        .and(path("/api/identity/token"))
        .and(body_partial_json(json!({
            "subject_token": soon,
            "subject_token_type": "urn:ietf:params:oauth:token-type:txn_token"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": fresh, "issued_token_type": "urn:ietf:params:oauth:token-type:txn_token",
            "token_type": "N_A", "expires_in": 900
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = Arc::new(IdentityClient::new(&server.uri(), "agents.local", None).unwrap());
    let current = TxnToken::new(soon.clone(), claims(now() + 30));
    let run = RunDelegation::new(client, current);
    assert_eq!(run.txn_token().await.unwrap(), fresh);
    assert_eq!(run.txn_token().await.unwrap(), fresh, "renewed once");
}
