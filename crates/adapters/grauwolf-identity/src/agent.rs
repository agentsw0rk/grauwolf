//! The agent's side of delegation (ADR-0111): the client of the identity service at
//! `GRAUWOLF_IDENTITY_URL` (entry exchange, renewal, configuration, keys) and the Txn-Token a run
//! holds.

use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use grauwolf_spiffe::{ClientAuth, SpiffeId};
use serde_json::{Value, json};
use tokio::sync::{Mutex, OnceCell};

use crate::IdentityError;
use crate::grant::GRANT_TYPE;
use crate::jose::{now_secs, peek_claims};
use crate::service::TokenResponse;
use crate::txn::{ACCESS_TOKEN_TYPE, TXN_TOKEN_TYPE, TxnClaims, TxnTokenVerifier};

/// A token this close to its expiry is renewed before an outgoing call.
pub const RENEW_BEFORE: Duration = Duration::from_secs(60);

/// Timeout of one call to the identity service.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// What `GET /api/identity/config` tells an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteConfig {
    /// `iss` of people's access tokens (Entra).
    pub person_issuer: String,
}

/// The identity service, seen from an agent. Calls authenticate with the agent's workload
/// identity (`auth`: mTLS and/or a JWT-SVID as bearer).
pub struct IdentityClient {
    base: String,
    http: reqwest::Client,
    auth: Option<Arc<ClientAuth>>,
    config: OnceCell<RemoteConfig>,
    verifier: TxnTokenVerifier,
}

impl std::fmt::Debug for IdentityClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdentityClient")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl IdentityClient {
    /// The service at `base_url`; Txn-Tokens must name `trust_domain` (the agent's own) as `aud`.
    pub fn new(
        base_url: &str,
        trust_domain: &str,
        auth: Option<Arc<ClientAuth>>,
    ) -> Result<Self, String> {
        let mut builder = reqwest::Client::builder().timeout(CALL_TIMEOUT);
        if let Some(auth) = &auth
            && let Some(tls) = auth
                .tls_config()
                .map_err(|error| format!("identity service: {error}"))?
        {
            builder = builder.use_preconfigured_tls(tls);
        }
        let http = builder
            .build()
            .map_err(|error| format!("identity service: {error}"))?;
        let base = base_url.trim_end_matches('/').to_owned();
        let verifier = TxnTokenVerifier::new(
            http.clone(),
            &format!("{base}/api/identity/jwks.json"),
            trust_domain,
        );
        Ok(Self {
            base,
            http,
            auth,
            config: OnceCell::new(),
            verifier,
        })
    }

    /// The configuration of the service, fetched once.
    pub async fn config(&self) -> Result<&RemoteConfig, IdentityError> {
        self.config
            .get_or_try_init(|| async {
                let url = format!("{}/api/identity/config", self.base);
                let unavailable =
                    |reason: String| IdentityError::Unavailable(format!("{url}: {reason}"));
                let response = self
                    .http
                    .get(&url)
                    .send()
                    .await
                    .map_err(|error| unavailable(error.to_string()))?;
                if !response.status().is_success() {
                    return Err(unavailable(format!("status {}", response.status())));
                }
                let body: Value = response
                    .json()
                    .await
                    .map_err(|error| unavailable(error.to_string()))?;
                let person_issuer = body["person_issuer"]
                    .as_str()
                    .ok_or_else(|| unavailable("no person_issuer".into()))?;
                Ok(RemoteConfig {
                    person_issuer: person_issuer.to_owned(),
                })
            })
            .await
    }

    /// Whether a bearer JWT is a person's access token to exchange: its `iss` is the person
    /// issuer the identity service names. A token whose `sub` is a SPIFFE ID is a JWT-SVID and
    /// is decided without asking the service. Only the unverified claims are read here; the
    /// identity service verifies the token.
    pub async fn is_person_token(&self, token: &str) -> Result<bool, IdentityError> {
        let Some(claims) = peek_claims(token) else {
            return Ok(false);
        };
        if claims["sub"]
            .as_str()
            .is_some_and(|sub| sub.starts_with("spiffe://"))
        {
            return Ok(false);
        }
        let issuer = &self.config().await?.person_issuer;
        Ok(claims["iss"].as_str() == Some(issuer.as_str()))
    }

    /// Exchanges a person's access token for a Txn-Token at the entry of a request.
    pub async fn exchange(&self, access_token: &str) -> Result<TxnToken, IdentityError> {
        self.request(access_token, ACCESS_TOKEN_TYPE, None).await
    }

    /// Renews a Txn-Token of the same chain.
    pub async fn renew(&self, txn_token: &str) -> Result<TxnToken, IdentityError> {
        self.request(txn_token, TXN_TOKEN_TYPE, None).await
    }

    /// The entry of a grant run (ADR-0111 stage 3): the grant ID from the `Grauwolf-Grant` header
    /// and `trigger`, the authenticated SPIFFE ID of the workload that called this agent.
    pub async fn grant(&self, grant: &str, trigger: &SpiffeId) -> Result<TxnToken, IdentityError> {
        self.request(grant, GRANT_TYPE, Some(trigger.as_str()))
            .await
    }

    /// Verifies a Txn-Token that a caller presented (signature from the cached JWKS, `aud`,
    /// `exp`).
    pub async fn verify(&self, token: &str) -> Result<TxnClaims, IdentityError> {
        self.verifier.verify(token).await
    }

    async fn request(
        &self,
        subject: &str,
        kind: &str,
        trigger: Option<&str>,
    ) -> Result<TxnToken, IdentityError> {
        let url = format!("{}/api/identity/token", self.base);
        let unavailable = |reason: String| {
            IdentityError::Unavailable(format!("identity service {}: {reason}", self.base))
        };
        let mut body = json!({
            "subject_token": subject,
            "subject_token_type": kind,
        });
        if let Some(trigger) = trigger {
            body["trigger"] = json!(trigger);
        }
        let mut request = self.http.post(&url).json(&body);
        if let Some(auth) = &self.auth
            && let Some(bearer) = auth
                .bearer()
                .await
                .map_err(|error| unavailable(error.to_string()))?
        {
            request = request.bearer_auth(bearer);
        }
        let response = request
            .send()
            .await
            .map_err(|error| unavailable(error.to_string()))?;
        let status = response.status();
        if status.is_server_error() {
            return Err(unavailable(format!("status {status}")));
        }
        let body: Value = response.json().await.unwrap_or_default();
        if !status.is_success() {
            let reason = body["error"].as_str().unwrap_or("refused").to_owned();
            let message = format!("the identity service refused the token ({status}): {reason}");
            // 403: the policy of the service refuses this agent; 401: the token is refused.
            return Err(if status == reqwest::StatusCode::FORBIDDEN {
                IdentityError::Forbidden(message)
            } else {
                IdentityError::Unauthorized(message)
            });
        }
        let answer: TokenResponse = serde_json::from_value(body)
            .map_err(|error| unavailable(format!("unexpected answer: {error}")))?;
        let claims = self.verify(&answer.access_token).await?;
        Ok(TxnToken {
            token: answer.access_token,
            claims,
        })
    }
}

/// A verified Txn-Token and its claims.
#[derive(Clone)]
pub struct TxnToken {
    token: String,
    /// The verified claims.
    pub claims: TxnClaims,
}

impl std::fmt::Debug for TxnToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TxnToken")
            .field("claims", &self.claims)
            .finish_non_exhaustive()
    }
}

impl TxnToken {
    pub fn new(token: String, claims: TxnClaims) -> Self {
        Self { token, claims }
    }

    /// The compact token. A credential: never log it.
    pub fn token(&self) -> &str {
        &self.token
    }
}

/// The Txn-Token of one run (`grauwolf_tools::Delegation`): renewed at the identity service when
/// it expires within [`RENEW_BEFORE`] before an outgoing call.
pub struct RunDelegation {
    client: Arc<IdentityClient>,
    current: Mutex<TxnToken>,
}

impl RunDelegation {
    pub fn new(client: Arc<IdentityClient>, token: TxnToken) -> Self {
        Self {
            client,
            current: Mutex::new(token),
        }
    }
}

impl grauwolf_tools::Delegation for RunDelegation {
    fn txn_token(&self) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            let mut current = self.current.lock().await;
            if current.claims.exp <= now_secs() + RENEW_BEFORE.as_secs() {
                *current = self
                    .client
                    .renew(&current.token)
                    .await
                    .map_err(|error| format!("renewing the Txn-Token failed: {error}"))?;
            }
            Ok(current.token.clone())
        })
    }
}
