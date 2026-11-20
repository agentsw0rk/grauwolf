//! A person signs in on the command line (ADR-0111, `grauwolf login`): the OAuth 2.0 device
//! authorization grant (RFC 8628) against Entra ID or Entra Local, and the refresh of the tokens
//! it returns. No token appears in an error or in `Debug` output.

use std::time::{Duration, Instant};

use serde_json::Value;

/// The device code grant type (RFC 8628 section 3.4).
pub const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// How much longer to wait after `slow_down` (RFC 8628 section 3.5).
pub const SLOW_DOWN_STEP: Duration = Duration::from_secs(5);
/// The polling interval when the server names none.
const DEFAULT_INTERVAL: u64 = 5;

/// Where the device authorization and token endpoints are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceEndpoints {
    pub device_authorization: String,
    pub token: String,
}

impl DeviceEndpoints {
    /// Entra's v2.0 endpoints: `<authority>/<tenant>/oauth2/v2.0/devicecode` and `…/token` (also
    /// Entra Local's).
    pub fn entra(authority: &str, tenant: &str) -> Self {
        let base = format!("{}/{tenant}/oauth2/v2.0", authority.trim_end_matches('/'));
        Self {
            device_authorization: format!("{base}/devicecode"),
            token: format!("{base}/token"),
        }
    }
}

/// The answer of the device authorization endpoint: what the person must do.
#[derive(Clone)]
pub struct DeviceCode {
    device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    /// The server's own instruction text, if it sends one.
    pub message: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
}

impl std::fmt::Debug for DeviceCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceCode")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

/// The tokens of a sign-in or a refresh.
#[derive(Clone)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Seconds the access token is valid from now.
    pub expires_in: u64,
}

impl std::fmt::Debug for OAuthTokens {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OAuthTokens")
            .field("refresh_token", &self.refresh_token.is_some())
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

/// A public client that signs a person in with a device code.
pub struct DeviceLogin {
    http: reqwest::Client,
    endpoints: DeviceEndpoints,
    client_id: String,
    scope: String,
    slow_down_step: Duration,
}

/// One answer of the token endpoint while waiting.
enum Poll {
    Tokens(OAuthTokens),
    Pending,
    SlowDown,
}

impl DeviceLogin {
    /// `scope` is sent as it is (space separated, e.g. `api://…/access_as_user offline_access`).
    pub fn new(endpoints: DeviceEndpoints, client_id: &str, scope: &str) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| format!("cannot build the HTTP client: {error}"))?;
        Ok(Self {
            http,
            endpoints,
            client_id: client_id.to_owned(),
            scope: scope.to_owned(),
            slow_down_step: SLOW_DOWN_STEP,
        })
    }

    /// How much longer to wait after `slow_down` (tests make it short).
    pub fn with_slow_down_step(mut self, step: Duration) -> Self {
        self.slow_down_step = step;
        self
    }

    async fn post(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, Value), String> {
        let response = self
            .http
            .post(url)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("accept", "application/json")
            .body(form_encode(form))
            .send()
            .await
            .map_err(|error| format!("{url} cannot be reached: {}", without_url(&error)))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|error| format!("{url}: {}", without_url(&error)))?;
        let value = serde_json::from_str(&body)
            .map_err(|_| format!("{url} answered HTTP {status} without JSON"))?;
        Ok((status, value))
    }

    /// Asks for a device code (step 1): the person then opens `verification_uri` and enters
    /// `user_code`.
    pub async fn start(&self) -> Result<DeviceCode, String> {
        let (status, body) = self
            .post(
                &self.endpoints.device_authorization,
                &[("client_id", &self.client_id), ("scope", &self.scope)],
            )
            .await?;
        if status != 200 {
            return Err(format!(
                "the device authorization was refused (HTTP {status}): {}",
                oauth_error(&body)
            ));
        }
        let text = |name: &str| body[name].as_str().map(str::to_owned);
        Ok(DeviceCode {
            device_code: text("device_code").ok_or("the answer has no device_code")?,
            user_code: text("user_code").ok_or("the answer has no user_code")?,
            verification_uri: text("verification_uri")
                // Older Entra v1 endpoints call it `verification_url`.
                .or_else(|| text("verification_url"))
                .ok_or("the answer has no verification_uri")?,
            verification_uri_complete: text("verification_uri_complete"),
            message: text("message"),
            expires_in: number(&body["expires_in"]).unwrap_or(900),
            interval: number(&body["interval"]).unwrap_or(DEFAULT_INTERVAL),
        })
    }

    async fn poll(&self, code: &DeviceCode) -> Result<Poll, String> {
        let (status, body) = self
            .post(
                &self.endpoints.token,
                &[
                    ("grant_type", DEVICE_CODE_GRANT),
                    ("client_id", &self.client_id),
                    ("device_code", &code.device_code),
                ],
            )
            .await?;
        if status == 200 {
            return tokens(&body).map(Poll::Tokens);
        }
        match body["error"].as_str() {
            Some("authorization_pending") => Ok(Poll::Pending),
            Some("slow_down") => Ok(Poll::SlowDown),
            Some("expired_token") => {
                Err("the device code has expired before the sign-in: run the login again".into())
            }
            Some("access_denied") => Err("the sign-in was declined".into()),
            _ => Err(format!(
                "the sign-in failed (HTTP {status}): {}",
                oauth_error(&body)
            )),
        }
    }

    /// Polls the token endpoint (step 2) every `interval` seconds until the person has signed in,
    /// declined, or the code expired; `slow_down` makes the interval longer.
    pub async fn wait(&self, code: &DeviceCode) -> Result<OAuthTokens, String> {
        let deadline = Instant::now() + Duration::from_secs(code.expires_in);
        let mut interval = Duration::from_secs(code.interval);
        loop {
            match self.poll(code).await? {
                Poll::Tokens(tokens) => return Ok(tokens),
                Poll::Pending => {}
                Poll::SlowDown => interval += self.slow_down_step,
            }
            if Instant::now() + interval > deadline {
                return Err(
                    "the device code has expired before the sign-in: run the login again".into(),
                );
            }
            tokio::time::sleep(interval).await;
        }
    }

    /// New tokens for a refresh token (the server may rotate the refresh token).
    pub async fn refresh(&self, refresh_token: &str) -> Result<OAuthTokens, String> {
        let (status, body) = self
            .post(
                &self.endpoints.token,
                &[
                    ("grant_type", "refresh_token"),
                    ("client_id", &self.client_id),
                    ("refresh_token", refresh_token),
                    ("scope", &self.scope),
                ],
            )
            .await?;
        if status != 200 {
            return Err(format!(
                "the refresh was refused (HTTP {status}): {}",
                oauth_error(&body)
            ));
        }
        let mut tokens = tokens(&body)?;
        if tokens.refresh_token.is_none() {
            // The old one stays valid when the server does not rotate it.
            tokens.refresh_token = Some(refresh_token.to_owned());
        }
        Ok(tokens)
    }
}

fn number(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

pub(crate) fn tokens(body: &Value) -> Result<OAuthTokens, String> {
    Ok(OAuthTokens {
        access_token: body["access_token"]
            .as_str()
            .ok_or("the token answer has no access_token")?
            .to_owned(),
        refresh_token: body["refresh_token"].as_str().map(str::to_owned),
        expires_in: number(&body["expires_in"]).unwrap_or(3600),
    })
}

/// `error: description` of an OAuth error answer (never a token: these fields hold none).
pub(crate) fn oauth_error(body: &Value) -> String {
    match (body["error"].as_str(), body["error_description"].as_str()) {
        (Some(error), Some(description)) => {
            format!("{error}: {}", description.lines().next().unwrap_or(""))
        }
        (Some(error), None) => error.to_owned(),
        _ => "no error code".to_owned(),
    }
}

/// A reqwest error without its URL: a URL never holds a token here, but the form body does, and
/// reqwest never prints the body; this only keeps the message short.
pub(crate) fn without_url(error: &reqwest::Error) -> String {
    let mut text = error.to_string();
    if let Some(url) = error.url() {
        text = text.replace(&format!(" for url ({url})"), "");
    }
    text
}

/// `application/x-www-form-urlencoded`.
pub(crate) fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(name, value)| format!("{}={}", encode(name), encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'*' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_are_url_encoded() {
        assert_eq!(
            form_encode(&[("scope", "api://x/a b"), ("grant_type", DEVICE_CODE_GRANT)]),
            "scope=api%3A%2F%2Fx%2Fa+b&grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"
        );
    }

    #[test]
    fn entra_endpoints_follow_the_tenant() {
        let endpoints = DeviceEndpoints::entra("https://login.example.com/", "t1");
        assert_eq!(
            endpoints.device_authorization,
            "https://login.example.com/t1/oauth2/v2.0/devicecode"
        );
        assert_eq!(
            endpoints.token,
            "https://login.example.com/t1/oauth2/v2.0/token"
        );
    }
}
