//! The Txn-Token: issued with a signer, checked for signature, type, audience and expiry.

use grauwolf_identity::{IdentityError, MemorySigner, TokenSigner, TxnClaims, check, issue};

const NOW: u64 = 1_900_000_000;

fn claims() -> TxnClaims {
    TxnClaims {
        iss: "https://identity.example".into(),
        aud: "agents.local".into(),
        sub: "oid-alice".into(),
        name: Some("Alice".into()),
        upn: Some("alice@example.org".into()),
        txn: "chain-1".into(),
        purp: Some("review".into()),
        azd: None,
        req_wl: "spiffe://agents.local/ns/agents/sa/coordinator".into(),
        iat: NOW,
        exp: NOW + 900,
        chain_exp: NOW + 8 * 3600,
    }
}

async fn signed(signer: &MemorySigner, claims: &TxnClaims) -> String {
    issue(signer, claims).await.unwrap()
}

#[tokio::test]
async fn an_issued_token_checks_out_with_the_signers_key() {
    let signer = MemorySigner::generate("k1");
    let token = signed(&signer, &claims()).await;
    let key = &signer.public_keys().await.unwrap()[0];
    let checked = check(
        &token,
        key,
        "agents.local",
        Some("https://identity.example"),
        NOW,
    )
    .unwrap();
    assert_eq!(checked, claims());
}

#[tokio::test]
async fn a_manipulated_token_is_refused() {
    let signer = MemorySigner::generate("k1");
    let token = signed(&signer, &claims()).await;
    let key = signer.jwk();
    let mut other = claims();
    other.sub = "oid-mallory".into();
    let forged_claims = signed(&signer, &other).await;
    // The claims of one token with the signature of another.
    let parts: Vec<&str> = token.split('.').collect();
    let forged: Vec<&str> = forged_claims.split('.').collect();
    let spliced = format!("{}.{}.{}", parts[0], forged[1], parts[2]);
    let error = check(&spliced, &key, "agents.local", None, NOW).unwrap_err();
    assert!(matches!(error, IdentityError::Unauthorized(_)), "{error}");
    assert!(error.to_string().contains("signature"));
    // Another key.
    let stranger = MemorySigner::generate("k1").jwk();
    assert!(check(&token, &stranger, "agents.local", None, NOW).is_err());
}

#[tokio::test]
async fn an_expired_token_or_one_for_another_trust_domain_is_refused() {
    let signer = MemorySigner::generate("k1");
    let token = signed(&signer, &claims()).await;
    let key = signer.jwk();
    let expired = check(&token, &key, "agents.local", None, NOW + 3600).unwrap_err();
    assert!(expired.to_string().contains("expired"), "{expired}");
    let other = check(&token, &key, "other.local", None, NOW).unwrap_err();
    assert!(other.to_string().contains("trust domain"), "{other}");
    let issuer = check(&token, &key, "agents.local", Some("https://x"), NOW).unwrap_err();
    assert!(issuer.to_string().contains("issuer"), "{issuer}");
}

#[tokio::test]
async fn an_error_never_contains_the_token() {
    let signer = MemorySigner::generate("k1");
    let token = signed(&signer, &claims()).await;
    let error = check(&token, &signer.jwk(), "other.local", None, NOW).unwrap_err();
    assert!(!error.to_string().contains(&token));
}
