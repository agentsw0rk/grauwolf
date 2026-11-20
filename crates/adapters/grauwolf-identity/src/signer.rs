//! Signing of Txn-Tokens behind a port: an in-memory key (tests, local trials) and OpenBao
//! Transit (the key never leaves OpenBao; ADR-0111 Q4).

use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use futures_util::future::BoxFuture;
use grauwolf_openbao::{Bao, TokenSource, TransitKey, Transport};
use tokio::sync::Mutex;

use crate::IdentityError;
use crate::jose::Jwk;

/// Signs Txn-Tokens with ES256 and publishes the public keys.
pub trait TokenSigner: Send + Sync {
    /// The `kid` of the key that signs now.
    fn key_id(&self) -> BoxFuture<'_, Result<String, IdentityError>>;
    /// The raw ES256 signature (`r || s`, 64 bytes) of `input` with key `kid`.
    fn sign<'a>(
        &'a self,
        kid: &'a str,
        input: &'a [u8],
    ) -> BoxFuture<'a, Result<Vec<u8>, IdentityError>>;
    /// The public keys for `jwks.json` (the current one and older ones still in use).
    fn public_keys(&self) -> BoxFuture<'_, Result<Vec<Jwk>, IdentityError>>;
}

/// A P-256 key made in memory: gone with the process. For tests and local trials.
pub struct MemorySigner {
    kid: String,
    pair: EcdsaKeyPair,
}

impl MemorySigner {
    /// A fresh key named `kid`.
    pub fn generate(kid: &str) -> Self {
        Self {
            kid: kid.to_owned(),
            pair: EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING)
                .expect("P-256 key generation"),
        }
    }

    /// The public key as a JWK.
    pub fn jwk(&self) -> Jwk {
        Jwk::p256(&self.kid, self.pair.public_key().as_ref()).expect("an uncompressed P-256 point")
    }
}

impl TokenSigner for MemorySigner {
    fn key_id(&self) -> BoxFuture<'_, Result<String, IdentityError>> {
        Box::pin(async move { Ok(self.kid.clone()) })
    }

    fn sign<'a>(
        &'a self,
        kid: &'a str,
        input: &'a [u8],
    ) -> BoxFuture<'a, Result<Vec<u8>, IdentityError>> {
        Box::pin(async move {
            if kid != self.kid {
                return Err(IdentityError::Unavailable(format!(
                    "no signing key '{kid}'"
                )));
            }
            self.pair
                .sign(&SystemRandom::new(), input)
                .map(|signature| signature.as_ref().to_vec())
                .map_err(|_| IdentityError::Unavailable("signing failed".into()))
        })
    }

    fn public_keys(&self) -> BoxFuture<'_, Result<Vec<Jwk>, IdentityError>> {
        Box::pin(async move { Ok(vec![self.jwk()]) })
    }
}

/// How long the key versions read from Transit are reused before they are read again.
pub const TRANSIT_KEYS_TTL: Duration = Duration::from_secs(60);

/// A Transit key of type `ecdsa-p256` in OpenBao: `transit/sign/<key>` signs (SHA-256, JWS
/// marshaling, so the signature is already `r || s`), `transit/keys/<key>` gives the public keys
/// of all versions. The `kid` is `<key>-v<version>`; the latest version signs, older versions
/// stay in `jwks.json` for tokens that are still valid (rotation with overlap).
pub struct TransitSigner<T> {
    bao: Bao<T>,
    tokens: Arc<dyn TokenSource>,
    mount: String,
    key: String,
    cached: Mutex<Option<(Instant, TransitKey)>>,
}

impl<T: Transport> TransitSigner<T> {
    /// Signs with `key` in the Transit engine at `mount`; OpenBao tokens from `tokens`.
    pub fn new(bao: Bao<T>, tokens: Arc<dyn TokenSource>, mount: &str, key: &str) -> Self {
        Self {
            bao,
            tokens,
            mount: mount.to_owned(),
            key: key.to_owned(),
            cached: Mutex::new(None),
        }
    }

    fn kid(&self, version: u64) -> String {
        format!("{}-v{version}", self.key)
    }

    fn version_of(&self, kid: &str) -> Option<u64> {
        kid.strip_prefix(&format!("{}-v", self.key))?.parse().ok()
    }

    async fn token(&self) -> Result<String, IdentityError> {
        self.tokens
            .token()
            .await
            .map_err(|error| IdentityError::Unavailable(format!("OpenBao login: {error}")))
    }

    async fn keys(&self) -> Result<TransitKey, IdentityError> {
        let mut cached = self.cached.lock().await;
        if let Some((at, key)) = cached.as_ref()
            && at.elapsed() < TRANSIT_KEYS_TTL
        {
            return Ok(key.clone());
        }
        let token = self.token().await?;
        let key = self
            .bao
            .transit_key(&token, &self.mount, &self.key)
            .await
            .map_err(|error| IdentityError::Unavailable(format!("OpenBao Transit: {error}")))?;
        *cached = Some((Instant::now(), key.clone()));
        Ok(key)
    }
}

impl<T: Transport> TokenSigner for TransitSigner<T> {
    fn key_id(&self) -> BoxFuture<'_, Result<String, IdentityError>> {
        Box::pin(async move { Ok(self.kid(self.keys().await?.latest_version)) })
    }

    fn sign<'a>(
        &'a self,
        kid: &'a str,
        input: &'a [u8],
    ) -> BoxFuture<'a, Result<Vec<u8>, IdentityError>> {
        Box::pin(async move {
            let version = self
                .version_of(kid)
                .ok_or_else(|| IdentityError::Unavailable(format!("no signing key '{kid}'")))?;
            let token = self.token().await?;
            self.bao
                .transit_sign(&token, &self.mount, &self.key, version, input)
                .await
                .map_err(|error| IdentityError::Unavailable(format!("OpenBao Transit: {error}")))
        })
    }

    fn public_keys(&self) -> BoxFuture<'_, Result<Vec<Jwk>, IdentityError>> {
        Box::pin(async move {
            let key = self.keys().await?;
            key.public_keys
                .iter()
                .map(|(version, point)| {
                    Jwk::p256(&self.kid(*version), point).ok_or_else(|| {
                        IdentityError::Unavailable(format!(
                            "Transit key '{}' version {version} is not ecdsa-p256",
                            self.key
                        ))
                    })
                })
                .collect()
        })
    }
}
