//! The identity service over HTTP (ADR-0111): `/api/identity/token`, `jwks.json`, `config`,
//! `/api/identity/grants`.

use std::sync::Arc;
use std::time::Duration;

use grauwolf_identity::testing::FakeEntra;
use grauwolf_identity::{
    EntraVerifier, GRANT_TYPE, GrantStore, IdentityPolicy, IdentityService, MemoryGrantStore,
    MemorySigner, ServiceSettings, TokenSigner, check,
};
use grauwolf_server::{CallerMap, SpiffeGate, identity_router};
use grauwolf_spiffe::testing::{FakeIdentity, now_secs, test_token};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ISSUER: &str = "https://identity.agents.local";
const SELF: &str = "spiffe://agents.local/ns/identity/sa/identity";
const COORDINATOR: &str = "spiffe://agents.local/ns/agents/sa/coordinator";
const OTHER: &str = "spiffe://agents.local/ns/agents/sa/other";
const MORNING: &str = "spiffe://agents.local/ns/schedules/sa/morning";
const ENTRA: &str = "https://entra.example/11111111-1111-1111-1111-111111111111/v2.0";

struct Fixture {
    url: String,
    entra: FakeEntra,
    signer: Arc<MemorySigner>,
    _keys: MockServer,
}

async fn start() -> Fixture {
    let entra = FakeEntra::new(ENTRA);
    let keys = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/keys"))
        .respond_with(ResponseTemplate::new(200).set_body_json(entra.jwks()))
        .mount(&keys)
        .await;
    let signer = Arc::new(MemorySigner::generate("k1"));
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
        IdentityPolicy::from_json(
            &json!({ "callers": { COORDINATOR: {} },
                "grants": { "agents": { COORDINATOR: {} }, "triggers": [MORNING] } })
            .to_string(),
        )
        .unwrap(),
    )
    .with_grants(
        Arc::new(MemoryGrantStore::new()) as Arc<dyn GrantStore>,
        entra.redeemer(),
    );
    let gate = Arc::new(SpiffeGate::new(
        Arc::new(FakeIdentity::new(SELF)),
        CallerMap::empty(),
        SELF.into(),
        false,
    ));
    let router = identity_router(Arc::new(service), Some(gate));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Fixture {
        url,
        entra,
        signer,
        _keys: keys,
    }
}

fn svid(id: &str) -> String {
    test_token(id, SELF, now_secs(), 300)
}

async fn post(fixture: &Fixture, bearer: Option<&str>, body: Value) -> (u16, String) {
    let mut request = reqwest::Client::new()
        .post(format!("{}/api/identity/token", fixture.url))
        .json(&body);
    if let Some(bearer) = bearer {
        request = request.bearer_auth(bearer);
    }
    let response = request.send().await.unwrap();
    (response.status().as_u16(), response.text().await.unwrap())
}

#[tokio::test]
async fn the_entry_exchanges_a_persons_entra_token_for_a_txn_token() {
    let fixture = start().await;
    let alice = fixture.entra.token("oid-alice", "Alice");
    let (status, body) = post(
        &fixture,
        Some(&svid(COORDINATOR)),
        json!({ "subject_token": alice }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(!body.contains(&alice), "the Entra token is not echoed");
    let answer: Value = serde_json::from_str(&body).unwrap();
    let claims = check(
        answer["access_token"].as_str().unwrap(),
        &fixture.signer.jwk(),
        "agents.local",
        Some(ISSUER),
        now_secs(),
    )
    .unwrap();
    assert_eq!(claims.sub, "oid-alice");
    assert_eq!(claims.req_wl, COORDINATOR);

    // Renewal over HTTP, by another workload of the trust domain.
    let (status, body) = post(
        &fixture,
        Some(&svid(OTHER)),
        json!({ "subject_token": answer["access_token"] }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn refusals_have_their_status_and_never_echo_a_token() {
    let fixture = start().await;
    let alice = fixture.entra.token("oid-alice", "Alice");
    // No workload identity.
    let (status, body) = post(&fixture, None, json!({ "subject_token": alice })).await;
    assert_eq!(status, 401, "{body}");
    // A caller outside the policy.
    let (status, body) = post(
        &fixture,
        Some(&svid(OTHER)),
        json!({ "subject_token": alice }),
    )
    .await;
    assert_eq!(status, 403, "{body}");
    assert!(!body.contains(&alice));
    // Another tenant.
    let mut claims = fixture.entra.claims("oid-alice", "Alice");
    claims["tid"] = json!("22222222-2222-2222-2222-222222222222");
    let foreign = fixture.entra.sign(&claims);
    let (status, body) = post(
        &fixture,
        Some(&svid(COORDINATOR)),
        json!({ "subject_token": foreign }),
    )
    .await;
    assert_eq!(status, 401, "{body}");
    assert!(!body.contains(&foreign));
    // Another audience.
    let mut claims = fixture.entra.claims("oid-alice", "Alice");
    claims["aud"] = json!("api://other");
    let (status, _) = post(
        &fixture,
        Some(&svid(COORDINATOR)),
        json!({ "subject_token": fixture.entra.sign(&claims) }),
    )
    .await;
    assert_eq!(status, 401);
    // A body that is not a token request.
    let (status, body) = post(
        &fixture,
        Some(&svid(COORDINATOR)),
        json!({ "token": alice }),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert!(!body.contains(&alice));
}

#[tokio::test]
async fn the_keys_and_the_configuration_are_public() {
    let fixture = start().await;
    let jwks: Value = reqwest::get(format!("{}/api/identity/jwks.json", fixture.url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(jwks["keys"][0]["kid"], "k1");
    assert_eq!(jwks["keys"][0]["alg"], "ES256");
    let config: Value = reqwest::get(format!("{}/api/identity/config", fixture.url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(config["person_issuer"], ENTRA);
    assert_eq!(config["trust_domain"], "agents.local");
}

#[tokio::test]
async fn people_create_list_and_revoke_grants_and_a_grant_run_enters_by_its_id() {
    let fixture = start().await;
    let http = reqwest::Client::new();
    let grants = format!("{}/api/identity/grants", fixture.url);
    let alice = fixture.entra.token("oid-alice", "Alice");
    let refresh = fixture.entra.refresh_token("oid-alice", "Alice");
    let body = json!({ "agent": COORDINATOR, "trigger": MORNING, "refresh_token": refresh });

    // No person: 401; a malformed body is not echoed.
    let response = http.post(&grants).json(&body).send().await.unwrap();
    assert_eq!(response.status(), 401);
    let response = http
        .post(&grants)
        .bearer_auth(&alice)
        .body(format!("{{\"refresh_token\": \"{refresh}\""))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert!(!response.text().await.unwrap().contains(&refresh));

    let response = http
        .post(&grants)
        .bearer_auth(&alice)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let grant: Value = response.json().await.unwrap();
    let id = grant["id"].as_str().unwrap().to_owned();
    assert!(id.starts_with("grt_"));
    assert!(grant.get("refresh_token").is_none());
    assert_eq!(grant["owner"]["oid"], "oid-alice");

    let listed: Value = http
        .get(&grants)
        .bearer_auth(&alice)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["grants"][0]["id"], id.as_str());
    assert!(!listed.to_string().contains("rt-"));

    // The coordinator enters with the grant, naming its caller as the trigger.
    let (status, text) = post(
        &fixture,
        Some(&svid(COORDINATOR)),
        json!({ "subject_token": id, "subject_token_type": GRANT_TYPE, "trigger": MORNING }),
    )
    .await;
    assert_eq!(status, 200, "{text}");
    let token: Value = serde_json::from_str(&text).unwrap();
    let claims = check(
        token["access_token"].as_str().unwrap(),
        &fixture.signer.public_keys().await.unwrap()[0],
        "agents.local",
        Some(ISSUER),
        now_secs(),
    )
    .unwrap();
    assert_eq!(claims.sub, "oid-alice");
    assert_eq!(claims.azd, Some(json!({ "grant": id })));

    // Bob cannot revoke Alice's grant; Alice can, once.
    let bob = fixture.entra.token("oid-bob", "Bob");
    let revoke = format!("{grants}/{id}");
    let status = |response: reqwest::Response| response.status().as_u16();
    assert_eq!(
        status(http.delete(&revoke).bearer_auth(&bob).send().await.unwrap()),
        404
    );
    assert_eq!(
        status(
            http.delete(&revoke)
                .bearer_auth(&alice)
                .send()
                .await
                .unwrap()
        ),
        204
    );
    assert_eq!(
        status(
            http.delete(&revoke)
                .bearer_auth(&alice)
                .send()
                .await
                .unwrap()
        ),
        404
    );
    let (status, _) = post(
        &fixture,
        Some(&svid(COORDINATOR)),
        json!({ "subject_token": id, "subject_token_type": GRANT_TYPE, "trigger": MORNING }),
    )
    .await;
    assert_eq!(status, 401);
}
