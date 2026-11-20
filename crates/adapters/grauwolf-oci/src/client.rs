//! Client for the subset of the OCI distribution API v2 that pulling needs: manifests and blobs,
//! anonymous access, the bearer-token flow and basic credentials.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use grauwolf_spec::OciReference;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, WWW_AUTHENTICATE};
use reqwest::{Client, Method, Response, StatusCode};

use crate::docker::{DockerConfig, default_config_path, path_dirs};
use crate::{OciError, sha256_digest};

/// Environment variable with the credentials per registry (the Docker configuration is the
/// fallback): a JSON map registry -> `"user:token"` (basic, also for the token request) or a bare
/// bearer token.
pub const REGISTRY_AUTH_ENV: &str = "GRAUWOLF_REGISTRY_AUTH";

/// Largest manifest or config that is read.
const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
/// Largest blob (module or sources) that is read.
const MAX_BLOB_BYTES: usize = 128 * 1024 * 1024;

/// Everything a repository may hold: a registry answers a media type that is not listed in
/// another representation (the distribution registry converts to schema 1), whose digest is not
/// the one of the stored manifest. Callers decide what to do with image indexes and docker
/// manifests; `manifest` only verifies the digest.
const MANIFEST_ACCEPT: &str = "application/vnd.oci.image.manifest.v1+json, \
     application/vnd.oci.artifact.manifest.v1+json, \
     application/vnd.oci.image.index.v1+json, \
     application/vnd.docker.distribution.manifest.list.v2+json, \
     application/vnd.docker.distribution.manifest.v2+json";

/// Credentials per registry.
#[derive(Debug, Clone, Default)]
pub struct RegistryAuth {
    credentials: BTreeMap<String, Credential>,
    docker: Option<DockerSource>,
}

/// The Docker configuration as fallback behind the explicit credentials. A configuration that
/// cannot be read only fails when a credential is actually needed, so anonymous pulls keep
/// working.
#[derive(Debug, Clone)]
struct DockerSource {
    config: Result<DockerConfig, String>,
    helper_dirs: Vec<PathBuf>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Credential {
    Basic { user: String, secret: String },
    Bearer(String),
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credential(..)")
    }
}

impl RegistryAuth {
    /// No credentials: anonymous pulls.
    pub fn none() -> Self {
        Self::default()
    }

    /// From the JSON text of `GRAUWOLF_REGISTRY_AUTH`; the error never repeats secrets.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let map: BTreeMap<String, String> = serde_json::from_str(text).map_err(|_| {
            format!("{REGISTRY_AUTH_ENV} must be a JSON map registry -> \"user:token\" or token")
        })?;
        let credentials = map
            .into_iter()
            .map(|(registry, value)| {
                let credential = match value.split_once(':') {
                    Some((user, secret)) => Credential::Basic {
                        user: user.to_owned(),
                        secret: secret.to_owned(),
                    },
                    None => Credential::Bearer(value),
                };
                (registry, credential)
            })
            .collect();
        Ok(Self {
            credentials,
            docker: None,
        })
    }

    /// From the environment: `GRAUWOLF_REGISTRY_AUTH` first, then the Docker configuration
    /// (`$DOCKER_CONFIG/config.json` or `~/.docker/config.json`) with helpers found in `PATH`.
    pub fn from_env() -> Result<Self, String> {
        let auth = match std::env::var(REGISTRY_AUTH_ENV) {
            Ok(text) if !text.trim().is_empty() => Self::from_json(&text)?,
            _ => Self::none(),
        };
        Ok(match default_config_path() {
            Some(path) => auth.with_docker_config(&path, path_dirs()),
            None => auth,
        })
    }

    /// Adds the Docker configuration at `path` as fallback behind the explicit credentials;
    /// credential helpers are looked up in `helper_dirs`.
    pub fn with_docker_config(mut self, path: &Path, helper_dirs: Vec<PathBuf>) -> Self {
        self.docker = Some(DockerSource {
            config: DockerConfig::load(path),
            helper_dirs,
        });
        self
    }

    /// A bearer token from the explicit credentials, sent without a challenge. The Docker
    /// configuration is not consulted here: its helpers run only when a registry asks.
    fn static_bearer(&self, registry: &str) -> Option<String> {
        match self.credentials.get(registry) {
            Some(Credential::Bearer(token)) => Some(token.clone()),
            _ => None,
        }
    }

    /// The credential for `registry`, looked up only when an answer needs it: the explicit
    /// credentials first, then the Docker configuration. Async because credential helpers run
    /// programs; they must not block the async workers.
    pub(crate) async fn credential(&self, registry: &str) -> Result<Option<Credential>, String> {
        if let Some(credential) = self.credentials.get(registry) {
            return Ok(Some(credential.clone()));
        }
        match &self.docker {
            Some(source) => match &source.config {
                Ok(config) => config.credential(registry, &source.helper_dirs).await,
                Err(error) => Err(error.clone()),
            },
            None => Ok(None),
        }
    }
}

/// Registry client; keeps the tokens it has obtained.
pub struct RegistryClient {
    http: Client,
    auth: RegistryAuth,
    tokens: Mutex<BTreeMap<(String, String), String>>,
}

impl Default for RegistryClient {
    fn default() -> Self {
        Self::new(RegistryAuth::none())
    }
}

/// Content and digest of a fetched manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedManifest {
    pub digest: String,
    pub bytes: Vec<u8>,
}

impl RegistryClient {
    /// Client that authenticates with `auth`.
    pub fn new(auth: RegistryAuth) -> Self {
        Self {
            http: Client::new(),
            auth,
            tokens: Mutex::new(BTreeMap::new()),
        }
    }

    /// Like [`new`](Self::new), but speaks HTTP/2 without negotiation (for registries behind
    /// TLS this is what `h2` negotiates). Test aid: over HTTP/2 the size of a `HEAD` answer
    /// is announced although no body follows.
    #[cfg(feature = "test-registry")]
    pub fn new_http2_prior_knowledge(auth: RegistryAuth) -> Self {
        Self {
            http: Client::builder()
                .http2_prior_knowledge()
                .build()
                .expect("http client"),
            auth,
            tokens: Mutex::new(BTreeMap::new()),
        }
    }

    /// Manifest of `reference` (by digest if the reference has one, else by tag); the digest is
    /// computed from the content and checked against the registry's header and the reference.
    pub async fn manifest(&self, reference: &OciReference) -> Result<FetchedManifest, OciError> {
        let wanted = reference.reference();
        let url = format!(
            "{}/v2/{}/manifests/{wanted}",
            base_url(&reference.registry),
            reference.repository
        );
        let (bytes, header) = self
            .get(reference, &url, MANIFEST_ACCEPT, MAX_MANIFEST_BYTES)
            .await?;
        let digest = sha256_digest(&bytes);
        let announced = reference.digest.clone().or(header);
        if let Some(expected) = announced.filter(|expected| *expected != digest) {
            return Err(OciError::DigestMismatch {
                what: format!("manifest {}/{}", reference.registry, reference.repository),
                expected,
                actual: digest,
            });
        }
        Ok(FetchedManifest { digest, bytes })
    }

    /// The blob `digest` of the repository of `reference`, verified against its digest.
    pub async fn blob(&self, reference: &OciReference, digest: &str) -> Result<Vec<u8>, OciError> {
        let url = format!(
            "{}/v2/{}/blobs/{digest}",
            base_url(&reference.registry),
            reference.repository
        );
        let (bytes, _) = self
            .get(reference, &url, "application/octet-stream", MAX_BLOB_BYTES)
            .await?;
        let actual = sha256_digest(&bytes);
        if actual != digest {
            return Err(OciError::DigestMismatch {
                what: format!("blob of {}/{}", reference.registry, reference.repository),
                expected: digest.to_owned(),
                actual,
            });
        }
        Ok(bytes)
    }

    /// GET for pulls: body and `Docker-Content-Digest` of a successful answer.
    async fn get(
        &self,
        reference: &OciReference,
        url: &str,
        accept: &str,
        limit: usize,
    ) -> Result<(Vec<u8>, Option<String>), OciError> {
        let request = Request::new(Method::GET, url, Scope::Pull).header(ACCEPT, accept);
        let reply = self.send(reference, request, limit).await?;
        match reply.status {
            status if status.is_success() => {
                let digest = reply.digest();
                Ok((reply.body, digest))
            }
            StatusCode::NOT_FOUND => Err(not_found(reference, url)),
            status => Err(unexpected(reference, url, status)),
        }
    }

    /// Sends `request` with the authentication flow (static bearer token, token obtained earlier,
    /// token or basic challenge on `401`) and returns every answer except an authentication
    /// failure, which is an error. The credential is looked up only when a challenge needs it.
    pub(crate) async fn send(
        &self,
        reference: &OciReference,
        request: Request<'_>,
        limit: usize,
    ) -> Result<Reply, OciError> {
        let registry = &reference.registry;
        let url = request.url;
        let scope = request.scope.for_repository(&reference.repository);
        let fail = |reason: String| OciError::Registry {
            registry: registry.clone(),
            reason,
        };
        let unauthorized = || OciError::Unauthorized {
            registry: registry.clone(),
        };
        let key = (registry.clone(), scope.clone());
        let mut authorization = self.initial_authorization(registry, &key).await?;
        for attempt in 0..2 {
            let mut builder = self.http.request(request.method.clone(), url);
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }
            if let Some(body) = request.body {
                builder = builder.body(body.to_vec());
            }
            if let Some(value) = &authorization {
                builder = builder.header(AUTHORIZATION, value);
            }
            let response = builder
                .send()
                .await
                .map_err(|error| fail(format!("{}: {}", url_without_query(url), error)))?;
            match response.status() {
                StatusCode::UNAUTHORIZED if attempt == 0 => {
                    authorization = self
                        .answer_challenge(&response, &key, &scope, authorization.as_deref())
                        .await?;
                    if authorization.is_none() {
                        return Err(unauthorized());
                    }
                }
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => return Err(unauthorized()),
                status => {
                    // A `HEAD` answer has no body; over HTTP/2 its size hint still carries the
                    // announced Content-Length of the resource (a blob), which is not read.
                    if request.method != Method::HEAD
                        && response
                            .content_length()
                            .is_some_and(|len| len as usize > limit)
                    {
                        return Err(fail(format!("answer larger than {limit} bytes")));
                    }
                    let headers = response.headers().clone();
                    let body = response
                        .bytes()
                        .await
                        .map_err(|error| fail(error.to_string()))?;
                    if body.len() > limit {
                        return Err(fail(format!("answer larger than {limit} bytes")));
                    }
                    return Ok(Reply {
                        status,
                        headers,
                        body: body.to_vec(),
                    });
                }
            }
        }
        Err(unauthorized())
    }

    async fn credential(&self, registry: &str) -> Result<Option<Credential>, OciError> {
        self.auth
            .credential(registry)
            .await
            .map_err(|reason| OciError::Registry {
                registry: registry.to_owned(),
                reason,
            })
    }

    /// Token obtained earlier, or a static bearer token from the credentials.
    async fn initial_authorization(
        &self,
        registry: &str,
        key: &(String, String),
    ) -> Result<Option<String>, OciError> {
        if let Some(token) = self.tokens.lock().expect("token lock").get(key) {
            return Ok(Some(format!("Bearer {token}")));
        }
        Ok(self
            .auth
            .static_bearer(registry)
            .map(|token| format!("Bearer {token}")))
    }

    /// Reacts to `401` and returns the `Authorization` value for the retry: from the bearer
    /// token flow (`realm`, `service`, `scope`), or basic credentials for a basic challenge.
    async fn answer_challenge(
        &self,
        response: &Response,
        key: &(String, String),
        scope: &str,
        sent: Option<&str>,
    ) -> Result<Option<String>, OciError> {
        let registry = &key.0;
        let Some(challenge) = response
            .headers()
            .get(WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok())
        else {
            return Ok(None);
        };
        let credential = self.credential(registry).await?;
        // A bearer token that was not sent yet (an identity token of the Docker configuration)
        // is used as it is.
        if let Some(Credential::Bearer(token)) = &credential {
            let value = format!("Bearer {token}");
            if sent != Some(value.as_str()) {
                return Ok(Some(value));
            }
        }
        if challenge
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("basic")
        {
            return Ok(match &credential {
                Some(Credential::Basic { user, secret }) => Some(basic(user, secret)),
                _ => None,
            });
        }
        let params = parse_challenge(challenge);
        let Some(realm) = params.get("realm") else {
            return Ok(None);
        };
        let fail = |reason: String| OciError::Registry {
            registry: registry.clone(),
            reason,
        };
        let mut request = self.http.get(realm);
        let mut query = Vec::new();
        if let Some(service) = params.get("service") {
            query.push(("service", service.as_str()));
        }
        query.push(("scope", params.get("scope").map_or(scope, String::as_str)));
        request = request.query(&query);
        if let Some(Credential::Basic { user, secret }) = &credential {
            request = request.header(AUTHORIZATION, basic(user, secret));
        }
        let answer = request
            .send()
            .await
            .map_err(|error| fail(format!("token request: {error}")))?;
        if !answer.status().is_success() {
            return Err(OciError::Unauthorized {
                registry: registry.clone(),
            });
        }
        let body: serde_json::Value = answer
            .json()
            .await
            .map_err(|error| fail(format!("token answer: {error}")))?;
        let token = body["token"]
            .as_str()
            .or_else(|| body["access_token"].as_str())
            .ok_or_else(|| fail("token answer without `token`".into()))?;
        self.tokens
            .lock()
            .expect("token lock")
            .insert(key.clone(), token.to_owned());
        Ok(Some(format!("Bearer {token}")))
    }
}

/// What a request needs from the token: reading, or also writing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    Pull,
    Push,
    /// Listing the repositories of the registry (`/v2/_catalog`).
    Catalog,
}

impl Scope {
    fn for_repository(self, repository: &str) -> String {
        match self {
            Scope::Pull => format!("repository:{repository}:pull"),
            Scope::Push => format!("repository:{repository}:push,pull"),
            Scope::Catalog => "registry:catalog:*".to_owned(),
        }
    }
}

/// A request to the registry; the body is kept so that it can be sent again after a challenge.
pub(crate) struct Request<'a> {
    pub(crate) method: Method,
    pub(crate) url: &'a str,
    pub(crate) scope: Scope,
    pub(crate) headers: Vec<(HeaderName, String)>,
    pub(crate) body: Option<&'a [u8]>,
}

impl<'a> Request<'a> {
    pub(crate) fn new(method: Method, url: &'a str, scope: Scope) -> Self {
        Self {
            method,
            url,
            scope,
            headers: Vec::new(),
            body: None,
        }
    }

    pub(crate) fn body(mut self, body: &'a [u8]) -> Self {
        self.body = Some(body);
        self
    }

    pub(crate) fn header(mut self, name: HeaderName, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }
}

/// Answer of the registry (any status except an authentication failure).
pub(crate) struct Reply {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Vec<u8>,
}

impl Reply {
    pub(crate) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    }

    pub(crate) fn digest(&self) -> Option<String> {
        self.header("docker-content-digest")
    }
}

pub(crate) fn not_found(reference: &OciReference, url: &str) -> OciError {
    OciError::NotFound {
        registry: reference.registry.clone(),
        what: url_without_query(url)
            .trim_start_matches(&base_url(&reference.registry))
            .to_owned(),
    }
}

pub(crate) fn unexpected(reference: &OciReference, url: &str, status: StatusCode) -> OciError {
    OciError::Registry {
        registry: reference.registry.clone(),
        reason: format!("{} answered {status}", url_without_query(url)),
    }
}

fn basic(user: &str, secret: &str) -> String {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{secret}"));
    format!("Basic {encoded}")
}

/// `https`, except for local registries (`localhost`, `127.0.0.1`), like Docker's insecure-local
/// default.
pub fn base_url(registry: &str) -> String {
    let host = registry.split(':').next().unwrap_or(registry);
    let scheme = if matches!(host, "localhost" | "127.0.0.1") {
        "http"
    } else {
        "https"
    };
    format!("{scheme}://{registry}")
}

pub(crate) fn url_without_query(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

/// Parameters of `Bearer realm="…",service="…",scope="…"`.
fn parse_challenge(header: &str) -> BTreeMap<String, String> {
    let rest = header
        .trim_start()
        .split_once(' ')
        .map_or("", |(_, rest)| rest);
    let mut params = BTreeMap::new();
    let mut remaining = rest;
    while let Some((key, after)) = remaining.split_once('=') {
        let key = key
            .trim()
            .trim_start_matches(',')
            .trim()
            .to_ascii_lowercase();
        let after = after.trim_start();
        let (value, next) = match after.strip_prefix('"') {
            Some(quoted) => match quoted.split_once('"') {
                Some((value, next)) => (value, next),
                None => (quoted, ""),
            },
            None => match after.split_once(',') {
                Some((value, next)) => (value, next),
                None => (after, ""),
            },
        };
        params.insert(key, value.to_owned());
        remaining = next;
    }
    params
}
