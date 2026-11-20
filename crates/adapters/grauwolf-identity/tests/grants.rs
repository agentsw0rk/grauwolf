//! Grants in the identity service (ADR-0111 stage 3): creation with the person's refresh token,
//! listing and revoking own grants, and the entry of a grant run with its checks.

use std::sync::Arc;
use std::time::Duration;

use grauwolf_identity::testing::FakeEntra;
use grauwolf_identity::{
    CreateGrant, EntraVerifier, GRANT_TYPE, Grant, GrantStore, IdentityError, IdentityPolicy,
    IdentityService, MemoryGrantStore, MemorySigner, ServiceSettings, TXN_TOKEN_TYPE, TokenRequest,
    TokenSigner, TxnClaims, check,
};
use grauwolf_spiffe::SpiffeId;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ISSUER: &str = "https://identity.agents.local";
const ENTRA: &str = "https://entra.example/11111111-1111-1111-1111-111111111111/v2.0";
const COORDINATOR: &str = "spiffe://agents.local/ns/coordinator/sa/coordinator";
const REVIEWER: &str = "spiffe://agents.local/ns/reviewer/sa/reviewer";
const MORNING: &str = "spiffe://agents.local/ns/schedules/sa/morning";
const EVENING: &str = "spiffe://agents.local/ns/schedules/sa/evening";
const OTHER: &str = "spiffe://agents.local/ns/ci/sa/runner";

struct Fixture {
    service: IdentityService,
    entra: FakeEntra,
    store: Arc<MemoryGrantStore>,
    signer: Arc<MemorySigner>,
    _keys: MockServer,
}

fn policy() -> IdentityPolicy {
    IdentityPolicy::from_json(
        &json!({ "grants": {
            "agents": { COORDINATOR: { "purposes": ["review", "report"] }, REVIEWER: {} },
            "triggers": ["spiffe://agents.local/ns/schedules/*"]
        } })
        .to_string(),
    )
    .unwrap()
}

async fn fixture() -> Fixture {
    let entra = FakeEntra::new(ENTRA);
    let keys = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/keys"))
        .respond_with(ResponseTemplate::new(200).set_body_json(entra.jwks()))
        .mount(&keys)
        .await;
    let signer = Arc::new(MemorySigner::generate("k1"));
    let store = Arc::new(MemoryGrantStore::new());
    let service = IdentityService::new(
        ServiceSettings {
            issuer: ISSUER.into(),
            trust_domain: "agents.local".into(),
            person_issuer: ENTRA.into(),
            lifetime: Duration::from_secs(900),
            max_chain_lifetime: Duration::from_secs(8 * 3600),
        },
        Arc::clone(&signer) as Arc<dyn TokenSigner>,
        Arc::new(EntraVerifier::new(
            reqwest::Client::new(),
            entra.settings(&format!("{}/keys", keys.uri())),
        )),
        policy(),
    )
    .with_grants(Arc::clone(&store) as Arc<dyn GrantStore>, entra.redeemer());
    Fixture {
        service,
        entra,
        store,
        signer,
        _keys: keys,
    }
}

fn create(refresh_token: &str, purposes: &[&str]) -> CreateGrant {
    serde_json::from_value(json!({
        "agent": COORDINATOR, "trigger": MORNING, "purposes": purposes,
        "refresh_token": refresh_token,
    }))
    .unwrap()
}

fn id(text: &str) -> SpiffeId {
    SpiffeId::parse(text).unwrap()
}

fn grant_request(grant: &str, trigger: &str) -> TokenRequest {
    serde_json::from_value(json!({
        "subject_token": grant, "subject_token_type": GRANT_TYPE, "trigger": trigger,
    }))
    .unwrap()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

async fn claims_of(signer: &MemorySigner, token: &str) -> TxnClaims {
    check(
        token,
        &signer.public_keys().await.unwrap()[0],
        "agents.local",
        Some(ISSUER),
        now(),
    )
    .unwrap()
}

/// Alice's grant for the coordinator, started by `morning`.
async fn alices_grant(fixture: &Fixture) -> Grant {
    let alice = fixture.entra.token("oid-alice", "Alice");
    let refresh = fixture.entra.refresh_token("oid-alice", "Alice");
    fixture
        .service
        .create_grant(&alice, create(&refresh, &["review"]))
        .await
        .unwrap()
}

#[tokio::test]
async fn a_person_creates_lists_and_revokes_own_grants() {
    let fixture = fixture().await;
    let refresh = fixture.entra.refresh_token("oid-alice", "Alice");
    let alice = fixture.entra.token("oid-alice", "Alice");
    let grant = fixture
        .service
        .create_grant(&alice, create(&refresh, &["review"]))
        .await
        .unwrap();
    assert_eq!(grant.owner.oid, "oid-alice");
    assert_eq!(grant.owner.name.as_deref(), Some("Alice"));
    assert_eq!(
        (grant.agent.as_str(), grant.trigger.as_str()),
        (COORDINATOR, MORNING)
    );
    assert_eq!(grant.max_run, 4 * 3600);
    // Redeemed once: the stored token is the rotated one, the handed-over one is used up.
    let stored = fixture.store.token(&grant.id).await.unwrap().unwrap();
    assert_ne!(stored, refresh);
    assert!(fixture.entra.is_valid_refresh_token(&stored));
    assert!(!fixture.entra.is_valid_refresh_token(&refresh));

    let bob = fixture.entra.token("oid-bob", "Bob");
    assert_eq!(
        fixture.service.own_grants(&alice).await.unwrap(),
        std::slice::from_ref(&grant)
    );
    assert_eq!(fixture.service.own_grants(&bob).await.unwrap(), []);
    assert!(matches!(
        fixture.service.revoke_grant(&bob, &grant.id).await,
        Err(IdentityError::NotFound(_))
    ));
    fixture
        .service
        .revoke_grant(&alice, &grant.id)
        .await
        .unwrap();
    assert_eq!(fixture.store.get(&grant.id).await.unwrap(), None);
    assert_eq!(fixture.store.token(&grant.id).await.unwrap(), None);
    assert!(matches!(
        fixture.service.revoke_grant(&alice, &grant.id).await,
        Err(IdentityError::NotFound(_))
    ));
}

#[tokio::test]
async fn creation_is_refused_outside_the_policy_or_with_another_persons_refresh_token() {
    let fixture = fixture().await;
    let alice = fixture.entra.token("oid-alice", "Alice");
    let refresh = || fixture.entra.refresh_token("oid-alice", "Alice");
    let refused = |result: Result<Grant, IdentityError>| result.unwrap_err();
    // A purpose the policy does not allow the coordinator.
    let error = refused(
        fixture
            .service
            .create_grant(&alice, create(&refresh(), &["deploy"]))
            .await,
    );
    assert!(matches!(error, IdentityError::Forbidden(_)), "{error:?}");
    // A trigger outside grants.triggers.
    let mut request = create(&refresh(), &[]);
    request.grant.trigger = OTHER.into();
    let error = refused(fixture.service.create_grant(&alice, request).await);
    assert!(matches!(error, IdentityError::Forbidden(_)), "{error:?}");
    // Bob's refresh token in Alice's request.
    let bobs = fixture.entra.refresh_token("oid-bob", "Bob");
    let error = refused(
        fixture
            .service
            .create_grant(&alice, create(&bobs, &[]))
            .await,
    );
    assert!(matches!(error, IdentityError::Forbidden(_)), "{error:?}");
    // A refresh token Entra refuses, and no person at all.
    let error = refused(
        fixture
            .service
            .create_grant(&alice, create("rt-x", &[]))
            .await,
    );
    assert!(matches!(error, IdentityError::BadRequest(_)), "{error:?}");
    let error = refused(
        fixture
            .service
            .create_grant("not-a-token", create(&refresh(), &[]))
            .await,
    );
    assert!(matches!(error, IdentityError::Unauthorized(_)), "{error:?}");
    assert_eq!(fixture.store.list().await.unwrap(), []);
}

#[tokio::test]
async fn a_grant_run_gets_a_txn_token_for_the_owner_with_the_grant_and_its_purposes() {
    let fixture = fixture().await;
    let grant = alices_grant(&fixture).await;
    let before = fixture.store.token(&grant.id).await.unwrap().unwrap();
    let answer = fixture
        .service
        .token(&id(COORDINATOR), grant_request(&grant.id, MORNING))
        .await
        .unwrap();
    let claims = claims_of(&fixture.signer, &answer.access_token).await;
    assert_eq!(claims.sub, "oid-alice");
    assert_eq!(claims.name.as_deref(), Some("Alice"));
    assert_eq!(claims.purp.as_deref(), Some("review"));
    assert_eq!(claims.azd, Some(json!({ "grant": grant.id })));
    assert_eq!(claims.req_wl, COORDINATOR);
    assert!(claims.chain_exp.abs_diff(now() + 4 * 3600) <= 2);
    // The rotated token is stored, `last_used` is set.
    let after = fixture.store.token(&grant.id).await.unwrap().unwrap();
    assert_ne!(after, before);
    assert!(fixture.entra.is_valid_refresh_token(&after));
    let used = fixture.store.get(&grant.id).await.unwrap().unwrap();
    assert!(used.last_used.is_some_and(|at| at.abs_diff(now()) <= 2));
    // A renewal keeps the grant, the purposes and `chain_exp`.
    let renewed: TokenRequest = serde_json::from_value(json!({
        "subject_token": answer.access_token, "subject_token_type": TXN_TOKEN_TYPE,
    }))
    .unwrap();
    let renewed = fixture.service.token(&id(REVIEWER), renewed).await.unwrap();
    let renewed = claims_of(&fixture.signer, &renewed.access_token).await;
    assert_eq!(
        (renewed.txn, renewed.azd, renewed.purp, renewed.chain_exp),
        (claims.txn, claims.azd, claims.purp, claims.chain_exp)
    );
}

#[tokio::test]
async fn a_grant_run_is_refused_for_the_wrong_trigger_or_agent_and_after_expiry_revocation_or_disabling()
 {
    let fixture = fixture().await;
    let grant = alices_grant(&fixture).await;
    let run = |agent: &'static str, trigger: &'static str, grant: String| {
        let service = &fixture.service;
        async move {
            service
                .token(&id(agent), grant_request(&grant, trigger))
                .await
                .unwrap_err()
        }
    };
    let status = |error: IdentityError| error.status();
    assert_eq!(
        status(run(COORDINATOR, EVENING, grant.id.clone()).await),
        403
    );
    assert_eq!(status(run(REVIEWER, MORNING, grant.id.clone()).await), 403);
    assert_eq!(
        status(
            run(
                COORDINATOR,
                MORNING,
                "grt_0123456789abcdef0123456789abcdef".into()
            )
            .await
        ),
        401
    );
    assert_eq!(status(run(COORDINATOR, MORNING, "grt_x".into()).await), 401);

    // Expired.
    let mut expired = grant.clone();
    expired.valid_until = now() - 1;
    fixture.store.save(&expired).await.unwrap();
    assert_eq!(
        status(run(COORDINATOR, MORNING, grant.id.clone()).await),
        401
    );
    fixture.store.save(&grant).await.unwrap();

    // The policy no longer allows a purpose of the grant.
    let mut widened = grant.clone();
    widened.purposes = vec!["deploy".into()];
    fixture.store.save(&widened).await.unwrap();
    assert_eq!(
        status(run(COORDINATOR, MORNING, grant.id.clone()).await),
        403
    );
    fixture.store.save(&grant).await.unwrap();

    // Alice is disabled in Entra.
    fixture.entra.disable("oid-alice");
    let error = run(COORDINATOR, MORNING, grant.id.clone()).await;
    assert_eq!(error.status(), 401, "{error:?}");
    assert!(!error.to_string().contains("rt-"), "{error}");

    // Revoked.
    let other = {
        let bob = fixture.entra.token("oid-bob", "Bob");
        let refresh = fixture.entra.refresh_token("oid-bob", "Bob");
        let grant = fixture
            .service
            .create_grant(&bob, create(&refresh, &[]))
            .await
            .unwrap();
        fixture.service.revoke_grant(&bob, &grant.id).await.unwrap();
        grant
    };
    assert_eq!(status(run(COORDINATOR, MORNING, other.id).await), 401);
}

#[tokio::test]
async fn without_grants_configured_grant_calls_are_unavailable() {
    let fixture = fixture().await;
    let plain = IdentityService::new(
        fixture.service.settings().clone(),
        Arc::clone(&fixture.signer) as Arc<dyn TokenSigner>,
        Arc::new(grauwolf_identity::testing::TrustingPeople),
        policy(),
    );
    let error = plain
        .token(
            &id(COORDINATOR),
            grant_request("grt_0123456789abcdef0123456789abcdef", MORNING),
        )
        .await
        .unwrap_err();
    assert_eq!(error.status(), 503);
}
