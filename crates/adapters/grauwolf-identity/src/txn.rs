//! The Txn-Token (ADR-0111): claims, issuing through a [`TokenSigner`] and verification.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::jose::{self, Jwk, b64};
use crate::keys::{JwksLocation, KeyCache};
use crate::{IdentityError, TokenSigner};

/// The `typ` of the JOSE header (from the transaction-token draft).
pub const TXN_TOKEN_TYP: &str = "txntoken+jwt";
/// The token type of a Txn-Token in a token request or response (RFC 8693 style).
pub const TXN_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:txn_token";
/// The token type of an access token (the Entra token at the entry).
pub const ACCESS_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:access_token";
/// Lifetime of one Txn-Token.
pub const DEFAULT_LIFETIME: Duration = Duration::from_secs(15 * 60);
/// How long a chain can be renewed at most, from its entry.
pub const MAX_CHAIN_LIFETIME: Duration = Duration::from_secs(8 * 60 * 60);
/// Tolerance for clocks that are a little apart.
pub const LEEWAY_SECS: u64 = 30;

/// The claims of a Txn-Token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TxnClaims {
    /// The identity service (its public URL).
    pub iss: String,
    /// The trust domain the token is valid in.
    pub aud: String,
    /// The person: their Entra object ID.
    pub sub: String,
    /// Display name, for events and spans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// User principal name, for events and spans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upn: Option<String>,
    /// The ID of the chain; kept by renewals.
    pub txn: String,
    /// The purpose the entry asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purp: Option<String>,
    /// Authorization details (grant ID, allowed targets; from stage 3 on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azd: Option<Value>,
    /// The SPIFFE ID of the workload that requested the chain at the entry.
    pub req_wl: String,
    pub iat: u64,
    pub exp: u64,
    /// The end of the chain's maximum lifetime: no renewal goes beyond it (a grauwolf claim,
    /// not in the draft).
    pub chain_exp: u64,
}

/// Signs `claims` as a Txn-Token (`ES256`, `typ` [`TXN_TOKEN_TYP`], `kid` of the signer).
pub async fn issue(signer: &dyn TokenSigner, claims: &TxnClaims) -> Result<String, IdentityError> {
    let kid = signer.key_id().await?;
    let header = json!({ "alg": "ES256", "typ": TXN_TOKEN_TYP, "kid": kid });
    let input = format!(
        "{}.{}",
        b64(header.to_string().as_bytes()),
        b64(serde_json::to_string(claims)
            .map_err(|error| IdentityError::Unavailable(error.to_string()))?
            .as_bytes())
    );
    let signature = signer.sign(&kid, input.as_bytes()).await?;
    Ok(format!("{input}.{}", b64(&signature)))
}

/// The `kid` a token names, for looking up its key.
pub(crate) fn kid_of(token: &str) -> Result<Option<String>, IdentityError> {
    jose::parse(token)
        .map(|compact| compact.kid().map(str::to_owned))
        .ok_or_else(|| IdentityError::Unauthorized("the Txn-Token is not a JWT".into()))
}

/// Checks a Txn-Token against `key`: signature (ES256 only), `typ`, `aud` = `audience`, `exp`
/// (with [`LEEWAY_SECS`]); with `issuer` also `iss`.
pub fn check(
    token: &str,
    key: &Jwk,
    audience: &str,
    issuer: Option<&str>,
    now: u64,
) -> Result<TxnClaims, IdentityError> {
    let invalid = |reason: &str| IdentityError::Unauthorized(format!("Txn-Token {reason}"));
    let compact = jose::parse(token).ok_or_else(|| invalid("is not a JWT"))?;
    if compact.alg() != "ES256" || compact.header["typ"] != TXN_TOKEN_TYP {
        return Err(invalid("has the wrong algorithm or type"));
    }
    if !key.verifies(
        "ES256",
        compact.signing_input.as_bytes(),
        &compact.signature,
    ) {
        return Err(invalid("has an invalid signature"));
    }
    let claims: TxnClaims =
        serde_json::from_value(compact.claims).map_err(|_| invalid("lacks required claims"))?;
    if claims.aud != audience {
        return Err(invalid("is for another trust domain"));
    }
    if issuer.is_some_and(|issuer| claims.iss != issuer) {
        return Err(invalid("is from another issuer"));
    }
    if claims.exp + LEEWAY_SECS <= now {
        return Err(invalid("has expired"));
    }
    Ok(claims)
}

/// Verifies Txn-Tokens with the keys of an identity service (agents, ADR-0111): signature from
/// the cached `jwks.json`, `aud` = the own trust domain, `exp`.
pub struct TxnTokenVerifier {
    keys: KeyCache,
    audience: String,
}

impl TxnTokenVerifier {
    /// Keys from `jwks_url`; tokens must name `trust_domain` as `aud`.
    pub fn new(http: reqwest::Client, jwks_url: &str, trust_domain: &str) -> Self {
        Self {
            keys: KeyCache::new(http, JwksLocation::Url(jwks_url.to_owned())),
            audience: trust_domain.to_owned(),
        }
    }

    /// The claims of a valid token; `Unauthorized` with the reason (never the token), or
    /// `Unavailable` when the keys cannot be fetched.
    pub async fn verify(&self, token: &str) -> Result<TxnClaims, IdentityError> {
        let kid = kid_of(token)?;
        let key = self.keys.key(kid.as_deref()).await?;
        check(token, &key, &self.audience, None, jose::now_secs())
    }
}
