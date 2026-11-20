//! OpenBao (and HashiCorp Vault): login with the JWT-SVID through the JWT auth method, KV v2
//! reads (the HTTP calls are `grauwolf-openbao`). The token is kept in memory and replaced by a fresh login before it expires and after
//! a `403`. Values and tokens appear in no error.

use std::sync::Arc;
use std::time::{Duration, Instant};

use grauwolf_openbao::{Bao, HttpTransport};
use grauwolf_spiffe::WorkloadIdentity;
use tokio::sync::Mutex;

use crate::config::OpenBaoConfig;

/// A token this close to its end is replaced before it is used.
const TOKEN_MARGIN: Duration = Duration::from_secs(30);

struct Token {
    value: String,
    expires: Instant,
}

/// The logged-in token for other OpenBao calls, e.g. Transit signing of the identity service
/// (ADR-0111).
impl grauwolf_openbao::TokenSource for OpenBaoClient {
    fn token(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
    {
        Box::pin(OpenBaoClient::token(self))
    }
}

/// A client of one OpenBao server, logged in with the agent's identity.
pub struct OpenBaoClient {
    bao: Bao<HttpTransport>,
    config: OpenBaoConfig,
    identity: Arc<dyn WorkloadIdentity>,
    token: Mutex<Option<Token>>,
}

impl OpenBaoClient {
    /// A client that logs in as configured, with JWT-SVIDs from `identity`.
    pub fn new(config: OpenBaoConfig, identity: Arc<dyn WorkloadIdentity>) -> Self {
        Self {
            bao: Bao::new(HttpTransport::new(&config.address)),
            config,
            identity,
            token: Mutex::new(None),
        }
    }

    /// The field `field` of the KV v2 secret `path` in `mount`. One fresh login and one retry
    /// if the server answers `403` (the token was revoked or has expired early).
    pub async fn read_field(&self, mount: &str, path: &str, field: &str) -> Result<String, String> {
        for attempt in 0..2 {
            let token = self.token().await?;
            let bao = Bao::new(HttpTransport::new(&self.config.address)).with_token(token);
            match bao.kv_read_field(mount, path, field).await {
                Ok(value) => return Ok(value),
                Err(error) if error.status == Some(403) && attempt == 0 => {
                    *self.token.lock().await = None;
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        Err("access denied after a new login (403)".to_owned())
    }

    /// The current token; logs in if there is none or it is about to expire.
    async fn token(&self) -> Result<String, String> {
        let mut guard = self.token.lock().await;
        if let Some(token) = guard.as_ref()
            && token.expires > Instant::now() + TOKEN_MARGIN
        {
            return Ok(token.value.clone());
        }
        let token = self.login().await?;
        let value = token.value.clone();
        *guard = Some(token);
        Ok(value)
    }

    async fn login(&self) -> Result<Token, String> {
        let login = &self.config.login;
        let svid = self
            .identity
            .jwt_svid(&login.audience)
            .await
            .map_err(|error| format!("no JWT-SVID for the login: {error}"))?;
        let answer = self
            .bao
            .login_jwt(&login.mount, &login.role, svid.token())
            .await
            .map_err(|error| match error.status {
                None => format!(
                    "login as {} failed: {}",
                    svid.spiffe_id(),
                    error.message.trim_start_matches("request failed: ")
                ),
                Some(_) => format!("login as {}: {}", svid.spiffe_id(), error),
            })?;
        Ok(Token {
            value: answer.value,
            expires: Instant::now() + Duration::from_secs(answer.lease_seconds),
        })
    }
}
