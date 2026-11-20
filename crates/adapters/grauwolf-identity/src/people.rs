//! The clients of people and triggers (ADR-0111 stage 3): a person manages grants at
//! `/api/identity/grants` with their access token (`grauwolf identity grant …`), and a trigger
//! calls an entry agent with its workload identity and a grant (`grauwolf call`).

use std::sync::Arc;
use std::time::Duration;

use grauwolf_spiffe::ClientAuth;
use serde_json::{Value, json};

use crate::IdentityError;
use crate::grant::{GRANT_HEADER, Grant, GrantRequest};

/// Timeout of one call to the identity service.
const GRANTS_TIMEOUT: Duration = Duration::from_secs(30);

/// The error of a refused answer, by its status; the text is the service's `error`.
fn refused(status: u16, body: &Value) -> IdentityError {
    let reason = body["error"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("HTTP {status}"));
    match status {
        400 => IdentityError::BadRequest(reason),
        401 => IdentityError::Unauthorized(reason),
        403 => IdentityError::Forbidden(reason),
        404 => IdentityError::NotFound(reason),
        _ => IdentityError::Unavailable(format!("HTTP {status}: {reason}")),
    }
}

/// `/api/identity/grants` of an identity service, for a person.
pub struct GrantsClient {
    base: String,
    http: reqwest::Client,
}

impl GrantsClient {
    /// The identity service at `base_url` (its public URL).
    pub fn new(base_url: &str) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(GRANTS_TIMEOUT)
            .build()
            .map_err(|error| format!("cannot build the HTTP client: {error}"))?;
        Ok(Self {
            base: base_url.trim_end_matches('/').to_owned(),
            http,
        })
    }

    fn url(&self, rest: &str) -> String {
        format!("{}/api/identity/grants{rest}", self.base)
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<(u16, Value), IdentityError> {
        let response = request.send().await.map_err(|error| {
            IdentityError::Unavailable(format!(
                "the identity service {} cannot be reached: {error}",
                self.base
            ))
        })?;
        let status = response.status().as_u16();
        let body = response.json().await.unwrap_or_default();
        Ok((status, body))
    }

    /// Creates a grant with the refresh token of a separate sign-in; the token is sent once and
    /// not kept.
    pub async fn create(
        &self,
        access_token: &str,
        request: &GrantRequest,
        refresh_token: &str,
    ) -> Result<Grant, IdentityError> {
        let mut body = serde_json::to_value(request)
            .map_err(|error| IdentityError::BadRequest(error.to_string()))?;
        body["refresh_token"] = json!(refresh_token);
        let (status, body) = self
            .send(
                self.http
                    .post(self.url(""))
                    .bearer_auth(access_token)
                    .json(&body),
            )
            .await?;
        if status != 201 {
            return Err(refused(status, &body));
        }
        serde_json::from_value(body)
            .map_err(|error| IdentityError::Unavailable(format!("unexpected answer: {error}")))
    }

    /// The person's own grants.
    pub async fn list(&self, access_token: &str) -> Result<Vec<Grant>, IdentityError> {
        let (status, body) = self
            .send(self.http.get(self.url("")).bearer_auth(access_token))
            .await?;
        if status != 200 {
            return Err(refused(status, &body));
        }
        serde_json::from_value(body["grants"].clone())
            .map_err(|error| IdentityError::Unavailable(format!("unexpected answer: {error}")))
    }

    /// Revokes the person's grant `id` (metadata and refresh token).
    pub async fn revoke(&self, access_token: &str, id: &str) -> Result<(), IdentityError> {
        let (status, body) = self
            .send(
                self.http
                    .delete(self.url(&format!("/{id}")))
                    .bearer_auth(access_token),
            )
            .await?;
        match status {
            204 | 200 => Ok(()),
            _ => Err(refused(status, &body)),
        }
    }
}

/// How a trigger authenticates at the agent.
pub enum CallerCredential {
    /// The workload identity: mTLS (`https://`) or a JWT-SVID (`http://`).
    Workload(Arc<ClientAuth>),
    /// A bearer token (shared token, user token, a person's access token).
    Bearer(String),
    /// Nothing (an open agent).
    None,
}

/// The answer of an agent's `/run`.
#[derive(Debug, Clone, PartialEq)]
pub struct CallAnswer {
    pub status: u16,
    pub body: Value,
}

/// Posts `{ "input": input }` to `url` (an agent's `/run`) with `credential` and, if given, the
/// grant (`Grauwolf-Grant`). Waits up to `timeout`. A transport error is the `Err`; any HTTP
/// answer is a [`CallAnswer`].
pub async fn call_agent(
    url: &str,
    credential: &CallerCredential,
    grant: Option<&str>,
    input: Value,
    timeout: Duration,
) -> Result<CallAnswer, String> {
    let mut builder = reqwest::Client::builder().timeout(timeout);
    if let CallerCredential::Workload(auth) = credential
        && let Some(tls) = auth.tls_config().map_err(|error| error.to_string())?
    {
        builder = builder.use_preconfigured_tls(tls);
    }
    let http = builder
        .build()
        .map_err(|error| format!("cannot build the HTTP client: {error}"))?;
    let mut request = http.post(url).json(&json!({ "input": input }));
    match credential {
        CallerCredential::Workload(auth) => {
            if let Some(bearer) = auth.bearer().await.map_err(|error| error.to_string())? {
                request = request.bearer_auth(bearer);
            }
        }
        CallerCredential::Bearer(token) => request = request.bearer_auth(token),
        CallerCredential::None => {}
    }
    if let Some(grant) = grant {
        request = request.header(GRANT_HEADER, grant);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("{url} cannot be reached: {error}"))?;
    let status = response.status().as_u16();
    let text = response
        .text()
        .await
        .map_err(|error| format!("{url}: {error}"))?;
    let body = serde_json::from_str(&text).unwrap_or(Value::String(text));
    Ok(CallAnswer { status, body })
}
