//! A cached JWK set fetched over HTTP: loaded on first use and fetched again when a token names
//! a key that is not in it (key rotation), at most once per [`MISS_COOLDOWN`].

use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::Mutex;

use crate::IdentityError;
use crate::jose::{Jwk, Jwks};

/// After a refresh caused by an unknown `kid`, the next such refresh waits this long, so tokens
/// with made-up key IDs cannot make the service hammer the key endpoint.
pub const MISS_COOLDOWN: Duration = Duration::from_secs(10);

/// Where the keys come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JwksLocation {
    /// The JWKS URL itself.
    Url(String),
    /// OpenID discovery: `<issuer>/.well-known/openid-configuration` names `jwks_uri`.
    Discovery(String),
}

#[derive(Default)]
struct State {
    keys: Option<Vec<Jwk>>,
    url: Option<String>,
    last_miss_refresh: Option<Instant>,
}

/// The keys of one issuer.
pub struct KeyCache {
    http: reqwest::Client,
    location: JwksLocation,
    state: Mutex<State>,
}

impl KeyCache {
    pub fn new(http: reqwest::Client, location: JwksLocation) -> Self {
        Self {
            http,
            location,
            state: Mutex::default(),
        }
    }

    /// The key `kid` (or the only key when the token names none). `Unauthorized` for a key that
    /// is not there even after a refresh, `Unavailable` when the keys cannot be fetched.
    pub async fn key(&self, kid: Option<&str>) -> Result<Jwk, IdentityError> {
        let mut state = self.state.lock().await;
        if state.keys.is_none() {
            let keys = self.fetch(&mut state).await?;
            state.keys = Some(keys);
        }
        if let Some(key) = find(state.keys.as_deref().unwrap_or_default(), kid) {
            return Ok(key);
        }
        let cooling = state
            .last_miss_refresh
            .is_some_and(|at| at.elapsed() < MISS_COOLDOWN);
        if !cooling {
            state.last_miss_refresh = Some(Instant::now());
            let keys = self.fetch(&mut state).await?;
            state.keys = Some(keys);
            if let Some(key) = find(state.keys.as_deref().unwrap_or_default(), kid) {
                return Ok(key);
            }
        }
        Err(IdentityError::Unauthorized(
            "the token is signed with an unknown key".into(),
        ))
    }

    async fn fetch(&self, state: &mut State) -> Result<Vec<Jwk>, IdentityError> {
        let url = match (&state.url, &self.location) {
            (Some(url), _) => url.clone(),
            (None, JwksLocation::Url(url)) => url.clone(),
            (None, JwksLocation::Discovery(issuer)) => {
                let discovery = format!(
                    "{}/.well-known/openid-configuration",
                    issuer.trim_end_matches('/')
                );
                let document: Value = self.get(&discovery).await?;
                let url = document["jwks_uri"].as_str().ok_or_else(|| {
                    IdentityError::Unavailable(format!("{discovery} names no jwks_uri"))
                })?;
                url.to_owned()
            }
        };
        let jwks: Jwks = serde_json::from_value(self.get(&url).await?)
            .map_err(|error| IdentityError::Unavailable(format!("{url}: {error}")))?;
        state.url = Some(url);
        Ok(jwks.keys)
    }

    async fn get(&self, url: &str) -> Result<Value, IdentityError> {
        let unavailable = |reason: String| IdentityError::Unavailable(format!("{url}: {reason}"));
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|error| unavailable(error.to_string()))?;
        if !response.status().is_success() {
            return Err(unavailable(format!("status {}", response.status())));
        }
        response
            .json()
            .await
            .map_err(|error| unavailable(error.to_string()))
    }
}

fn find(keys: &[Jwk], kid: Option<&str>) -> Option<Jwk> {
    match kid {
        Some(kid) => keys.iter().find(|key| key.kid.as_deref() == Some(kid)),
        None if keys.len() == 1 => keys.first(),
        None => None,
    }
    .cloned()
}
