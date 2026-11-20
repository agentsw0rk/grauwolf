//! Entra access tokens: RS256 from the tenant's JWKS, tenant, issuer, audience, expiry, scope.

use grauwolf_identity::testing::{AUDIENCE, FakeEntra};
use grauwolf_identity::{EntraVerifier, IdentityError, PersonTokenVerifier};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ISSUER: &str = "https://entra.example/11111111-1111-1111-1111-111111111111/v2.0";

async fn serve_keys(server: &MockServer, entra: &FakeEntra) {
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/keys"))
        .respond_with(ResponseTemplate::new(200).set_body_json(entra.jwks()))
        .mount(server)
        .await;
}

async fn verifier(entra: &FakeEntra) -> (MockServer, EntraVerifier) {
    let server = MockServer::start().await;
    serve_keys(&server, entra).await;
    let verifier = EntraVerifier::new(
        reqwest::Client::new(),
        entra.settings(&format!("{}/keys", server.uri())),
    );
    (server, verifier)
}

fn unauthorized(result: Result<grauwolf_identity::Person, IdentityError>, reason: &str) {
    match result {
        Err(IdentityError::Unauthorized(text)) => assert!(text.contains(reason), "{text}"),
        other => panic!("expected 401 ({reason}), got {other:?}"),
    }
}

#[tokio::test]
async fn a_valid_token_names_the_person() {
    let entra = FakeEntra::new(ISSUER);
    let (_server, verifier) = verifier(&entra).await;
    let person = verifier
        .verify(&entra.token("oid-alice", "Alice"))
        .await
        .unwrap();
    assert_eq!(person.oid, "oid-alice");
    assert_eq!(person.name.as_deref(), Some("Alice"));
    assert_eq!(person.upn.as_deref(), Some("alice@example.org"));
}

#[tokio::test]
async fn wrong_tenant_audience_issuer_scope_and_expiry_are_refused() {
    let entra = FakeEntra::new(ISSUER);
    let (_server, verifier) = verifier(&entra).await;
    let with = |field: &str, value: serde_json::Value| {
        let mut claims = entra.claims("oid-alice", "Alice");
        claims[field] = value;
        entra.sign(&claims)
    };
    unauthorized(
        verifier.verify(&with("tid", json!("22222222-2222"))).await,
        "tenant",
    );
    unauthorized(
        verifier.verify(&with("aud", json!("api://other"))).await,
        "audience",
    );
    unauthorized(
        verifier
            .verify(&with("iss", json!("https://evil/v2.0")))
            .await,
        "issuer",
    );
    unauthorized(
        verifier.verify(&with("scp", json!("User.Read"))).await,
        "scope",
    );
    unauthorized(verifier.verify(&with("exp", json!(1_000))).await, "expired");
    unauthorized(
        verifier.verify(&with("nbf", json!(u32::MAX))).await,
        "not valid yet",
    );
    // An audience list that contains the API is fine.
    let listed = with("aud", json!(["api://other", AUDIENCE]));
    assert!(verifier.verify(&listed).await.is_ok());
}

#[tokio::test]
async fn a_manipulated_or_unsigned_token_is_refused() {
    let entra = FakeEntra::new(ISSUER);
    let (_server, verifier) = verifier(&entra).await;
    let token = entra.token("oid-alice", "Alice");
    let mallory = entra.token("oid-mallory", "Mallory");
    let (a, b): (Vec<&str>, Vec<&str>) = (token.split('.').collect(), mallory.split('.').collect());
    unauthorized(
        verifier
            .verify(&format!("{}.{}.{}", a[0], b[1], a[2]))
            .await,
        "signature",
    );
    let none = format!("eyJhbGciOiJub25lIn0.{}.", a[1]);
    unauthorized(verifier.verify(&none).await, "");
}

#[tokio::test]
async fn an_unknown_key_fetches_the_keys_again() {
    let mut entra = FakeEntra::new(ISSUER);
    let (server, verifier) = verifier(&entra).await;
    verifier
        .verify(&entra.token("oid-alice", "Alice"))
        .await
        .unwrap();
    entra.rotate("k2");
    serve_keys(&server, &entra).await;
    let person = verifier
        .verify(&entra.token("oid-bob", "Bob"))
        .await
        .unwrap();
    assert_eq!(person.oid, "oid-bob");
}

#[tokio::test]
async fn keys_come_from_discovery_and_an_unreachable_issuer_is_unavailable() {
    let entra = FakeEntra::new(ISSUER);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "jwks_uri": format!("{}/keys", server.uri()) })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/keys"))
        .respond_with(ResponseTemplate::new(200).set_body_json(entra.jwks()))
        .mount(&server)
        .await;
    let mut settings = entra.settings("unused");
    settings.jwks_url = None;
    settings.issuer = server.uri();
    let verifier = EntraVerifier::new(reqwest::Client::new(), settings);
    let mut claims = entra.claims("oid-alice", "Alice");
    claims["iss"] = json!(server.uri());
    assert!(verifier.verify(&entra.sign(&claims)).await.is_ok());

    let mut settings = entra.settings("http://127.0.0.1:9/keys");
    settings.jwks_url = Some("http://127.0.0.1:9/keys".into());
    let down = EntraVerifier::new(reqwest::Client::new(), settings);
    let error = down
        .verify(&entra.token("oid-alice", "Alice"))
        .await
        .unwrap_err();
    assert!(matches!(error, IdentityError::Unavailable(_)), "{error}");
}

#[tokio::test]
async fn a_token_is_accepted_for_any_of_several_audiences() {
    const API_ID: &str = "22222222-2222-2222-2222-222222222222";
    let entra = FakeEntra::new(ISSUER);
    let server = MockServer::start().await;
    serve_keys(&server, &entra).await;
    let mut settings = entra.settings(&format!("{}/keys", server.uri()));
    settings.audiences = vec![API_ID.to_owned(), AUDIENCE.to_owned()];
    let verifier = EntraVerifier::new(reqwest::Client::new(), settings);
    let with = |aud: serde_json::Value| {
        let mut claims = entra.claims("oid-alice", "Alice");
        claims["aud"] = aud;
        entra.sign(&claims)
    };
    // v2.0: the application ID; v1.0 and Entra Local's generator: the application ID URI.
    assert!(verifier.verify(&with(json!(API_ID))).await.is_ok());
    assert!(verifier.verify(&with(json!(AUDIENCE))).await.is_ok());
    assert!(
        verifier
            .verify(&with(json!(["api://other", API_ID])))
            .await
            .is_ok()
    );
    unauthorized(
        verifier.verify(&with(json!("api://other"))).await,
        "audience",
    );
}
