//! Delegation in an agent (ADR-0111): the entry exchanges a person's Entra token for a Txn-Token,
//! a hop verifies the Txn-Token of a calling agent, and the person becomes the run's user; end
//! to end, a coordinator's A2A tool carries the Txn-Token to the reviewer. A trigger with a
//! SPIFFE identity starts a run on a person's grant (`Grauwolf-Grant`, stage 3).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use grauwolf_agents::events::{AgentEvent, Feature, OnBehalfOf};
use grauwolf_agents::memory::MemoryChatHistoryStore;
use grauwolf_identity::testing::FakeEntra;
use grauwolf_identity::{
    CreateGrant, EntraVerifier, Grant, GrantStore, IdentityClient, IdentityPolicy, IdentityService,
    MemoryGrantStore, MemorySigner, ServiceSettings, TokenSigner, TxnClaims, issue,
};
use grauwolf_interpreter::{Interpreter, RunExtras};
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::ToolCall;
use grauwolf_server::{CallerMap, ServedAgent, ServiceOptions, SpiffeGate, Users};
use grauwolf_spec::{AgentSpec, AgentSpecExt};
use grauwolf_spiffe::testing::{FakeIdentity, now_secs, test_token};
use grauwolf_spiffe::{AuthMode, ClientAuth};
use grauwolf_test::MockLlmClient;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const DOMAIN: &str = "agentswork.de";
const ISSUER: &str = "https://identity.agentswork.de";
const IDENTITY: &str = "spiffe://agentswork.de/ns/identity/sa/identity";
const REVIEWER: &str = "spiffe://agentswork.de/ns/reviewer/sa/reviewer";
const COORDINATOR: &str = "spiffe://agentswork.de/ns/coordinator/sa/coordinator";
const MORNING: &str = "spiffe://agentswork.de/ns/schedules/sa/morning";
const EVENING: &str = "spiffe://agentswork.de/ns/schedules/sa/evening";
const INTRUDER: &str = "spiffe://agentswork.de/ns/intruder/sa/intruder";
const ENTRA: &str = "https://entra.example/11111111-1111-1111-1111-111111111111/v2.0";
const SHARED: &str = "shared-secret";

/// The identity service on a plain port, people's tokens from `entra`.
struct IdentityFixture {
    url: String,
    entra: FakeEntra,
    signer: Arc<MemorySigner>,
    service: Arc<IdentityService>,
    store: Arc<MemoryGrantStore>,
    _keys: MockServer,
}

async fn identity_service() -> IdentityFixture {
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
            trust_domain: DOMAIN.into(),
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
            &json!({ "callers": { REVIEWER: {}, COORDINATOR: {} },
                "grants": { "agents": { COORDINATOR: { "purposes": ["review"] } },
                    "triggers": ["spiffe://agentswork.de/ns/schedules/*"] } })
            .to_string(),
        )
        .unwrap(),
    );
    let store = Arc::new(MemoryGrantStore::new());
    let service =
        Arc::new(service.with_grants(Arc::clone(&store) as Arc<dyn GrantStore>, entra.redeemer()));
    let gate = Arc::new(SpiffeGate::new(
        Arc::new(FakeIdentity::new(IDENTITY)),
        CallerMap::empty(),
        IDENTITY.into(),
        false,
    ));
    let url = serve(grauwolf_server::identity_router(
        Arc::clone(&service),
        Some(gate),
    ))
    .await;
    IdentityFixture {
        url,
        entra,
        signer,
        service,
        store,
        _keys: keys,
    }
}

async fn serve(router: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    url
}

/// The client of the identity service for the workload `id` (JWT-SVIDs over http).
fn client(url: &str, id: &str) -> Arc<IdentityClient> {
    let auth = ClientAuth::new(
        Arc::new(FakeIdentity::new(id)),
        AuthMode::Jwt,
        None,
        Some(IDENTITY.into()),
    )
    .unwrap();
    Arc::new(IdentityClient::new(url, DOMAIN, Some(Arc::new(auth))).unwrap())
}

/// What a run saw: its user, the person it acts for, the Txn-Token its outgoing calls carry.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    user: Option<String>,
    on_behalf_of: Option<OnBehalfOf>,
    txn_token: Option<String>,
}

/// An agent that records what each run saw; the input `"approve"` asks for an approval first.
#[derive(Default)]
struct Recorder(Mutex<Vec<Seen>>);

impl ServedAgent for Recorder {
    fn input_schema(&self) -> Option<Value> {
        None
    }

    fn run(
        &self,
        input: Value,
        extras: RunExtras,
        user: Option<String>,
    ) -> BoxFuture<'_, Result<Value, String>> {
        Box::pin(async move {
            let txn_token = grauwolf_tools::delegation_header()
                .await
                .unwrap()
                .map(|(_, token)| token);
            self.0.lock().unwrap().push(Seen {
                user,
                on_behalf_of: extras.on_behalf_of.clone(),
                txn_token,
            });
            if input == "approve" {
                let approver = extras.approver.clone().ok_or("no approver")?;
                let call = ToolCall {
                    id: "c1".into(),
                    tool: "send_mail".into(),
                    args: json!({}),
                };
                return Ok(json!(format!("{:?}", approver(&call).await)));
            }
            Ok(json!("done"))
        })
    }
}

/// The reviewer: `router` behind the SPIFFE gate (coordinator mapped), with `delegation` as its
/// identity service.
async fn reviewer(router: axum::Router, delegation: Option<Arc<IdentityClient>>) -> String {
    serve(gated(router, delegation)).await
}

/// `router` behind the reviewer's gate.
fn gated(router: axum::Router, delegation: Option<Arc<IdentityClient>>) -> axum::Router {
    let mut gate = SpiffeGate::new(
        Arc::new(FakeIdentity::new(REVIEWER)),
        CallerMap::from_json(&json!({ COORDINATOR: "coordinator" }).to_string()).unwrap(),
        REVIEWER.into(),
        false,
    );
    if let Some(client) = delegation {
        gate = gate.delegating_to(client);
    }
    grauwolf_server::with_spiffe_auth(
        router,
        Some(SHARED.into()),
        Users::default(),
        Some(Arc::new(gate)),
    )
}

fn recorded(recorder: &Arc<Recorder>) -> axum::Router {
    grauwolf_server::router_for(
        Arc::clone(recorder) as Arc<dyn ServedAgent>,
        Arc::new(MemoryChatHistoryStore::default()),
        ServiceOptions::default(),
    )
}

/// A JWT-SVID of `id` for the reviewer.
fn svid(id: &str) -> String {
    test_token(id, REVIEWER, now_secs(), 300)
}

async fn run(url: &str, bearer: Option<&str>, txn_token: Option<&str>) -> (u16, String) {
    let mut request = reqwest::Client::new()
        .post(format!("{url}/run"))
        .json(&json!({ "input": "hi" }));
    if let Some(bearer) = bearer {
        request = request.bearer_auth(bearer);
    }
    if let Some(token) = txn_token {
        request = request.header("Txn-Token", token);
    }
    let response = request.send().await.unwrap();
    (response.status().as_u16(), response.text().await.unwrap())
}

/// The Txn-Token the coordinator holds for alice.
async fn alices_txn_token(identity: &IdentityFixture) -> String {
    let alice = identity.entra.token("oid-alice", "Alice");
    client(&identity.url, COORDINATOR)
        .exchange(&alice)
        .await
        .unwrap()
        .token()
        .to_owned()
}

fn claims(aud: &str, exp: u64) -> TxnClaims {
    TxnClaims {
        iss: ISSUER.into(),
        aud: aud.into(),
        sub: "oid-mallory".into(),
        name: None,
        upn: None,
        txn: "txn-forged".into(),
        purp: None,
        azd: None,
        req_wl: COORDINATOR.into(),
        iat: now_secs(),
        exp,
        chain_exp: now_secs() + 3600,
    }
}

#[tokio::test]
async fn the_entry_exchanges_a_persons_token_and_the_person_is_the_runs_user() {
    let identity = identity_service().await;
    let recorder = Arc::new(Recorder::default());
    let url = reviewer(recorded(&recorder), Some(client(&identity.url, REVIEWER))).await;
    let alice = identity.entra.token("oid-alice", "Alice");
    let (status, body) = run(&url, Some(&alice), None).await;
    assert_eq!(status, 200, "{body}");
    let seen = recorder.0.lock().unwrap()[0].clone();
    assert_eq!(seen.user.as_deref(), Some("oid-alice"));
    let person = seen.on_behalf_of.unwrap();
    assert_eq!(person.oid, "oid-alice");
    assert_eq!(person.name.as_deref(), Some("Alice"));
    let token = seen.txn_token.expect("the run holds a Txn-Token");
    let claims = client(&identity.url, COORDINATOR)
        .verify(&token)
        .await
        .unwrap();
    assert_eq!(claims.sub, "oid-alice");
    assert_eq!(
        claims.req_wl, REVIEWER,
        "the agent at the entry requested it"
    );
    assert_eq!(claims.txn, person.txn);
}

#[tokio::test]
async fn the_entry_answers_401_for_a_refused_token_and_503_without_the_identity_service() {
    let identity = identity_service().await;
    let recorder = Arc::new(Recorder::default());
    let url = reviewer(recorded(&recorder), Some(client(&identity.url, REVIEWER))).await;
    // Same issuer, another key: the identity service refuses it.
    let forged = FakeEntra::new(ENTRA).token("oid-alice", "Alice");
    let (status, body) = run(&url, Some(&forged), None).await;
    assert_eq!(status, 401, "{body}");
    assert!(!body.contains(&forged));

    let down = reviewer(
        recorded(&recorder),
        Some(client("http://127.0.0.1:9", REVIEWER)),
    )
    .await;
    let alice = identity.entra.token("oid-alice", "Alice");
    let (status, body) = run(&down, Some(&alice), None).await;
    assert_eq!(status, 503, "{body}");
    assert!(recorder.0.lock().unwrap().is_empty(), "no run started");
}

#[tokio::test]
async fn the_entry_passes_on_403_when_the_policy_of_the_identity_service_refuses_the_agent() {
    let identity = identity_service().await;
    let recorder = Arc::new(Recorder::default());
    // INTRUDER is not in the policy of the identity service.
    let url = reviewer(recorded(&recorder), Some(client(&identity.url, INTRUDER))).await;
    let alice = identity.entra.token("oid-alice", "Alice");
    let (status, body) = run(&url, Some(&alice), None).await;
    assert_eq!(status, 403, "{body}");
    assert!(body.contains("delegation refused"), "{body}");
    assert!(!body.contains(&alice));
    assert!(recorder.0.lock().unwrap().is_empty(), "no run started");
}

#[tokio::test]
async fn a_hop_verifies_the_txn_token_and_acts_for_the_person() {
    let identity = identity_service().await;
    let recorder = Arc::new(Recorder::default());
    let url = reviewer(recorded(&recorder), Some(client(&identity.url, REVIEWER))).await;
    let token = alices_txn_token(&identity).await;
    let (status, body) = run(&url, Some(&svid(COORDINATOR)), Some(&token)).await;
    assert_eq!(status, 200, "{body}");
    let seen = recorder.0.lock().unwrap()[0].clone();
    assert_eq!(seen.user.as_deref(), Some("oid-alice"));
    assert_eq!(seen.on_behalf_of.unwrap().oid, "oid-alice");
    assert_eq!(seen.txn_token.as_deref(), Some(token.as_str()), "passed on");

    // Without a Txn-Token the coordinator is the mapped user, acting for nobody.
    let (status, _) = run(&url, Some(&svid(COORDINATOR)), None).await;
    assert_eq!(status, 200);
    let seen = recorder.0.lock().unwrap()[1].clone();
    assert_eq!(seen.user.as_deref(), Some("coordinator"));
    assert_eq!((seen.on_behalf_of, seen.txn_token), (None, None));
}

#[tokio::test]
async fn manipulated_expired_foreign_and_wrong_audience_txn_tokens_get_401() {
    let identity = identity_service().await;
    let recorder = Arc::new(Recorder::default());
    let url = reviewer(recorded(&recorder), Some(client(&identity.url, REVIEWER))).await;
    let token = alices_txn_token(&identity).await;
    let (head, signature) = token.rsplit_once('.').unwrap();
    let flipped = if signature.starts_with('A') { 'B' } else { 'A' };
    let manipulated = format!("{head}.{flipped}{}", &signature[1..]);
    let signer = &*identity.signer;
    let expired = issue(signer, &claims(DOMAIN, now_secs() - 120))
        .await
        .unwrap();
    let wrong_aud = issue(signer, &claims("other.de", now_secs() + 600))
        .await
        .unwrap();
    let foreign = issue(
        &MemorySigner::generate("k1"),
        &claims(DOMAIN, now_secs() + 600),
    )
    .await
    .unwrap();
    for (what, bad) in [
        ("manipulated", manipulated),
        ("expired", expired),
        ("wrong aud", wrong_aud),
        ("foreign key", foreign),
        ("not a JWT", "garbage".to_owned()),
    ] {
        let (status, body) = run(&url, Some(&svid(COORDINATOR)), Some(&bad)).await;
        assert_eq!(status, 401, "{what}: {body}");
        assert!(!body.contains(&bad), "{what}: the token is not echoed");
    }
    assert!(recorder.0.lock().unwrap().is_empty(), "no run started");
}

#[tokio::test]
async fn a_txn_token_from_a_caller_without_a_mapped_spiffe_identity_gets_403() {
    let identity = identity_service().await;
    let recorder = Arc::new(Recorder::default());
    let url = reviewer(recorded(&recorder), Some(client(&identity.url, REVIEWER))).await;
    let token = alices_txn_token(&identity).await;
    let alice = identity.entra.token("oid-alice", "Alice");
    for (what, bearer) in [
        ("shared token", Some(SHARED.to_owned())),
        ("no credential", None),
        ("unmapped workload", Some(svid(INTRUDER))),
        ("a person's token", Some(alice)),
    ] {
        let (status, body) = run(&url, bearer.as_deref(), Some(&token)).await;
        assert_eq!(status, 403, "{what}: {body}");
    }
    assert!(recorder.0.lock().unwrap().is_empty(), "no run started");
}

#[tokio::test]
async fn an_agent_without_an_identity_service_refuses_txn_tokens_with_401() {
    let identity = identity_service().await;
    let recorder = Arc::new(Recorder::default());
    let url = reviewer(recorded(&recorder), None).await;
    let token = alices_txn_token(&identity).await;
    let (status, body) = run(&url, Some(&svid(COORDINATOR)), Some(&token)).await;
    assert_eq!(status, 401, "{body}");
    assert!(body.contains("GRAUWOLF_IDENTITY_URL"), "{body}");
}

/// Reads the SSE stream until the `approval` event and returns its ID.
async fn approval_id(response: &mut reqwest::Response) -> String {
    let mut text = String::new();
    while let Some(chunk) = response.chunk().await.unwrap() {
        text.push_str(&String::from_utf8_lossy(&chunk));
        if let Some(start) = text.find("event: approval\ndata: ") {
            let rest = &text[start + "event: approval\ndata: ".len()..];
            if let Some(end) = rest.find('\n') {
                let data: Value = serde_json::from_str(&rest[..end]).unwrap();
                return data["id"].as_str().unwrap().to_owned();
            }
        }
    }
    panic!("no approval event in {text}");
}

#[tokio::test]
async fn only_the_person_approves_a_delegated_run() {
    let identity = identity_service().await;
    let recorder = Arc::new(Recorder::default());
    let url = reviewer(recorded(&recorder), Some(client(&identity.url, REVIEWER))).await;
    let token = alices_txn_token(&identity).await;
    let http = reqwest::Client::new();
    let mut stream = http
        .post(format!("{url}/run/stream"))
        .bearer_auth(svid(COORDINATOR))
        .header("Txn-Token", &token)
        .json(&json!({ "input": "approve" }))
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), 200);
    let id = approval_id(&mut stream).await;
    let decide = |bearer: String| {
        http.post(format!("{url}/approvals/{id}"))
            .bearer_auth(bearer)
            .json(&json!({ "approved": true }))
            .send()
    };
    // The coordinator is not the person.
    assert_eq!(decide(svid(COORDINATOR)).await.unwrap().status(), 404);
    // Alice, signed in with her Entra token, decides.
    let alice = identity.entra.token("oid-alice", "Alice");
    assert_eq!(decide(alice).await.unwrap().status(), 204);
    let rest = stream.text().await.unwrap();
    assert!(rest.contains("Proceed"), "{rest}");
}

/// Remembers every event of the runs.
#[derive(Default)]
struct Events(Mutex<Vec<AgentEvent>>);

impl Feature for Events {
    fn on_event(&self, event: &AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

#[tokio::test]
async fn agent_starting_names_the_person_and_no_event_carries_the_token() {
    let identity = identity_service().await;
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "reviewer",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let events = Arc::new(Events::default());
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("ollama", Arc::new(MockLlmClient::new("Looks good.")))
            .build(),
    ))
    .with_feature(events.clone())
    .load(&spec)
    .unwrap();
    let url = reviewer(
        grauwolf_server::router(Arc::new(agent), Arc::new(MemoryChatHistoryStore::default())),
        Some(client(&identity.url, REVIEWER)),
    )
    .await;
    let token = alices_txn_token(&identity).await;
    let (status, body) = run(&url, Some(&svid(COORDINATOR)), Some(&token)).await;
    assert_eq!(status, 200, "{body}");
    let events = events.0.lock().unwrap();
    let Some(AgentEvent::AgentStarting {
        on_behalf_of: Some(person),
        caller,
        ..
    }) = events.first()
    else {
        panic!("{events:?}");
    };
    assert_eq!(person.oid, "oid-alice");
    assert_eq!(person.name.as_deref(), Some("Alice"));
    assert_eq!(caller.as_deref(), Some(COORDINATOR));
    let all = serde_json::to_string(&*events).unwrap();
    assert!(!all.contains(&token), "the Txn-Token is in no event");
}

/// The chain of the end-to-end tests: the coordinator (callers file `callers`) with an A2A tool
/// to the reviewer; returns the coordinator's URL, what the reviewer's runs saw and the
/// coordinator's events.
async fn chain(
    identity: &IdentityFixture,
    callers: CallerMap,
) -> (String, Arc<Recorder>, Arc<Events>) {
    // The reviewer: A2A behind its gate, recording what its runs see.
    let recorder = Arc::new(Recorder::default());
    let reviewer_spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "reviewer",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    // The agent card names the endpoint: bind first, then build the routes for that URL.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let reviewer_url = format!("http://{}", listener.local_addr().unwrap());
    let a2a = grauwolf_server::a2a::router_for(
        Arc::clone(&recorder) as Arc<dyn ServedAgent>,
        Arc::new(MemoryChatHistoryStore::default()),
        grauwolf_server::a2a::AgentCardInfo::from_spec(&reviewer_spec, &reviewer_url),
        ServiceOptions::default(),
    );
    let gated = gated(a2a, Some(client(&identity.url, REVIEWER)));
    tokio::spawn(async move { axum::serve(listener, gated).await.unwrap() });

    // The coordinator: an A2A tool to the reviewer with its workload identity (JWT-SVID).
    let coordinator_auth = ClientAuth::new(
        Arc::new(FakeIdentity::new(COORDINATOR)),
        AuthMode::Jwt,
        None,
        Some(REVIEWER.into()),
    )
    .unwrap();
    let a2a_client = Arc::new(
        grauwolf_a2a::A2aClient::connect_with_auth(
            &reviewer_url,
            &[],
            grauwolf_a2a::DEFAULT_READ_TIMEOUT,
            Some(Arc::new(coordinator_auth)),
        )
        .await
        .unwrap(),
    );
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "coordinator",
            "model": { "provider": "ollama", "id": "m" },
            "mcp_servers": { "agents": { "command": "agents", "tools": ["reviewer"] } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let mock = MockLlmClient::new("?")
        .answer_when_contains("done", "Reviewed.")
        .call_tool_when_contains(
            "Please review",
            "reviewer",
            json!({ "request": "Review this." }),
        );
    let events = Arc::new(Events::default());
    let coordinator = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("ollama", Arc::new(mock))
            .build(),
    ))
    .with_tools(vec![a2a_client.tool("reviewer", Some("Reviews"))])
    .with_feature(events.clone())
    .load(&spec)
    .unwrap();
    let gate = SpiffeGate::new(
        Arc::new(FakeIdentity::new(COORDINATOR)),
        callers,
        COORDINATOR.into(),
        false,
    )
    .delegating_to(client(&identity.url, COORDINATOR));
    let coordinator_url = serve(grauwolf_server::with_spiffe_auth(
        grauwolf_server::router(
            Arc::new(coordinator),
            Arc::new(MemoryChatHistoryStore::default()),
        ),
        None,
        Users::default(),
        Some(Arc::new(gate)),
    ))
    .await;
    (coordinator_url, recorder, events)
}

/// What the entry recorded in `agent_starting`.
fn person_at_entry(events: &Events) -> OnBehalfOf {
    match events.0.lock().unwrap().first() {
        Some(AgentEvent::AgentStarting {
            on_behalf_of: Some(person),
            ..
        }) => person.clone(),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_coordinator_passes_the_person_on_to_the_reviewer_in_one_chain() {
    let identity = identity_service().await;
    let (coordinator_url, recorder, events) = chain(&identity, CallerMap::empty()).await;

    // Alice signs in at the coordinator.
    let alice = identity.entra.token("oid-alice", "Alice");
    let response = reqwest::Client::new()
        .post(format!("{coordinator_url}/run"))
        .bearer_auth(&alice)
        .json(&json!({ "input": "Please review the change." }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["output"], "Reviewed.", "{body}");

    let at_entry = person_at_entry(&events);
    let seen = recorder.0.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].user.as_deref(), Some("oid-alice"));
    let at_hop = seen[0].on_behalf_of.clone().unwrap();
    assert_eq!(at_hop.oid, "oid-alice");
    assert_eq!(at_hop.txn, at_entry.txn, "one chain");
    let claims = client(&identity.url, REVIEWER)
        .verify(seen[0].txn_token.as_deref().unwrap())
        .await
        .unwrap();
    assert_eq!(claims.req_wl, COORDINATOR, "requested at the entry");
}

/// Alice's grant for the coordinator, started by `morning`, with the purpose `review`.
async fn alices_grant(identity: &IdentityFixture) -> Grant {
    let alice = identity.entra.token("oid-alice", "Alice");
    let request: CreateGrant = serde_json::from_value(json!({
        "agent": COORDINATOR, "trigger": MORNING, "purposes": ["review"],
        "refresh_token": identity.entra.refresh_token("oid-alice", "Alice"),
    }))
    .unwrap();
    identity
        .service
        .create_grant(&alice, request)
        .await
        .unwrap()
}

/// The schedules' workloads as callers of the coordinator.
fn schedules() -> CallerMap {
    CallerMap::from_json(&json!({ MORNING: "morning", EVENING: "evening" }).to_string()).unwrap()
}

/// `POST /run` with a grant, as the workload `trigger` (JWT-SVID for `audience`), or with
/// `bearer` instead.
async fn run_on_grant(
    url: &str,
    grant: &str,
    caller: Result<(&str, &str), &str>,
    input: &str,
) -> (u16, Value) {
    let bearer = match caller {
        Ok((trigger, audience)) => test_token(trigger, audience, now_secs(), 300),
        Err(bearer) => bearer.to_owned(),
    };
    let response = reqwest::Client::new()
        .post(format!("{url}/run"))
        .bearer_auth(bearer)
        .header("Grauwolf-Grant", grant)
        .json(&json!({ "input": input }))
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or_default())
}

#[tokio::test]
async fn a_trigger_starts_a_run_on_a_grant_and_the_reviewer_sees_the_person_in_the_same_chain() {
    let identity = identity_service().await;
    let grant = alices_grant(&identity).await;
    let (coordinator_url, recorder, events) = chain(&identity, schedules()).await;

    let (status, body) = run_on_grant(
        &coordinator_url,
        &grant.id,
        Ok((MORNING, COORDINATOR)),
        "Please review the change.",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["output"], "Reviewed.", "{body}");

    let at_entry = person_at_entry(&events);
    assert_eq!(at_entry.oid, "oid-alice");
    assert_eq!(at_entry.name.as_deref(), Some("Alice"));
    assert_eq!(at_entry.grant.as_deref(), Some(grant.id.as_str()));
    let seen = recorder.0.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].user.as_deref(), Some("oid-alice"));
    let at_hop = seen[0].on_behalf_of.clone().unwrap();
    assert_eq!(at_hop.txn, at_entry.txn, "one chain");
    assert_eq!(at_hop.grant.as_deref(), Some(grant.id.as_str()));
    let claims = client(&identity.url, REVIEWER)
        .verify(seen[0].txn_token.as_deref().unwrap())
        .await
        .unwrap();
    assert_eq!(claims.purp.as_deref(), Some("review"));
    assert_eq!(claims.req_wl, COORDINATOR);
    assert!(claims.chain_exp.abs_diff(now_secs() + 4 * 3600) <= 2);
    let used = identity.store.get(&grant.id).await.unwrap().unwrap();
    assert!(used.last_used.is_some());
}

#[tokio::test]
async fn grant_runs_are_refused_with_the_status_of_the_reason() {
    let identity = identity_service().await;
    let grant = alices_grant(&identity).await;
    let recorder = Arc::new(Recorder::default());
    let coordinator = {
        let gate = SpiffeGate::new(
            Arc::new(FakeIdentity::new(COORDINATOR)),
            schedules(),
            COORDINATOR.into(),
            false,
        )
        .delegating_to(client(&identity.url, COORDINATOR));
        serve(grauwolf_server::with_spiffe_auth(
            recorded(&recorder),
            Some(SHARED.into()),
            Users::default(),
            Some(Arc::new(gate)),
        ))
        .await
    };
    let id = grant.id.as_str();
    let morning = Ok((MORNING, COORDINATOR));
    let status = |(status, _): (u16, Value)| status;

    // The wrong trigger (mapped, but not the grant's): 403.
    assert_eq!(
        status(run_on_grant(&coordinator, id, Ok((EVENING, COORDINATOR)), "x").await),
        403
    );
    // A trigger that is not in the callers file: 403 before any exchange.
    assert_eq!(
        status(run_on_grant(&coordinator, id, Ok((INTRUDER, COORDINATOR)), "x").await),
        403
    );
    // The header from a caller without a SPIFFE identity: the shared token, a person's token.
    assert_eq!(
        status(run_on_grant(&coordinator, id, Err(SHARED), "x").await),
        403
    );
    let alice = identity.entra.token("oid-alice", "Alice");
    assert_eq!(
        status(run_on_grant(&coordinator, id, Err(&alice), "x").await),
        403
    );
    // Another agent than the grant's: the reviewer with morning as a caller.
    let reviewer = serve(grauwolf_server::with_spiffe_auth(
        recorded(&recorder),
        None,
        Users::default(),
        Some(Arc::new(
            SpiffeGate::new(
                Arc::new(FakeIdentity::new(REVIEWER)),
                schedules(),
                REVIEWER.into(),
                false,
            )
            .delegating_to(client(&identity.url, REVIEWER)),
        )),
    ))
    .await;
    assert_eq!(
        status(run_on_grant(&reviewer, id, Ok((MORNING, REVIEWER)), "x").await),
        403
    );
    // Unknown and expired grants: 401.
    let unknown = "grt_0123456789abcdef0123456789abcdef";
    assert_eq!(
        status(run_on_grant(&coordinator, unknown, morning, "x").await),
        401
    );
    let mut expired = grant.clone();
    expired.valid_until = now_secs() - 1;
    identity.store.save(&expired).await.unwrap();
    assert_eq!(
        status(run_on_grant(&coordinator, id, morning, "x").await),
        401
    );
    identity.store.save(&grant).await.unwrap();
    // A purpose the policy does not allow the coordinator: 403.
    let mut widened = grant.clone();
    widened.purposes = vec!["deploy".into()];
    identity.store.save(&widened).await.unwrap();
    assert_eq!(
        status(run_on_grant(&coordinator, id, morning, "x").await),
        403
    );
    identity.store.save(&grant).await.unwrap();
    // Still fine, then Alice is disabled: 401.
    assert_eq!(
        status(run_on_grant(&coordinator, id, morning, "x").await),
        200
    );
    identity.entra.disable("oid-alice");
    assert_eq!(
        status(run_on_grant(&coordinator, id, morning, "x").await),
        401
    );
    // Revoked: 401.
    identity.service.revoke_grant(&alice, id).await.unwrap();
    assert_eq!(
        status(run_on_grant(&coordinator, id, morning, "x").await),
        401
    );
    // Only the one run that was allowed reached the agent.
    assert_eq!(recorder.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_grant_with_a_txn_token_is_400_and_an_agent_without_identity_service_answers_401() {
    let identity = identity_service().await;
    let grant = alices_grant(&identity).await;
    let recorder = Arc::new(Recorder::default());
    let gate = |delegating: bool| {
        let gate = SpiffeGate::new(
            Arc::new(FakeIdentity::new(COORDINATOR)),
            schedules(),
            COORDINATOR.into(),
            false,
        );
        Arc::new(match delegating {
            true => gate.delegating_to(client(&identity.url, COORDINATOR)),
            false => gate,
        })
    };
    let with = serve(grauwolf_server::with_spiffe_auth(
        recorded(&recorder),
        None,
        Users::default(),
        Some(gate(true)),
    ))
    .await;
    let response = reqwest::Client::new()
        .post(format!("{with}/run"))
        .bearer_auth(test_token(MORNING, COORDINATOR, now_secs(), 300))
        .header("Grauwolf-Grant", &grant.id)
        .header("Txn-Token", alices_txn_token(&identity).await)
        .json(&json!({ "input": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let without = serve(grauwolf_server::with_spiffe_auth(
        recorded(&recorder),
        None,
        Users::default(),
        Some(gate(false)),
    ))
    .await;
    let (status, _) = run_on_grant(&without, &grant.id, Ok((MORNING, COORDINATOR)), "x").await;
    assert_eq!(status, 401);
}
