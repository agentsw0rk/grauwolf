//! The small part of JOSE that delegation needs: compact JWTs, JWKs and the two signature
//! algorithms (ES256 for Txn-Tokens, RS256 for Entra access tokens), on aws-lc-rs.

use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_FIXED, RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents, UnparsedPublicKey,
};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Base64url without padding.
pub(crate) fn b64(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

/// The bytes of base64url text (padding tolerated).
pub(crate) fn unb64(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text.trim_end_matches('=')).ok()
}

/// A compact JWS, split and decoded but not verified.
pub(crate) struct Compact<'a> {
    pub header: Value,
    pub claims: Value,
    /// `<header>.<claims>`, the bytes the signature covers.
    pub signing_input: &'a str,
    pub signature: Vec<u8>,
}

impl Compact<'_> {
    /// `alg` of the header.
    pub fn alg(&self) -> &str {
        self.header["alg"].as_str().unwrap_or_default()
    }

    /// `kid` of the header.
    pub fn kid(&self) -> Option<&str> {
        self.header["kid"].as_str()
    }
}

/// Splits a compact JWS; `None` if it is not one (three parts, JSON objects).
pub(crate) fn parse(token: &str) -> Option<Compact<'_>> {
    let mut parts = token.split('.');
    let (header, claims, signature) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let header: Value = serde_json::from_slice(&unb64(header)?).ok()?;
    let claims: Value = serde_json::from_slice(&unb64(claims)?).ok()?;
    if !header.is_object() || !claims.is_object() {
        return None;
    }
    Some(Compact {
        header,
        claims,
        signing_input: &token[..token.len() - signature.len() - 1],
        signature: unb64(signature)?,
    })
}

/// The claims of a JWT without any check, to tell kinds of tokens apart (an Entra access token
/// from a JWT-SVID or a Txn-Token). Never a basis for trust.
pub fn peek_claims(token: &str) -> Option<Value> {
    parse(token).map(|compact| compact.claims)
}

/// A JSON Web Key (only the members grauwolf uses).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Jwk {
    /// `EC` or `RSA`.
    pub kty: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alg: Option<String>,
    #[serde(default, rename = "use", skip_serializing_if = "Option::is_none")]
    pub use_: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crv: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e: Option<String>,
}

/// A JWK set, as `jwks.json` serves it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Jwks {
    pub keys: Vec<Jwk>,
}

impl Jwk {
    /// An ES256 signing key from the uncompressed P-256 point (`04 || x || y`).
    pub fn p256(kid: &str, point: &[u8]) -> Option<Self> {
        if point.len() != 65 || point[0] != 4 {
            return None;
        }
        Some(Self {
            kty: "EC".into(),
            kid: Some(kid.into()),
            alg: Some("ES256".into()),
            use_: Some("sig".into()),
            crv: Some("P-256".into()),
            x: Some(b64(&point[1..33])),
            y: Some(b64(&point[33..])),
            n: None,
            e: None,
        })
    }

    /// An RS256 key from modulus and exponent (big-endian bytes).
    pub fn rsa(kid: &str, n: &[u8], e: &[u8]) -> Self {
        Self {
            kty: "RSA".into(),
            kid: Some(kid.into()),
            alg: Some("RS256".into()),
            use_: Some("sig".into()),
            crv: None,
            x: None,
            y: None,
            n: Some(b64(n)),
            e: Some(b64(e)),
        }
    }

    /// Whether `signature` over `input` is valid for this key with `alg` (`ES256` or `RS256`;
    /// anything else, including `none`, is never valid).
    pub fn verifies(&self, alg: &str, input: &[u8], signature: &[u8]) -> bool {
        if self.alg.as_deref().is_some_and(|own| own != alg) {
            return false;
        }
        let member = |value: &Option<String>| value.as_deref().and_then(unb64);
        match (alg, self.kty.as_str()) {
            ("ES256", "EC") if self.crv.as_deref() == Some("P-256") => {
                let (Some(x), Some(y)) = (member(&self.x), member(&self.y)) else {
                    return false;
                };
                if x.len() != 32 || y.len() != 32 {
                    return false;
                }
                let mut point = vec![4];
                point.extend(x);
                point.extend(y);
                UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point)
                    .verify(input, signature)
                    .is_ok()
            }
            ("RS256", "RSA") => {
                let (Some(n), Some(e)) = (member(&self.n), member(&self.e)) else {
                    return false;
                };
                RsaPublicKeyComponents { n, e }
                    .verify(&RSA_PKCS1_2048_8192_SHA256, input, signature)
                    .is_ok()
            }
            _ => false,
        }
    }
}

/// The current time in seconds since the epoch.
pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}
