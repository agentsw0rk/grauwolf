//! `grauwolf identity grant create | list | revoke` (ADR-0111 stage 3) against an identity
//! service of the server test kit, with a mocked device code endpoint whose sign-in returns a
//! refresh token of the fake Entra; `grauwolf call` with a token against a served agent.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use grauwolf_cli::{Captured, Io, run};
use grauwolf_identity::testing::FakeEntra;
use grauwolf_identity::{
    EntraVerifier, GrantStore, IdentityPolicy, IdentityService, MemoryGrantStore, MemorySigner,
    ServiceSettings, TokenSigner,
};
use grauwolf_test::TestDir;
use serde_json::json;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TENANT: &str = "11111111-1111-1111-1111-111111111111";
const ENTRA: &str = "https://entra.example/11111111-1111-1111-1111-111111111111/v2.0";
const COORDINATOR: &str = "spiffe://agents.local/ns/coordinator/sa/coordinator";
const MORNING: &str = "spiffe://agents.local/ns/schedules/sa/morning";

fn grauwolf(args: &[&str]) -> (ExitCode, Captured) {
    let (io, captured) = Io::captured();
    let code = run(std::iter::once("grauwolf").chain(args.iter().copied()), &io);
    (code, captured)
}

struct Fixture {
    runtime: tokio::runtime::Runtime,
    entra: FakeEntra,
    store: Arc<MemoryGrantStore>,
    identity_url: String,
    authority: MockServer,
    dir: TestDir,
}

/// The identity service (fake Entra, grants in memory) and the mocked device code endpoint on a
/// runtime of their own; a login cache of alice in `dir`.
fn fixture() -> Fixture {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let entra = FakeEntra::new(ENTRA);
    let store = Arc::new(MemoryGrantStore::new());
    let (identity_url, authority) = runtime.block_on(async {
        let authority = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/keys"))
            .respond_with(ResponseTemplate::new(200).set_body_json(entra.jwks()))
            .mount(&authority)
            .await;
        let service = IdentityService::new(
            ServiceSettings {
                issuer: "https://identity.agents.local".into(),
                trust_domain: "agents.local".into(),
                person_issuer: ENTRA.into(),
                lifetime: Duration::from_secs(900),
                max_chain_lifetime: Duration::from_secs(8 * 3600),
            },
            Arc::new(MemorySigner::generate("k1")) as Arc<dyn TokenSigner>,
            Arc::new(EntraVerifier::new(
                reqwest::Client::new(),
                entra.settings(&format!("{}/keys", authority.uri())),
            )),
            IdentityPolicy::from_json(
                &json!({ "grants": { "agents": { COORDINATOR: {} }, "triggers": [MORNING] } })
                    .to_string(),
            )
            .unwrap(),
        )
        .with_grants(Arc::clone(&store) as Arc<dyn GrantStore>, entra.redeemer());
        let router = grauwolf_server::identity_router(Arc::new(service), None);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (url, authority)
    });
    let dir = TestDir::new("grauwolf-cli-grants");
    let cache = json!({
        "authority": authority.uri(), "tenant": TENANT, "client_id": "cli-app",
        "scope": "api://grauwolf-identity/access_as_user", "identity_url": identity_url,
        "access_token": entra.token("oid-alice", "Alice"), "refresh_token": "rt-login",
        "expires_at": 4_000_000_000_u64,
    });
    std::fs::write(dir.path().join("login.json"), cache.to_string()).unwrap();
    Fixture {
        runtime,
        entra,
        store,
        identity_url,
        authority,
        dir,
    }
}

/// The device code sign-in of `grant create`: it answers at once with `refresh_token`.
fn mount_sign_in(fixture: &Fixture, refresh_token: &str) {
    let base = format!("/{TENANT}/oauth2/v2.0");
    let access = fixture.entra.token("oid-alice", "Alice");
    fixture.runtime.block_on(async {
        Mock::given(method("POST"))
            .and(path(format!("{base}/devicecode")))
            .and(body_string_contains("offline_access"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "device_code": "dc", "user_code": "BCDF-GHJK",
                "verification_uri": "https://login.example/devicecode",
                "expires_in": 30, "interval": 0})))
            .mount(&fixture.authority)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{base}/token")))
            .and(body_string_contains("device_code=dc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": access, "refresh_token": refresh_token, "expires_in": 3600})))
            .mount(&fixture.authority)
            .await;
    });
}

fn dir_arg(fixture: &Fixture) -> String {
    fixture.dir.path().display().to_string()
}

#[test]
fn a_person_creates_lists_and_revokes_a_grant_and_the_cli_keeps_no_refresh_token() {
    let fixture = fixture();
    let refresh = fixture.entra.refresh_token("oid-alice", "Alice");
    mount_sign_in(&fixture, &refresh);
    let dir = dir_arg(&fixture);
    let before = std::fs::read_to_string(fixture.dir.path().join("login.json")).unwrap();

    let (code, out) = grauwolf(&[
        "identity",
        "grant",
        "create",
        "--agent",
        COORDINATOR,
        "--trigger",
        MORNING,
        "--valid",
        "7d",
        "--max-run",
        "2h",
        "--config-dir",
        &dir,
    ]);
    assert_eq!(code, ExitCode::SUCCESS, "{}", out.stderr());
    let stdout = out.stdout();
    assert!(stdout.contains("BCDF-GHJK"), "{stdout}");
    assert!(stdout.contains("max run:   2 h"), "{stdout}");
    assert!(!stdout.contains(&refresh), "{stdout}");
    let grants = fixture.runtime.block_on(fixture.store.list()).unwrap();
    assert_eq!(grants.len(), 1);
    let grant = &grants[0];
    assert!(
        stdout.contains(&format!("grant {} created", grant.id)),
        "{stdout}"
    );
    assert_eq!(grant.max_run, 7200);
    assert!(grant.valid_until.abs_diff(grant.created + 7 * 86_400) <= 5);
    // The login cache is untouched and nothing else is written.
    assert_eq!(
        std::fs::read_to_string(fixture.dir.path().join("login.json")).unwrap(),
        before
    );
    let files: Vec<_> = std::fs::read_dir(fixture.dir.path()).unwrap().collect();
    assert_eq!(files.len(), 1);

    let (code, out) = grauwolf(&[
        "identity",
        "grant",
        "list",
        "--config-dir",
        &dir,
        "--identity-url",
        &fixture.identity_url,
    ]);
    assert_eq!(code, ExitCode::SUCCESS, "{}", out.stderr());
    assert!(out.stdout().contains(&grant.id), "{}", out.stdout());
    assert!(out.stdout().contains("last used never"), "{}", out.stdout());

    let (code, out) = grauwolf(&[
        "identity",
        "grant",
        "revoke",
        &grant.id,
        "--config-dir",
        &dir,
    ]);
    assert_eq!(code, ExitCode::SUCCESS, "{}", out.stderr());
    assert!(
        fixture
            .runtime
            .block_on(fixture.store.list())
            .unwrap()
            .is_empty()
    );
    let (code, out) = grauwolf(&[
        "identity",
        "grant",
        "revoke",
        &grant.id,
        "--config-dir",
        &dir,
    ]);
    assert_eq!(code, ExitCode::from(1));
    assert!(
        out.stderr().contains("you have no grant"),
        "{}",
        out.stderr()
    );
}

#[test]
fn a_grant_outside_the_policy_is_refused_with_the_reason() {
    let fixture = fixture();
    let refresh = fixture.entra.refresh_token("oid-alice", "Alice");
    mount_sign_in(&fixture, &refresh);
    let (code, out) = grauwolf(&[
        "identity",
        "grant",
        "create",
        "--agent",
        COORDINATOR,
        "--trigger",
        "spiffe://agents.local/ns/ci/sa/runner",
        "--config-dir",
        &dir_arg(&fixture),
    ]);
    assert_eq!(code, ExitCode::from(1));
    assert!(out.stderr().contains("grants.triggers"), "{}", out.stderr());
    assert!(!out.stderr().contains(&refresh));
    let (code, out) = grauwolf(&[
        "identity",
        "grant",
        "create",
        "--agent",
        COORDINATOR,
        "--trigger",
        MORNING,
        "--valid",
        "91d",
        "--config-dir",
        &dir_arg(&fixture),
    ]);
    assert_eq!(code, ExitCode::from(1));
    assert!(out.stderr().contains("90 days"), "{}", out.stderr());
}
