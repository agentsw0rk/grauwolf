//! The identity service's decisions: entry, renewal, policy, errors.

use std::sync::Arc;
use std::time::Duration;

use grauwolf_identity::testing::TrustingPeople;
use grauwolf_identity::{
    ACCESS_TOKEN_TYPE, IdentityError, IdentityPolicy, IdentityService, MemorySigner,
    ServiceSettings, TXN_TOKEN_TYPE, TokenRequest, TokenSigner, TxnClaims, check, issue,
};
use grauwolf_spiffe::SpiffeId;
use serde_json::json;

const ISSUER: &str = "https://identity.agents.local";
const COORDINATOR: &str = "spiffe://agents.local/ns/agents/sa/coordinator";
const REVIEWER: &str = "spiffe://agents.local/ns/agents/sa/reviewer";

fn service(signer: Arc<MemorySigner>, max_chain: Duration) -> IdentityService {
    IdentityService::new(
        ServiceSettings {
            issuer: ISSUER.into(),
            trust_domain: "agents.local".into(),
            person_issuer: "https://entra.example/v2.0".into(),
            lifetime: Duration::from_secs(900),
            max_chain_lifetime: max_chain,
        },
        signer,
        Arc::new(TrustingPeople),
        IdentityPolicy::from_json(&format!(
            r#"{{ "callers": {{ "{COORDINATOR}": {{ "purposes": ["review"] }} }} }}"#
        ))
        .unwrap(),
    )
}

/// An unsigned stand-in for a person's access token (`TrustingPeople` reads its claims).
fn person_token(claims: serde_json::Value) -> String {
    use base64::Engine;
    let b64 = |text: String| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text);
    format!(
        "{}.{}.c2ln",
        b64(json!({ "alg": "RS256" }).to_string()),
        b64(claims.to_string())
    )
}

fn request(token: &str, kind: Option<&str>, purpose: Option<&str>) -> TokenRequest {
    serde_json::from_value(json!({
        "subject_token": token, "subject_token_type": kind, "purpose": purpose
    }))
    .unwrap()
}

fn id(text: &str) -> SpiffeId {
    SpiffeId::parse(text).unwrap()
}

async fn claims_of(signer: &MemorySigner, token: &str) -> TxnClaims {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    check(
        token,
        &signer.public_keys().await.unwrap()[0],
        "agents.local",
        Some(ISSUER),
        now,
    )
    .unwrap()
}

#[tokio::test]
async fn the_entry_issues_a_txn_token_for_the_person_requested_by_the_caller() {
    let signer = Arc::new(MemorySigner::generate("k1"));
    let service = service(Arc::clone(&signer), Duration::from_secs(8 * 3600));
    let alice = person_token(json!({ "oid": "oid-alice", "name": "Alice", "upn": "alice@x" }));
    let answer = service
        .token(
            &id(COORDINATOR),
            request(&alice, Some(ACCESS_TOKEN_TYPE), Some("review")),
        )
        .await
        .unwrap();
    assert_eq!(answer.issued_token_type, TXN_TOKEN_TYPE);
    assert_eq!(answer.expires_in, 900);
    let claims = claims_of(&signer, &answer.access_token).await;
    assert_eq!(claims.sub, "oid-alice");
    assert_eq!(claims.name.as_deref(), Some("Alice"));
    assert_eq!(claims.req_wl, COORDINATOR);
    assert_eq!(claims.purp.as_deref(), Some("review"));
    assert_eq!(claims.iss, ISSUER);
    assert_eq!(claims.txn.len(), 32);
    assert_eq!(claims.chain_exp - claims.iat, 8 * 3600);
}

#[tokio::test]
async fn callers_and_purposes_outside_the_policy_are_forbidden() {
    let service = service(
        Arc::new(MemorySigner::generate("k1")),
        Duration::from_secs(3600),
    );
    let alice = person_token(json!({ "oid": "oid-alice" }));
    let other = service
        .token(&id(REVIEWER), request(&alice, None, None))
        .await;
    assert!(
        matches!(other, Err(IdentityError::Forbidden(_))),
        "{other:?}"
    );
    let purpose = service
        .token(&id(COORDINATOR), request(&alice, None, Some("deploy")))
        .await;
    assert!(
        matches!(purpose, Err(IdentityError::Forbidden(_))),
        "{purpose:?}"
    );
}

#[tokio::test]
async fn an_invalid_person_token_is_unauthorized_and_bad_requests_are_400() {
    let service = service(
        Arc::new(MemorySigner::generate("k1")),
        Duration::from_secs(3600),
    );
    let bad = person_token(json!({ "oid": "oid-alice", "bad": true }));
    let refused = service
        .token(&id(COORDINATOR), request(&bad, None, None))
        .await;
    assert_eq!(refused.unwrap_err().status(), 401);
    let empty = service
        .token(&id(COORDINATOR), request("", None, None))
        .await;
    assert_eq!(empty.unwrap_err().status(), 400);
    let kind = service
        .token(&id(COORDINATOR), request("a.b.c", Some("urn:x"), None))
        .await;
    assert_eq!(kind.unwrap_err().status(), 400);
}

#[tokio::test]
async fn renewal_keeps_the_chain_and_any_workload_of_the_trust_domain_may_renew() {
    let signer = Arc::new(MemorySigner::generate("k1"));
    let service = service(Arc::clone(&signer), Duration::from_secs(8 * 3600));
    let alice = person_token(json!({ "oid": "oid-alice", "name": "Alice" }));
    let first = service
        .token(&id(COORDINATOR), request(&alice, None, Some("review")))
        .await
        .unwrap();
    let first_claims = claims_of(&signer, &first.access_token).await;
    // The kind follows from `iss` when the type is not given.
    let renewed = service
        .token(&id(REVIEWER), request(&first.access_token, None, None))
        .await
        .unwrap();
    let renewed_claims = claims_of(&signer, &renewed.access_token).await;
    assert_eq!(renewed_claims.txn, first_claims.txn);
    assert_eq!(renewed_claims.sub, "oid-alice");
    assert_eq!(renewed_claims.req_wl, COORDINATOR);
    assert_eq!(renewed_claims.chain_exp, first_claims.chain_exp);
    let foreign = service
        .token(
            &id("spiffe://other.local/ns/a/sa/b"),
            request(&first.access_token, Some(TXN_TOKEN_TYPE), None),
        )
        .await;
    assert_eq!(foreign.unwrap_err().status(), 403);
}

#[tokio::test]
async fn renewal_never_goes_beyond_the_chain_lifetime_and_refuses_foreign_or_expired_tokens() {
    let signer = Arc::new(MemorySigner::generate("k1"));
    let service = service(Arc::clone(&signer), Duration::from_secs(120));
    let alice = person_token(json!({ "oid": "oid-alice" }));
    let first = service
        .token(&id(COORDINATOR), request(&alice, None, None))
        .await
        .unwrap();
    assert_eq!(first.expires_in, 120, "capped by the chain");

    let now = claims_of(&signer, &first.access_token).await.iat;
    let mut ended = claims_of(&signer, &first.access_token).await;
    ended.chain_exp = now - 1;
    let ended = issue(signer.as_ref(), &ended).await.unwrap();
    let refused = service
        .token(&id(REVIEWER), request(&ended, Some(TXN_TOKEN_TYPE), None))
        .await;
    assert!(
        refused
            .unwrap_err()
            .to_string()
            .contains("maximum lifetime")
    );

    let mut expired = claims_of(&signer, &first.access_token).await;
    expired.exp = now - 3600;
    let expired = issue(signer.as_ref(), &expired).await.unwrap();
    let refused = service
        .token(&id(REVIEWER), request(&expired, Some(TXN_TOKEN_TYPE), None))
        .await;
    assert_eq!(refused.unwrap_err().status(), 401);

    let stranger = MemorySigner::generate("k1");
    let forged = issue(&stranger, &claims_of(&signer, &first.access_token).await)
        .await
        .unwrap();
    let refused = service
        .token(&id(REVIEWER), request(&forged, Some(TXN_TOKEN_TYPE), None))
        .await;
    assert_eq!(refused.unwrap_err().status(), 401);
}

#[tokio::test]
async fn the_jwks_and_the_config_are_public_and_name_no_secret() {
    let signer = Arc::new(MemorySigner::generate("k1"));
    let service = service(Arc::clone(&signer), Duration::from_secs(3600));
    let jwks = service.jwks().await.unwrap();
    assert_eq!(jwks.keys, vec![signer.jwk()]);
    assert_eq!(
        service.config(),
        json!({
            "issuer": ISSUER, "trust_domain": "agents.local",
            "person_issuer": "https://entra.example/v2.0"
        })
    );
}
