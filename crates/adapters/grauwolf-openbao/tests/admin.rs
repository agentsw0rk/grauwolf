//! The admin calls against a mocked OpenBao API.

use grauwolf_openbao::{Bao, HttpTransport, Outcome, jwt_role, kv_read_policy};
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn bao(server: &MockServer) -> Bao<HttpTransport> {
    Bao::new(HttpTransport::new(&server.uri())).with_token("root")
}

#[tokio::test]
async fn jwt_auth_is_enabled_and_configured_once() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/sys/auth"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data": {"token/": {"type": "token"}}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/sys/auth/jwt"))
        .and(header("X-Vault-Token", "root"))
        .and(body_partial_json(json!({"type": "jwt"})))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/auth/jwt/config"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/jwt/config"))
        .and(body_partial_json(json!({"jwks_url": "http://x/keys"})))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let outcome = bao(&server)
        .ensure_jwt_auth("jwt", "http://x/keys", None)
        .await
        .unwrap();
    assert_eq!(outcome, Outcome::Created);
}

#[tokio::test]
async fn jwt_auth_that_matches_is_left_alone() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/sys/auth"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"jwt/": {"type": "jwt"}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/auth/jwt/config"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data": {"jwks_url": "http://x/keys", "bound_issuer": ""}})),
        )
        .mount(&server)
        .await;
    // No POST mock: a write would answer 404 and fail the call.
    let outcome = bao(&server)
        .ensure_jwt_auth("jwt", "http://x/keys", None)
        .await
        .unwrap();
    assert_eq!(outcome, Outcome::Unchanged);
}

#[tokio::test]
async fn kv_mount_of_the_wrong_kind_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/sys/mounts"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"data": {"kv/": {"type": "kv", "options": {"version": "1"}}}}),
            ),
        )
        .mount(&server)
        .await;
    let error = bao(&server).ensure_kv2("kv").await.unwrap_err();
    assert!(error.to_string().contains("not as KV v2"), "{error}");
}

#[tokio::test]
async fn role_is_written_when_it_differs_and_only_then() {
    let server = MockServer::start().await;
    let desired = jwt_role("openbao", "spiffe://d/ns/a/sa/a", "agent-a");
    Mock::given(method("GET"))
        .and(path("/v1/auth/jwt/role/agent-a"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {
            "role_type": "jwt", "user_claim": "sub", "bound_audiences": ["openbao"],
            "bound_subject": "spiffe://d/ns/a/sa/other", "token_policies": ["agent-a"], "token_ttl": 3600, "extra": 1}})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/jwt/role/agent-a"))
        .and(body_partial_json(
            json!({"bound_subject": "spiffe://d/ns/a/sa/a"}),
        ))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let outcome = bao(&server)
        .ensure_jwt_role("jwt", "agent-a", &desired)
        .await
        .unwrap();
    assert_eq!(outcome, Outcome::Updated);
}

#[tokio::test]
async fn policy_text_is_compared_and_limited_to_the_prefix() {
    let hcl = kv_read_policy("kv", "agents/a");
    assert_eq!(
        hcl,
        "path \"kv/data/agents/a/*\" {\n  capabilities = [\"read\"]\n}\n"
    );
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/sys/policies/acl/agent-a"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"policy": hcl}})))
        .mount(&server)
        .await;
    assert_eq!(
        bao(&server).ensure_policy("agent-a", &hcl).await.unwrap(),
        Outcome::Unchanged
    );
}

#[tokio::test]
async fn kv_tree_is_removed_recursively_and_errors_hold_no_value() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/kv/metadata/agents/a"))
        .and(query_param("list", "true"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data": {"keys": ["jira", "sub/"]}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/kv/metadata/agents/a/sub"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"keys": ["deep"]}})))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(204))
        .expect(2)
        .mount(&server)
        .await;
    let removed = bao(&server).kv_delete_tree("kv", "agents/a").await.unwrap();
    assert_eq!(removed, vec!["agents/a/jira", "agents/a/sub/deep"]);

    let failing = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(json!({"errors": ["permission denied"]})),
        )
        .mount(&failing)
        .await;
    let error = bao(&failing)
        .kv_put_field("kv", "agents/a/jira", "value", "s3cret-value")
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "write: HTTP 403: permission denied");
    assert!(!error.to_string().contains("s3cret"));
}

#[tokio::test]
async fn capabilities_of_the_token_are_returned_per_path() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/sys/capabilities-self"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data": {"sys/auth/jwt": ["read", "update"], "kv/data/agents/x": ["deny"]}}),
        ))
        .mount(&server)
        .await;
    let caps = bao(&server)
        .capabilities_self(&["sys/auth/jwt", "kv/data/agents/x"])
        .await
        .unwrap();
    assert_eq!(caps["sys/auth/jwt"], vec!["read", "update"]);
    assert_eq!(caps["kv/data/agents/x"], vec!["deny"]);
}

#[tokio::test]
async fn health_reads_a_sealed_server() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/sys/health"))
        .respond_with(
            ResponseTemplate::new(503)
                .set_body_json(json!({"initialized": true, "sealed": true, "version": "2.7.2"})),
        )
        .mount(&server)
        .await;
    let health = bao(&server).health().await.unwrap();
    assert!(health.sealed && health.initialized);
    assert_eq!(health.version.as_deref(), Some("2.7.2"));
}

#[tokio::test]
async fn a_token_is_created_without_a_parent_with_only_the_given_policy() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/token/create"))
        .and(header("X-Vault-Token", "root"))
        .and(body_partial_json(json!({
            "policies": ["grauwolf-k8-ops"], "ttl": "8h", "renewable": true,
            "no_default_policy": true, "no_parent": true})))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"auth": {"client_token": "s.new", "policies": ["grauwolf-k8-ops"]}}),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let token = bao(&server)
        .create_token("grauwolf-k8-ops", "8h", true)
        .await
        .unwrap();

    assert_eq!(token, "s.new");
}

#[tokio::test]
async fn a_token_that_may_not_create_orphans_makes_a_child_token_and_a_refusal_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/token/create"))
        .and(body_partial_json(json!({"no_parent": true})))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(json!({"errors": ["no_parent needs sudo"]})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/token/create"))
        .and(body_partial_json(json!({"no_parent": false})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"auth": {"client_token": "s.child"}})),
        )
        .mount(&server)
        .await;
    assert_eq!(
        bao(&server).create_token("p", "1h", false).await.unwrap(),
        "s.child"
    );

    let denied = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/token/create"))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(json!({"errors": ["permission denied"]})),
        )
        .mount(&denied)
        .await;
    let error = bao(&denied)
        .create_token("p", "1h", false)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("permission denied"), "{error}");
}

#[tokio::test]
async fn the_policies_of_the_token_in_use_are_looked_up() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/auth/token/lookup-self"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data": {"policies": ["root"], "id": "never-printed"}})),
        )
        .mount(&server)
        .await;
    assert_eq!(bao(&server).token_policies().await.unwrap(), ["root"]);
}

#[tokio::test]
async fn a_transit_key_is_created_once_and_one_of_another_type_is_refused() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/sys/mounts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {}})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/sys/mounts/transit"))
        .and(body_partial_json(json!({"type": "transit"})))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/transit/keys/new"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/transit/keys/new"))
        .and(body_partial_json(json!({"type": "ecdsa-p256"})))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/transit/keys/old"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data": {"type": "rsa-2048"}})),
        )
        .mount(&server)
        .await;
    let bao = bao(&server);
    assert_eq!(
        bao.ensure_transit("transit").await.unwrap(),
        Outcome::Created
    );
    assert_eq!(
        bao.ensure_transit_key("transit", "new", "ecdsa-p256")
            .await
            .unwrap(),
        Outcome::Created
    );
    let error = bao
        .ensure_transit_key("transit", "old", "ecdsa-p256")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("rsa-2048"), "{error}");
}

#[tokio::test]
async fn a_transit_key_is_deleted_only_after_its_deletion_is_allowed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/transit/keys/k"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data": {"type": "ecdsa-p256"}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/transit/keys/k/config"))
        .and(body_partial_json(json!({"deletion_allowed": true})))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/v1/transit/keys/k"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/transit/keys/gone"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let bao = bao(&server);
    assert!(bao.delete_transit_key("transit", "k").await.unwrap());
    assert!(!bao.delete_transit_key("transit", "gone").await.unwrap());
}

#[tokio::test]
async fn a_whole_secret_is_read_and_a_missing_one_is_none() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/kv/data/identity/grants/grt_1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data": {"data": {"id": "grt_1", "purposes": ["review"]}, "metadata": {}}}),
        ))
        .mount(&server)
        .await;
    let bao = bao(&server);
    assert_eq!(
        bao.kv_read("kv", "identity/grants/grt_1").await.unwrap(),
        Some(json!({"id": "grt_1", "purposes": ["review"]}))
    );
    assert_eq!(
        bao.kv_read("kv/", "identity/grants/grt_2").await.unwrap(),
        None
    );
}
