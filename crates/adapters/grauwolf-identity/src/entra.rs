//! People's access tokens from Entra ID (or Entra Local) behind a port, so the Txn-Token side
//! does not depend on Entra (ADR-0111).

use futures_util::future::BoxFuture;
use serde_json::Value;

use crate::IdentityError;
use crate::jose::{self, now_secs};
use crate::keys::{JwksLocation, KeyCache};
use crate::txn::LEEWAY_SECS;

/// A person, as a verified access token names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// The object ID (`oid`), stable per person and tenant.
    pub oid: String,
    pub name: Option<String>,
    /// `upn`, else `preferred_username`.
    pub upn: Option<String>,
    /// The tenant (`tid`).
    pub tid: String,
}

/// Verifies a person's access token.
pub trait PersonTokenVerifier: Send + Sync {
    /// The person of a valid token; `Unauthorized` with the reason (never the token), or
    /// `Unavailable` when the issuer's keys cannot be fetched.
    fn verify<'a>(&'a self, token: &'a str) -> BoxFuture<'a, Result<Person, IdentityError>>;
}

/// What an Entra access token must name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntraSettings {
    /// The tenant ID (`tid`).
    pub tenant: String,
    /// The issuer (`iss`), e.g. `https://login.microsoftonline.com/<tenant>/v2.0`.
    pub issuer: String,
    /// The accepted audiences (`aud`): the grauwolf identity API as its application ID (v2.0
    /// tokens) and as its application ID URI (v1.0 tokens, Entra Local's token generator). A token
    /// is accepted when its `aud` is any of them.
    pub audiences: Vec<String>,
    /// The scope that must be in `scp`, e.g. `access_as_user`.
    pub scope: String,
    /// The JWKS URL; `None`: from the issuer's OpenID discovery.
    pub jwks_url: Option<String>,
}

impl EntraSettings {
    /// `aud` is one of the accepted audiences.
    fn accepts(&self, aud: &str) -> bool {
        self.audiences.iter().any(|accepted| accepted == aud)
    }
}

/// Entra access tokens: RS256 with a key from the tenant's JWKS (cached, fetched again for an
/// unknown `kid`), then `tid`, `iss`, `aud`, `exp`/`nbf` and `scp`.
pub struct EntraVerifier {
    settings: EntraSettings,
    keys: KeyCache,
}

impl EntraVerifier {
    pub fn new(http: reqwest::Client, settings: EntraSettings) -> Self {
        let location = match &settings.jwks_url {
            Some(url) => JwksLocation::Url(url.clone()),
            None => JwksLocation::Discovery(settings.issuer.clone()),
        };
        Self {
            keys: KeyCache::new(http, location),
            settings,
        }
    }

    /// The same with an HTTP client of its own (10 s timeout).
    pub fn with_settings(settings: EntraSettings) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self::new(http, settings))
    }

    async fn check(&self, token: &str) -> Result<Person, IdentityError> {
        let invalid = |reason: &str| IdentityError::Unauthorized(format!("access token {reason}"));
        let compact = jose::parse(token).ok_or_else(|| invalid("is not a JWT"))?;
        if compact.alg() != "RS256" {
            return Err(invalid("is not signed with RS256"));
        }
        let key = self.keys.key(compact.kid()).await?;
        if !key.verifies(
            "RS256",
            compact.signing_input.as_bytes(),
            &compact.signature,
        ) {
            return Err(invalid("has an invalid signature"));
        }
        let claims = &compact.claims;
        let text = |name: &str| claims[name].as_str().map(str::to_owned);
        let settings = &self.settings;
        if text("tid").as_deref() != Some(settings.tenant.as_str()) {
            return Err(invalid("is from another tenant"));
        }
        if text("iss").as_deref() != Some(settings.issuer.as_str()) {
            return Err(invalid("is from another issuer"));
        }
        let audience_matches = match &claims["aud"] {
            Value::String(aud) => settings.accepts(aud),
            Value::Array(auds) => auds
                .iter()
                .any(|aud| aud.as_str().is_some_and(|aud| settings.accepts(aud))),
            _ => false,
        };
        if !audience_matches {
            return Err(invalid("is for another audience"));
        }
        let now = now_secs();
        match claims["exp"].as_u64() {
            Some(exp) if exp + LEEWAY_SECS > now => {}
            _ => return Err(invalid("has expired")),
        }
        if claims["nbf"]
            .as_u64()
            .is_some_and(|nbf| nbf > now + LEEWAY_SECS)
        {
            return Err(invalid("is not valid yet"));
        }
        let scopes = claims["scp"].as_str().unwrap_or_default();
        if !scopes.split(' ').any(|scope| scope == settings.scope) {
            return Err(invalid(&format!("lacks the scope '{}'", settings.scope)));
        }
        let oid = text("oid").ok_or_else(|| invalid("names no person (oid)"))?;
        Ok(Person {
            oid,
            name: text("name"),
            upn: text("upn").or_else(|| text("preferred_username")),
            tid: settings.tenant.clone(),
        })
    }
}

impl PersonTokenVerifier for EntraVerifier {
    fn verify<'a>(&'a self, token: &'a str) -> BoxFuture<'a, Result<Person, IdentityError>> {
        Box::pin(self.check(token))
    }
}
