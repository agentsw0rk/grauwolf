//! Redeeming a grant's refresh token as the public client (wiremock token endpoint).

use grauwolf_identity::{IdentityError, PublicClientRefresh, RefreshRedeemer, TokenEndpoint};
use serde_json::json;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn the_token_endpoint_comes_from_discovery_and_a_rotated_token_is_returned() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/t1/v2.0/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "token_endpoint": format!("{}/t1/oauth2/v2.0/token", server.uri()) }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/t1/oauth2/v2.0/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("client_id=cli-id"))
        .and(body_string_contains("refresh_token=rt-old"))
        .and(body_string_contains("offline_access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "access_token": "at", "refresh_token": "rt-new", "expires_in": 3600 }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/t1/oauth2/v2.0/token"))
        .and(body_string_contains("refresh_token=rt-kept"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "access_token": "at2" })))
        .mount(&server)
        .await;
    let refresh = PublicClientRefresh::new(
        TokenEndpoint::Discovery(format!("{}/t1/v2.0", server.uri())),
        "cli-id",
        "api://grauwolf-identity/access_as_user offline_access",
    )
    .unwrap();
    let redeemed = refresh.redeem("rt-old").await.unwrap();
    assert_eq!(
        (
            redeemed.access_token.as_str(),
            redeemed.refresh_token.as_str()
        ),
        ("at", "rt-new")
    );
    // Without rotation the old token stays the one to store.
    let kept = refresh.redeem("rt-kept").await.unwrap();
    assert_eq!(kept.refresh_token, "rt-kept");
    assert!(!format!("{kept:?}").contains("rt-kept"));
}

#[tokio::test]
async fn a_refused_token_is_unauthorized_and_an_unreachable_endpoint_unavailable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("refresh_token=rt-gone"))
        .respond_with(ResponseTemplate::new(400).set_body_json(
            json!({ "error": "invalid_grant", "error_description": "AADSTS50057: user disabled" }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("refresh_token=rt-down"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let refresh = |url: String| {
        PublicClientRefresh::new(TokenEndpoint::Url(url), "cli-id", "scope offline_access").unwrap()
    };
    let error = refresh(format!("{}/token", server.uri()))
        .redeem("rt-gone")
        .await
        .unwrap_err();
    assert!(
        matches!(&error, IdentityError::Unauthorized(text) if text.contains("invalid_grant") && !text.contains("rt-gone")),
        "{error:?}"
    );
    let error = refresh(format!("{}/token", server.uri()))
        .redeem("rt-down")
        .await
        .unwrap_err();
    assert!(matches!(error, IdentityError::Unavailable(_)), "{error:?}");
    let error = refresh("http://127.0.0.1:9/token".into())
        .redeem("rt-x")
        .await
        .unwrap_err();
    assert!(matches!(error, IdentityError::Unavailable(_)), "{error:?}");
}
