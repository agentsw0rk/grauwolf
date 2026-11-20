//! The `openbao` source against a mocked OpenBao API and a fake workload identity.

use std::sync::Arc;
use std::time::Duration;

use grauwolf_config::{Env, MapEnv};
use grauwolf_credentials::{CredentialEnv, CredentialsConfig};
use grauwolf_spiffe::WorkloadIdentity;
use grauwolf_spiffe::testing::FakeIdentity;
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ID: &str = "spiffe://agentswork.de/ns/agents/sa/grauwolf";
const SECRET: &str = "jira-token-s3cret";

fn config(address: &str, refresh: &str) -> CredentialsConfig {
    CredentialsConfig::parse(
        &format!(
            r#"
[credentials.openbao]
address = "{address}"
auth = {{ spiffe = {{ audience = "openbao", role = "grauwolf-agents", mount = "jwt" }} }}
refresh = "{refresh}"
[credentials.variables]
ATLASSIAN_API_TOKEN = "openbao:kv/agents/reviewer#jira"
"#
        ),
        std::path::Path::new("."),
    )
    .unwrap()
    .unwrap()
}

fn login_ok(lease: u64) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_json(json!({"auth": {"client_token": "bao-token", "lease_duration": lease}}))
}

fn kv(value: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"data": {"data": {"jira": value}}}))
}

async fn login_mock(server: &MockServer, lease: u64) {
    Mock::given(method("POST"))
        .and(path("/v1/auth/jwt/login"))
        .and(body_partial_json(json!({"role": "grauwolf-agents"})))
        .respond_with(login_ok(lease))
        .mount(server)
        .await;
}

fn start(
    config: &CredentialsConfig,
    identity: &FakeIdentity,
) -> Result<CredentialEnv, grauwolf_credentials::CredentialError> {
    CredentialEnv::start(
        Arc::new(MapEnv::default()),
        config,
        Some(Arc::new(identity.clone()) as Arc<dyn WorkloadIdentity>),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logs_in_with_the_jwt_svid_and_reads_the_kv_field() {
    let server = MockServer::start().await;
    login_mock(&server, 3600).await;
    Mock::given(method("GET"))
        .and(path("/v1/kv/data/agents/reviewer"))
        .and(header("X-Vault-Token", "bao-token"))
        .respond_with(kv(SECRET))
        .expect(1)
        .mount(&server)
        .await;
    let identity = FakeIdentity::new(ID);
    let env = start(&config(&server.uri(), "5m"), &identity).unwrap();
    assert_eq!(env.var("ATLASSIAN_API_TOKEN").as_deref(), Some(SECRET));
    assert_eq!(identity.audiences(), vec!["openbao".to_owned()]);
    // The login body carries the token of the identity.
    let login = &server.received_requests().await.unwrap()[0];
    let body: serde_json::Value = serde_json::from_slice(&login.body).unwrap();
    assert_eq!(body["jwt"].as_str().unwrap().split('.').count(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreachable_broker_fails_the_start_naming_variable_and_source() {
    let identity = FakeIdentity::new(ID);
    let error = start(&config("http://127.0.0.1:1", "5m"), &identity)
        .unwrap_err()
        .to_string();
    assert!(error.contains("ATLASSIAN_API_TOKEN"), "{error}");
    assert!(error.contains("openbao:kv/agents/reviewer#jira"), "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_login_fails_the_start_with_the_server_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/jwt/login"))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(json!({"errors": ["role not found"]})),
        )
        .mount(&server)
        .await;
    let error = start(&config(&server.uri(), "5m"), &FakeIdentity::new(ID))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("role not found") && error.contains(ID),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_identity_fails_the_start_naming_the_variable() {
    let server = MockServer::start().await;
    let error = CredentialEnv::start(
        Arc::new(MapEnv::default()),
        &config(&server.uri(), "5m"),
        None,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("SPIFFE_ENDPOINT_SOCKET"), "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_identity_fails_the_start() {
    let identity = FakeIdentity::new(ID);
    identity.fail_with(Some("agent down"));
    let server = MockServer::start().await;
    let error = start(&config(&server.uri(), "5m"), &identity)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("agent down") && error.contains("ATLASSIAN_API_TOKEN"),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn errors_never_contain_secret_values() {
    let server = MockServer::start().await;
    login_mock(&server, 3600).await;
    Mock::given(method("GET"))
        .and(path("/v1/kv/data/agents/reviewer"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data": {"data": {"other": SECRET, "jira": {"nested": SECRET}}}}),
        ))
        .mount(&server)
        .await;
    let error = start(&config(&server.uri(), "5m"), &FakeIdentity::new(ID))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("not a string") && !error.contains(SECRET),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_secret_and_a_missing_field_are_errors() {
    let server = MockServer::start().await;
    login_mock(&server, 3600).await;
    Mock::given(method("GET"))
        .and(path("/v1/kv/data/agents/reviewer"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"errors": []})))
        .mount(&server)
        .await;
    let error = start(&config(&server.uri(), "5m"), &FakeIdentity::new(ID))
        .unwrap_err()
        .to_string();
    assert!(error.contains("HTTP 404"), "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_403_triggers_one_new_login_and_a_retry() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/jwt/login"))
        .respond_with(login_ok(3600))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/kv/data/agents/reviewer"))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(json!({"errors": ["permission denied"]})),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/kv/data/agents/reviewer"))
        .respond_with(kv(SECRET))
        .with_priority(2)
        .mount(&server)
        .await;
    let identity = FakeIdentity::new(ID);
    let env = start(&config(&server.uri(), "5m"), &identity).unwrap();
    assert_eq!(env.var("ATLASSIAN_API_TOKEN").as_deref(), Some(SECRET));
    assert_eq!(
        identity.audiences().len(),
        2,
        "a fresh JWT-SVID for the second login"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn values_are_read_again_after_the_refresh_interval_with_the_same_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/jwt/login"))
        .respond_with(login_ok(3600))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/kv/data/agents/reviewer"))
        .respond_with(kv("first"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/kv/data/agents/reviewer"))
        .respond_with(kv("second"))
        .with_priority(2)
        .mount(&server)
        .await;
    let mut config = config(&server.uri(), "5m");
    config.refresh = Duration::from_millis(100);
    let env = start(&config, &FakeIdentity::new(ID)).unwrap();
    assert_eq!(env.var("ATLASSIAN_API_TOKEN").as_deref(), Some("first"));
    for _ in 0..100 {
        if env.var("ATLASSIAN_API_TOKEN").as_deref() == Some("second") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the value was not refreshed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expiring_token_is_replaced_by_a_new_login_on_refresh() {
    let server = MockServer::start().await;
    // A lease shorter than the margin: every use needs a new login.
    Mock::given(method("POST"))
        .and(path("/v1/auth/jwt/login"))
        .respond_with(login_ok(10))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/kv/data/agents/reviewer"))
        .respond_with(kv(SECRET))
        .mount(&server)
        .await;
    let identity = FakeIdentity::new(ID);
    let mut config = config(&server.uri(), "5m");
    config.refresh = Duration::from_millis(100);
    let _env = start(&config, &identity).unwrap();
    for _ in 0..100 {
        if identity.audiences().len() >= 2 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no second login");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_refresh_keeps_the_previous_value() {
    let server = MockServer::start().await;
    login_mock(&server, 3600).await;
    Mock::given(method("GET"))
        .and(path("/v1/kv/data/agents/reviewer"))
        .respond_with(kv("kept"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/kv/data/agents/reviewer"))
        .respond_with(ResponseTemplate::new(500))
        .with_priority(2)
        .mount(&server)
        .await;
    let mut config = config(&server.uri(), "5m");
    config.refresh = Duration::from_millis(100);
    let env = start(&config, &FakeIdentity::new(ID)).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(env.var("ATLASSIAN_API_TOKEN").as_deref(), Some("kept"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn check_reports_openbao_as_the_origin_and_the_length() {
    let server = MockServer::start().await;
    login_mock(&server, 3600).await;
    Mock::given(method("GET"))
        .and(path("/v1/kv/data/agents/reviewer"))
        .respond_with(kv(SECRET))
        .mount(&server)
        .await;
    let dir = grauwolf_test::TestDir::new("credentials-check-openbao");
    std::fs::write(
        dir.join("grauwolf.toml"),
        format!(
            "[credentials.openbao]\naddress = \"{}\"\n\
             auth = {{ spiffe = {{ audience = \"openbao\", role = \"grauwolf-agents\" }} }}\n\
             [credentials.variables]\nATLASSIAN_API_TOKEN = \"openbao:kv/agents/reviewer#jira\"\n",
            server.uri()
        ),
    )
    .unwrap();
    let message = CredentialEnv::check(
        Arc::new(MapEnv::default()),
        Some(&dir.join("agent.json")),
        "ATLASSIAN_API_TOKEN",
        Some(Arc::new(FakeIdentity::new(ID)) as Arc<dyn WorkloadIdentity>),
    )
    .unwrap();
    assert_eq!(
        message,
        format!(
            "resolved ATLASSIAN_API_TOKEN from openbao ({} bytes)",
            SECRET.len()
        )
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn check_many_reports_each_variable_and_one_failure_does_not_hide_the_others() {
    let server = MockServer::start().await;
    login_mock(&server, 3600).await;
    Mock::given(method("GET"))
        .and(path("/v1/kv/data/agents/reviewer"))
        .respond_with(kv(SECRET))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/kv/data/agents/other"))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(json!({"errors": ["permission denied"]})),
        )
        .mount(&server)
        .await;
    let dir = grauwolf_test::TestDir::new("credentials-check-many");
    std::fs::write(dir.join("key"), "sk-file\n").unwrap();
    std::fs::write(
        dir.join("grauwolf.toml"),
        format!(
            "[credentials.openbao]\naddress = \"{}\"\n\
             auth = {{ spiffe = {{ audience = \"openbao\", role = \"grauwolf-agents\" }} }}\n\
             [credentials.variables]\nATLASSIAN_API_TOKEN = \"openbao:kv/agents/reviewer#jira\"\n\
             GIT_TOKEN = \"openbao:kv/agents/other#jira\"\nOPENAI_API_KEY = \"file:key\"\n",
            server.uri()
        ),
    )
    .unwrap();
    let identity = || Some(Arc::new(FakeIdentity::new(ID)) as Arc<dyn WorkloadIdentity>);
    let file = dir.join("agent.json");

    let all = CredentialEnv::check_many(Arc::new(MapEnv::default()), Some(&file), &[], identity())
        .unwrap();

    let names: Vec<_> = all.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        ["ATLASSIAN_API_TOKEN", "GIT_TOKEN", "OPENAI_API_KEY"]
    );
    assert_eq!(
        all[0].1.as_deref(),
        Ok(format!(
            "resolved ATLASSIAN_API_TOKEN from openbao ({} bytes)",
            SECRET.len()
        )
        .as_str())
    );
    let denied = all[1].1.as_ref().unwrap_err();
    assert!(
        denied.contains("GIT_TOKEN") && denied.contains("permission denied"),
        "{denied}"
    );
    assert_eq!(
        all[2].1.as_deref(),
        Ok("resolved OPENAI_API_KEY from file (7 bytes)")
    );

    let some = CredentialEnv::check_many(
        Arc::new(MapEnv::default()),
        Some(&file),
        &["OPENAI_API_KEY".to_owned()],
        identity(),
    )
    .unwrap();
    assert_eq!(some.len(), 1);
    assert!(some[0].1.is_ok());
}

#[test]
fn check_many_without_a_mapping_fails_for_all_but_checks_the_environment_by_name() {
    let dir = grauwolf_test::TestDir::new("credentials-check-many-none");
    let file = dir.join("agent.json");
    let error =
        CredentialEnv::check_many(Arc::new(MapEnv::default()), Some(&file), &[], None).unwrap_err();
    assert!(error.contains("no variable is mapped"), "{error}");
    let named = CredentialEnv::check_many(
        Arc::new(MapEnv::new([("GIT_TOKEN", "abc")])),
        Some(&file),
        &["GIT_TOKEN".to_owned()],
        None,
    )
    .unwrap();
    assert_eq!(
        named[0].1.as_deref(),
        Ok("resolved GIT_TOKEN from environment (3 bytes)")
    );
}

/// The logged-in token serves other OpenBao calls, e.g. Transit signing (ADR-0111).
#[tokio::test]
async fn the_client_is_a_token_source_for_other_openbao_calls() {
    use grauwolf_openbao::TokenSource;
    let server = MockServer::start().await;
    login_mock(&server, 3600).await;
    let client = grauwolf_credentials::OpenBaoClient::new(
        config(&server.uri(), "5m").openbao.unwrap(),
        Arc::new(FakeIdentity::new(ID)),
    );
    assert_eq!(client.token().await.unwrap(), "bao-token");
}
