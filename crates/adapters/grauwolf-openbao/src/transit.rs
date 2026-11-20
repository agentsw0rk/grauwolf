//! The Transit secrets engine (ADR-0111): signing with a key that never leaves OpenBao, and its
//! public keys. The calls take the token explicitly, because the signer logs in again when its
//! token runs out while the `Bao` stays the same.

use std::future::Future;
use std::pin::Pin;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde_json::{Value, json};

use crate::BaoError;
use crate::admin::Bao;
use crate::transport::{Method, Request, Transport};

/// Gives a valid OpenBao token, logging in when needed (e.g. with a JWT-SVID).
pub trait TokenSource: Send + Sync {
    /// A token for the next call. A credential: never log it.
    fn token(&self) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>>;
}

/// A fixed token (tests, a token from the environment).
pub struct StaticToken(pub String);

impl TokenSource for StaticToken {
    fn token(&self) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
        Box::pin(async move { Ok(self.0.clone()) })
    }
}

/// The public side of a Transit key of type `ecdsa-p256`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitKey {
    /// The version that signs by default.
    pub latest_version: u64,
    /// `(version, uncompressed P-256 point 04 || x || y)` of every version still readable.
    pub public_keys: Vec<(u64, Vec<u8>)>,
}

/// The DER prefix of a P-256 `SubjectPublicKeyInfo`; the uncompressed point follows it.
const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

/// The point of a PEM `PUBLIC KEY` of a P-256 key, else `None`.
fn p256_point(pem: &str) -> Option<Vec<u8>> {
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .map(str::trim)
        .collect();
    let der = STANDARD.decode(body).ok()?;
    let point = der.strip_prefix(&P256_SPKI_PREFIX)?;
    (point.len() == 65 && point[0] == 4).then(|| point.to_vec())
}

impl<T: Transport> Bao<T> {
    async fn transit_call(
        &self,
        what: &str,
        token: &str,
        method: Method,
        path: String,
        body: Option<Value>,
    ) -> Result<Value, BaoError> {
        let response = self
            .transport()
            .send(Request {
                method,
                path,
                token: Some(token.to_owned()),
                body,
            })
            .await?;
        if response.status / 100 != 2 {
            return Err(BaoError::status(what, response.status, &response.body));
        }
        serde_json::from_str(&response.body).map_err(|_| BaoError::other("the answer is not JSON"))
    }

    /// `GET /v1/<mount>/keys/<key>`: the versions and public keys of an `ecdsa-p256` key.
    pub async fn transit_key(
        &self,
        token: &str,
        mount: &str,
        key: &str,
    ) -> Result<TransitKey, BaoError> {
        let body = self
            .transit_call(
                "read transit key",
                token,
                Method::Get,
                format!("/v1/{}/keys/{key}", mount.trim_matches('/')),
                None,
            )
            .await?;
        let data = &body["data"];
        if data["type"].as_str() != Some("ecdsa-p256") {
            return Err(BaoError::other(format!(
                "transit key '{key}' is not of type ecdsa-p256"
            )));
        }
        let latest_version = data["latest_version"]
            .as_u64()
            .ok_or_else(|| BaoError::other("the transit key has no latest_version"))?;
        let mut public_keys = Vec::new();
        for (version, entry) in data["keys"].as_object().into_iter().flatten() {
            let version: u64 = version
                .parse()
                .map_err(|_| BaoError::other("a transit key version is not a number"))?;
            let point = entry["public_key"]
                .as_str()
                .and_then(p256_point)
                .ok_or_else(|| {
                    BaoError::other(format!(
                        "version {version} of transit key '{key}' has no P-256 public key"
                    ))
                })?;
            public_keys.push((version, point));
        }
        public_keys.sort();
        Ok(TransitKey {
            latest_version,
            public_keys,
        })
    }

    /// `POST /v1/<mount>/sign/<key>` with SHA-256 and JWS marshaling: the raw ES256 signature
    /// (`r || s`, 64 bytes) of `input` with key version `version`.
    pub async fn transit_sign(
        &self,
        token: &str,
        mount: &str,
        key: &str,
        version: u64,
        input: &[u8],
    ) -> Result<Vec<u8>, BaoError> {
        let body = self
            .transit_call(
                "transit sign",
                token,
                Method::Post,
                format!("/v1/{}/sign/{key}", mount.trim_matches('/')),
                Some(json!({
                    "input": STANDARD.encode(input),
                    "hash_algorithm": "sha2-256",
                    "marshaling_algorithm": "jws",
                    "key_version": version,
                })),
            )
            .await?;
        let signature = body
            .pointer("/data/signature")
            .and_then(Value::as_str)
            .ok_or_else(|| BaoError::other("the sign answer has no signature"))?;
        // `vault:v<version>:<base64url signature>`.
        let encoded = signature
            .rsplit(':')
            .next()
            .ok_or_else(|| BaoError::other("the signature has an unknown format"))?;
        let raw = URL_SAFE_NO_PAD
            .decode(encoded.trim_end_matches('='))
            .map_err(|_| BaoError::other("the signature is not base64url"))?;
        if raw.len() != 64 {
            return Err(BaoError::other(
                "the signature is not a 64-byte ES256 signature",
            ));
        }
        Ok(raw)
    }
}
