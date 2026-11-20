//! Redeeming a person's refresh token (ADR-0111 stage 3, Q2): the identity service redeems the
//! refresh token of a grant as the same public client that obtained it (`grauwolf identity grant
//! create`), at every run, so a disabled person or a revoked consent stops the grant. Entra may
//! rotate the refresh token; the new one is stored again.

use std::time::Duration;

use futures_util::future::BoxFuture;
use serde_json::Value;
use tokio::sync::OnceCell;

use crate::IdentityError;
use crate::device::{form_encode, oauth_error, tokens, without_url};

/// The tokens of one redemption: an access token for the grauwolf identity API (to learn and
/// check the person) and the refresh token to store (the rotated one, or the old one when the
/// server does not rotate).
#[derive(Clone)]
pub struct Redeemed {
    pub access_token: String,
    pub refresh_token: String,
}

impl std::fmt::Debug for Redeemed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Redeemed").finish_non_exhaustive()
    }
}

/// Redeems refresh tokens.
pub trait RefreshRedeemer: Send + Sync {
    /// New tokens for `refresh_token`; `Unauthorized` when the issuer refuses it (expired,
    /// revoked, the person disabled), `Unavailable` when it cannot be reached. The reason never
    /// holds a token.
    fn redeem<'a>(
        &'a self,
        refresh_token: &'a str,
    ) -> BoxFuture<'a, Result<Redeemed, IdentityError>>;
}

/// Where the token endpoint is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenEndpoint {
    /// The URL itself.
    Url(String),
    /// `token_endpoint` of the issuer's OpenID discovery document, read once.
    Discovery(String),
}

/// The public client of `grauwolf identity grant create` (`client_id`) at Entra's token endpoint,
/// with `scope` (the grauwolf identity API's scope and `offline_access`).
pub struct PublicClientRefresh {
    http: reqwest::Client,
    endpoint: TokenEndpoint,
    resolved: OnceCell<String>,
    client_id: String,
    scope: String,
}

impl PublicClientRefresh {
    pub fn new(endpoint: TokenEndpoint, client_id: &str, scope: &str) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| format!("cannot build the HTTP client: {error}"))?;
        Ok(Self {
            http,
            endpoint,
            resolved: OnceCell::new(),
            client_id: client_id.to_owned(),
            scope: scope.to_owned(),
        })
    }

    async fn token_url(&self) -> Result<&str, IdentityError> {
        self.resolved
            .get_or_try_init(|| async {
                match &self.endpoint {
                    TokenEndpoint::Url(url) => Ok(url.clone()),
                    TokenEndpoint::Discovery(issuer) => {
                        let url = format!(
                            "{}/.well-known/openid-configuration",
                            issuer.trim_end_matches('/')
                        );
                        let unavailable =
                            |reason: String| IdentityError::Unavailable(format!("{url}: {reason}"));
                        let response = self
                            .http
                            .get(&url)
                            .send()
                            .await
                            .map_err(|error| unavailable(without_url(&error)))?;
                        let body: Value = response
                            .json()
                            .await
                            .map_err(|error| unavailable(without_url(&error)))?;
                        body["token_endpoint"]
                            .as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| unavailable("no token_endpoint".into()))
                    }
                }
            })
            .await
            .map(String::as_str)
    }

    async fn call(&self, refresh_token: &str) -> Result<Redeemed, IdentityError> {
        let url = self.token_url().await?;
        let unavailable = |reason: String| {
            IdentityError::Unavailable(format!("{url} cannot be reached: {reason}"))
        };
        let response = self
            .http
            .post(url)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("accept", "application/json")
            .body(form_encode(&[
                ("grant_type", "refresh_token"),
                ("client_id", &self.client_id),
                ("refresh_token", refresh_token),
                ("scope", &self.scope),
            ]))
            .send()
            .await
            .map_err(|error| unavailable(without_url(&error)))?;
        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap_or_default();
        if status >= 500 {
            return Err(unavailable(format!("HTTP {status}")));
        }
        if status != 200 {
            return Err(IdentityError::Unauthorized(format!(
                "the refresh token was refused (HTTP {status}): {}",
                oauth_error(&body)
            )));
        }
        let tokens = tokens(&body).map_err(IdentityError::Unavailable)?;
        Ok(Redeemed {
            access_token: tokens.access_token,
            refresh_token: tokens
                .refresh_token
                .unwrap_or_else(|| refresh_token.to_owned()),
        })
    }
}

impl RefreshRedeemer for PublicClientRefresh {
    fn redeem<'a>(
        &'a self,
        refresh_token: &'a str,
    ) -> BoxFuture<'a, Result<Redeemed, IdentityError>> {
        Box::pin(self.call(refresh_token))
    }
}
