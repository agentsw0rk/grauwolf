//! Test helpers (feature `testing`): an Entra stand-in that issues RS256 access tokens with a
//! key made at test time, and a person verifier that needs no keys.

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::rsa::KeySize;
use aws_lc_rs::signature::{KeyPair, RSA_PKCS1_SHA256, RsaKeyPair};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use serde_json::{Value, json};

use crate::IdentityError;
use crate::entra::{EntraSettings, Person, PersonTokenVerifier};
use crate::jose::{Jwk, Jwks, b64, now_secs, peek_claims};
use crate::refresh::{Redeemed, RefreshRedeemer};

/// The tenant of [`FakeEntra`] tokens.
pub const TENANT: &str = "11111111-1111-1111-1111-111111111111";
/// The audience of [`FakeEntra`] tokens: the grauwolf identity API.
pub const AUDIENCE: &str = "api://grauwolf-identity";
/// The scope of [`FakeEntra`] tokens.
pub const SCOPE: &str = "access_as_user";

/// An Entra issuer for tests: RSA keys made at test time, tokens for people, and refresh tokens
/// that rotate on every redemption and stop working for a disabled person.
pub struct FakeEntra {
    issuer: String,
    keys: Vec<(String, Arc<RsaKeyPair>)>,
    book: Arc<Mutex<RefreshBook>>,
}

/// The refresh tokens that are valid now, and the disabled people.
#[derive(Default)]
struct RefreshBook {
    tokens: HashMap<String, (String, String)>,
    disabled: HashSet<String>,
    issued: u64,
}

impl RefreshBook {
    fn issue(&mut self, oid: &str, name: &str) -> String {
        self.issued += 1;
        let token = format!("rt-{oid}-{}", self.issued);
        self.tokens
            .insert(token.clone(), (oid.to_owned(), name.to_owned()));
        token
    }
}

impl FakeEntra {
    /// An issuer named `issuer` with one key `k1`.
    pub fn new(issuer: &str) -> Self {
        let mut fake = Self {
            issuer: issuer.to_owned(),
            keys: Vec::new(),
            book: Arc::default(),
        };
        fake.rotate("k1");
        fake
    }

    /// The issuer URL.
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Adds a key `kid` that signs from now on (the old ones stay in the JWKS).
    pub fn rotate(&mut self, kid: &str) {
        let pair = RsaKeyPair::generate(KeySize::Rsa2048).expect("RSA key generation");
        self.keys.push((kid.to_owned(), Arc::new(pair)));
    }

    /// A new refresh token of `oid` (as a sign-in with `offline_access` returns it).
    pub fn refresh_token(&self, oid: &str, name: &str) -> String {
        self.book.lock().unwrap().issue(oid, name)
    }

    /// Disables the person `oid`: their refresh tokens are refused from now on.
    pub fn disable(&self, oid: &str) {
        self.book.lock().unwrap().disabled.insert(oid.to_owned());
    }

    /// Whether `token` is a refresh token that is valid now.
    pub fn is_valid_refresh_token(&self, token: &str) -> bool {
        self.book.lock().unwrap().tokens.contains_key(token)
    }

    /// Redeems this issuer's refresh tokens like Entra's token endpoint: a valid token gives an
    /// access token (signed with the latest key now) and a new refresh token, and the old one
    /// stops working; an unknown token or one of a disabled person is `invalid_grant`.
    pub fn redeemer(&self) -> Arc<dyn RefreshRedeemer> {
        let (kid, pair) = self.keys.last().expect("a key");
        Arc::new(FakeRefresh {
            issuer: self.issuer.clone(),
            kid: kid.clone(),
            pair: Arc::clone(pair),
            book: Arc::clone(&self.book),
        })
    }

    /// The settings an identity service needs to accept these tokens, keys from `jwks_url`.
    pub fn settings(&self, jwks_url: &str) -> EntraSettings {
        EntraSettings {
            tenant: TENANT.into(),
            issuer: self.issuer.clone(),
            audiences: vec![AUDIENCE.into()],
            scope: SCOPE.into(),
            jwks_url: Some(jwks_url.to_owned()),
        }
    }

    /// The public keys.
    pub fn jwks(&self) -> Value {
        let keys: Vec<Jwk> = self
            .keys
            .iter()
            .map(|(kid, pair)| {
                let (n, e) = rsa_components(pair.public_key().as_ref());
                Jwk::rsa(kid, n, e)
            })
            .collect();
        serde_json::to_value(Jwks { keys }).expect("JWKS as JSON")
    }

    /// The claims of a valid token for `oid` (one hour, the right tenant, audience and scope).
    pub fn claims(&self, oid: &str, name: &str) -> Value {
        person_claims(&self.issuer, oid, name)
    }

    /// A valid token for `oid`.
    pub fn token(&self, oid: &str, name: &str) -> String {
        self.sign(&self.claims(oid, name))
    }

    /// `claims` signed with the latest key.
    pub fn sign(&self, claims: &Value) -> String {
        let (kid, pair) = self.keys.last().expect("a key");
        sign_with(kid, pair, claims)
    }
}

fn person_claims(issuer: &str, oid: &str, name: &str) -> Value {
    let now = now_secs();
    json!({
        "iss": issuer, "aud": AUDIENCE, "tid": TENANT, "oid": oid, "sub": oid,
        "name": name, "preferred_username": format!("{name}@example.org").to_lowercase(),
        "scp": SCOPE, "iat": now, "nbf": now, "exp": now + 3600, "ver": "2.0"
    })
}

/// The redeemer of [`FakeEntra::redeemer`].
struct FakeRefresh {
    issuer: String,
    kid: String,
    pair: Arc<RsaKeyPair>,
    book: Arc<Mutex<RefreshBook>>,
}

impl RefreshRedeemer for FakeRefresh {
    fn redeem<'a>(
        &'a self,
        refresh_token: &'a str,
    ) -> BoxFuture<'a, Result<Redeemed, IdentityError>> {
        Box::pin(async move {
            let mut book = self.book.lock().unwrap();
            let refused = |reason: &str| {
                IdentityError::Unauthorized(format!(
                    "the refresh token was refused (HTTP 400): invalid_grant: {reason}"
                ))
            };
            let (oid, name) = book
                .tokens
                .get(refresh_token)
                .cloned()
                .ok_or_else(|| refused("unknown or used token"))?;
            if book.disabled.contains(&oid) {
                return Err(refused("the user is disabled"));
            }
            book.tokens.remove(refresh_token);
            let refresh_token = book.issue(&oid, &name);
            Ok(Redeemed {
                access_token: sign_with(
                    &self.kid,
                    &self.pair,
                    &person_claims(&self.issuer, &oid, &name),
                ),
                refresh_token,
            })
        })
    }
}

fn sign_with(kid: &str, pair: &RsaKeyPair, claims: &Value) -> String {
    let header = json!({ "alg": "RS256", "typ": "JWT", "kid": kid });
    let input = format!(
        "{}.{}",
        b64(header.to_string().as_bytes()),
        b64(claims.to_string().as_bytes())
    );
    let mut signature = vec![0; pair.public_modulus_len()];
    pair.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        input.as_bytes(),
        &mut signature,
    )
    .expect("RSA signing");
    format!("{input}.{}", b64(&signature))
}

/// Modulus and exponent of a PKCS#1 `RSAPublicKey` (`SEQUENCE { INTEGER n, INTEGER e }`), without
/// leading zero bytes.
fn rsa_components(der: &[u8]) -> (&[u8], &[u8]) {
    /// Tag, then the content of one DER element and the rest.
    fn element(input: &[u8]) -> (&[u8], &[u8]) {
        let (first, rest) = (input[1], &input[2..]);
        let (len, rest) = if first < 0x80 {
            (first as usize, rest)
        } else {
            let count = (first & 0x7f) as usize;
            let len = rest[..count]
                .iter()
                .fold(0_usize, |len, byte| (len << 8) | *byte as usize);
            (len, &rest[count..])
        };
        rest.split_at(len)
    }
    let (sequence, _) = element(der);
    let (n, rest) = element(sequence);
    let (e, _) = element(rest);
    let strip = |value: &[u8]| -> usize { value.iter().take_while(|byte| **byte == 0).count() };
    (&n[strip(n)..], &e[strip(e)..])
}

/// Accepts every JWT and takes the person from its unverified claims (`oid`, `name`, `upn`);
/// refuses tokens with `"bad": true`. For tests of the parts around the verification.
pub struct TrustingPeople;

impl PersonTokenVerifier for TrustingPeople {
    fn verify<'a>(&'a self, token: &'a str) -> BoxFuture<'a, Result<Person, IdentityError>> {
        Box::pin(async move {
            let claims = peek_claims(token)
                .filter(|claims| claims["bad"] != true)
                .ok_or_else(|| IdentityError::Unauthorized("access token is not valid".into()))?;
            let text = |name: &str| claims[name].as_str().map(str::to_owned);
            Ok(Person {
                oid: text("oid").ok_or_else(|| IdentityError::Unauthorized("no oid".into()))?,
                name: text("name"),
                upn: text("upn"),
                tid: TENANT.into(),
            })
        })
    }
}

/// An OpenBao KV v2 stand-in for [`crate::OpenBaoGrantStore`]: `data/` read and write,
/// `metadata/` read, list and delete, in memory. [`FakeKv::set_down`] makes every call fail as a
/// transport error (OpenBao unreachable).
#[derive(Clone, Default)]
pub struct FakeKv {
    entries: std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, Value>>>,
    down: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tokens_seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl FakeKv {
    pub fn new() -> Self {
        Self::default()
    }

    /// The stored data of `path` below the mount (e.g. `identity/grants/grt_…`).
    pub fn data(&self, path: &str) -> Option<Value> {
        self.entries.lock().unwrap().get(path).cloned()
    }

    /// Every stored path.
    pub fn paths(&self) -> Vec<String> {
        self.entries.lock().unwrap().keys().cloned().collect()
    }

    /// Makes OpenBao unreachable (`true`) or reachable again.
    pub fn set_down(&self, down: bool) {
        self.down.store(down, std::sync::atomic::Ordering::SeqCst);
    }

    /// The OpenBao tokens the calls carried.
    pub fn tokens_seen(&self) -> Vec<String> {
        self.tokens_seen.lock().unwrap().clone()
    }

    fn answer(&self, request: grauwolf_openbao::Request) -> grauwolf_openbao::Response {
        use grauwolf_openbao::{Method, Response};
        let reply = |status: u16, body: Value| Response {
            status,
            body: body.to_string(),
        };
        self.tokens_seen
            .lock()
            .unwrap()
            .push(request.token.clone().unwrap_or_default());
        let (path, list) = match request.path.split_once('?') {
            Some((path, query)) => (path.to_owned(), query == "list=true"),
            None => (request.path.clone(), false),
        };
        // `/v1/<mount>/<data|metadata>/<path>`
        let mut parts = path.trim_start_matches("/v1/").splitn(3, '/');
        let (_mount, kind, rest) = (
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default().to_owned(),
        );
        let mut entries = self.entries.lock().unwrap();
        match (request.method, kind) {
            (Method::Get, "data") => match entries.get(&rest) {
                Some(data) => reply(200, json!({ "data": { "data": data } })),
                None => reply(404, json!({ "errors": [] })),
            },
            (Method::Post | Method::Put, "data") => {
                let data = request.body.unwrap_or_default()["data"].clone();
                entries.insert(rest, data);
                reply(200, json!({ "data": { "version": 1 } }))
            }
            (Method::Get, "metadata") if list => {
                let prefix = format!("{}/", rest.trim_end_matches('/'));
                let mut keys: Vec<String> = entries
                    .keys()
                    .filter_map(|key| key.strip_prefix(&prefix))
                    .map(|name| match name.split_once('/') {
                        Some((folder, _)) => format!("{folder}/"),
                        None => name.to_owned(),
                    })
                    .collect();
                keys.dedup();
                if keys.is_empty() {
                    reply(404, json!({ "errors": [] }))
                } else {
                    reply(200, json!({ "data": { "keys": keys } }))
                }
            }
            (Method::Get, "metadata") => match entries.contains_key(&rest) {
                true => reply(200, json!({ "data": { "current_version": 1 } })),
                false => reply(404, json!({ "errors": [] })),
            },
            (Method::Delete, "metadata") => {
                entries.remove(&rest);
                reply(204, Value::Null)
            }
            _ => reply(405, json!({ "errors": ["unsupported"] })),
        }
    }
}

impl grauwolf_openbao::Transport for FakeKv {
    async fn send(
        &self,
        request: grauwolf_openbao::Request,
    ) -> Result<grauwolf_openbao::Response, grauwolf_openbao::BaoError> {
        if self.down.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(grauwolf_openbao::BaoError {
                status: None,
                message: "connection refused".into(),
            });
        }
        Ok(self.answer(request))
    }
}
