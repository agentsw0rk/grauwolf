//! `Decider` for a System One service (ADR-0100): `POST {base_url}/v1/systemone`, the API that
//! RKEV implements with small decision models.
//!
//! One question per request with the question ID `decision`. A `503` (all slots busy) and
//! network errors are transient, so the retry policy applies; every other error carries the
//! service's own message.

mod wire;

use std::time::Duration;

use futures_util::future::BoxFuture;
use grauwolf_agents::decision::{DecideError, Decider, Decision, Verdict};
use serde_json::Value;

/// ID of the single question of every request.
const QUESTION_ID: &str = "decision";

/// Maximum duration for connecting.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum duration of a whole request; a decision model answers in well under a second.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Client of a System One service.
#[derive(Clone)]
pub struct SystemOneClient {
    http: reqwest::Client,
    endpoint: String,
    api_key: Option<String>,
}

/// Shows no API key.
impl std::fmt::Debug for SystemOneClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemOneClient")
            .field("endpoint", &self.endpoint)
            .field("api_key", &self.api_key.as_ref().map(|_| "…"))
            .finish()
    }
}

fn http_client(request_timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(request_timeout)
        .build()
        .expect("a reqwest client with timeouts can always be built")
}

impl SystemOneClient {
    /// Client for the service at `base_url`, e.g. `http://localhost:8090`.
    pub fn new(base_url: impl AsRef<str>) -> Self {
        Self {
            http: http_client(REQUEST_TIMEOUT),
            endpoint: format!("{}/v1/systemone", base_url.as_ref().trim_end_matches('/')),
            api_key: None,
        }
    }

    /// Sends `Authorization: Bearer <api_key>` with every request.
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Waits at most `timeout` for a whole request (default [`REQUEST_TIMEOUT`]).
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.http = http_client(timeout);
        self
    }

    /// Client from the variables `RKEV_BASE_URL` (required) and `RKEV_API_KEY` (optional);
    /// `None` without a base URL.
    pub fn from_env(env: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let base_url = env("RKEV_BASE_URL").filter(|url| !url.trim().is_empty())?;
        let client = Self::new(base_url.trim());
        Some(match env("RKEV_API_KEY").filter(|key| !key.is_empty()) {
            Some(key) => client.with_api_key(key),
            None => client,
        })
    }

    async fn ask(&self, decision: &Decision, state: &Value) -> Result<Verdict, DecideError> {
        let body = wire::request(decision, state, QUESTION_ID);
        let mut request = self.http.post(&self.endpoint).json(&body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request.send().await.map_err(|error| {
            // Timeouts, refused connections and the like may pass; asking again may help.
            DecideError::Transient(format!("System One is not reachable: {error}"))
        })?;
        let status = response.status();
        let text = response.text().await.map_err(|error| {
            DecideError::Transient(format!("System One answer could not be read: {error}"))
        })?;
        if !status.is_success() {
            let message = wire::error_message(&text);
            let error = format!("System One returned HTTP {}: {message}", status.as_u16());
            // 503: all slots are busy; the service answers at once, so a later try may work.
            return Err(if status.as_u16() == 503 {
                DecideError::Transient(error)
            } else {
                DecideError::Failed(error)
            });
        }
        wire::verdict(decision, &text, QUESTION_ID).map_err(DecideError::Failed)
    }
}

impl Decider for SystemOneClient {
    fn decide<'a>(
        &'a self,
        decision: &'a Decision,
        state: &'a Value,
    ) -> BoxFuture<'a, Result<Verdict, DecideError>> {
        Box::pin(self.ask(decision, state))
    }
}
