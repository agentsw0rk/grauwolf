//! In-memory registry for tests (feature `test-registry`): the subset of the OCI distribution
//! API v2 that the client uses (manifests and blobs by digest or tag, blob uploads, manifest
//! `PUT`; it stores whatever blobs and manifests the test hands it), optionally with the bearer-token challenge: a token for reading, and a distinct one
//! for writing that the token endpoint issues only for a `push` scope and, when it demands
//! credentials, only to a caller that sent them.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use serde_json::{Value, json};

use crate::{IMAGE_MANIFEST, sha256_digest};

#[derive(Default)]
struct Inner {
    blobs: HashMap<String, Vec<u8>>,
    /// (repository, tag or digest) → manifest.
    manifests: HashMap<(String, String), Vec<u8>>,
    /// Open upload sessions: id → bytes received so far.
    uploads: HashMap<String, Vec<u8>>,
    /// Honour `If-Match` / `If-None-Match` on manifest `PUT`s (answer `412`).
    conditional: bool,
    /// Answer `/v2/_catalog`.
    catalog: bool,
    /// A concurrent writer: (repository, tag) → the manifest it writes at the next `PUT` there.
    interfere: HashMap<(String, String), Vec<u8>>,
}

struct Shared {
    inner: Mutex<Inner>,
    /// Bearer challenge; the inner option demands basic credentials for the token request.
    token_auth: Option<Option<(String, String)>>,
    requests: Mutex<Vec<String>>,
    addr: Mutex<Option<SocketAddr>>,
}

const TOKEN: &str = "test-token";
const PUSH_TOKEN: &str = "test-push-token";

/// A running registry on `127.0.0.1`; stops when dropped.
pub struct TestRegistry {
    addr: SocketAddr,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

impl TestRegistry {
    /// Anonymous registry.
    pub async fn start() -> Self {
        Self::launch(None).await
    }

    /// Registry that answers `401` with a bearer challenge; the token endpoint is anonymous
    /// (`login` = `None`, issues read tokens only) or demands basic credentials
    /// `(user, password)` (and then also issues push tokens).
    pub async fn start_with_token_auth(login: Option<(&str, &str)>) -> Self {
        let login = login.map(|(user, password)| (user.to_owned(), password.to_owned()));
        Self::launch(Some(login)).await
    }

    async fn launch(token_auth: Option<Option<(String, String)>>) -> Self {
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner::default()),
            token_auth,
            requests: Mutex::new(Vec::new()),
            addr: Mutex::new(None),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the test registry");
        let addr = listener.local_addr().expect("address");
        *shared.addr.lock().expect("lock") = Some(addr);
        let app = Router::new()
            .fallback(handle)
            .with_state(Arc::clone(&shared));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { addr, shared, task }
    }

    /// `127.0.0.1:<port>`, the registry part of references.
    pub fn host(&self) -> String {
        self.addr.to_string()
    }

    /// Stores `bytes` as a blob and returns its digest.
    pub fn add_blob(&self, bytes: &[u8]) -> String {
        let digest = sha256_digest(bytes);
        self.shared
            .inner
            .lock()
            .expect("lock")
            .blobs
            .insert(digest.clone(), bytes.to_vec());
        digest
    }

    /// Stores a manifest as it is (its blobs are not checked), under `tag` and its digest.
    pub fn push_raw_manifest(&self, repository: &str, tag: &str, manifest: &[u8]) -> String {
        let digest = sha256_digest(manifest);
        let mut inner = self.shared.inner.lock().expect("lock");
        inner
            .manifests
            .insert((repository.into(), tag.into()), manifest.to_vec());
        inner
            .manifests
            .insert((repository.into(), digest.clone()), manifest.to_vec());
        digest
    }

    /// Replaces the content stored under the blob `digest`, so that it no longer matches.
    pub fn corrupt_blob(&self, digest: &str, bytes: &[u8]) {
        self.shared
            .inner
            .lock()
            .expect("lock")
            .blobs
            .insert(digest.into(), bytes.to_vec());
    }

    /// Makes manifest `PUT`s conditional: `If-Match: "<digest>"` and `If-None-Match: *` are
    /// checked against what the tag points to, a mismatch is `412` (off by default, like many
    /// registries).
    pub fn honour_conditional_writes(&self) {
        self.shared.inner.lock().expect("lock").conditional = true;
    }

    /// Lets the registry answer `/v2/_catalog` (off by default, like ghcr.io).
    pub fn offer_catalog(&self) {
        self.shared.inner.lock().expect("lock").catalog = true;
    }

    /// Simulates a concurrent writer at the next manifest `PUT` to `repository:tag`: with
    /// conditional writes it writes `manifest` just before (so the condition fails), else just
    /// after (so the `PUT` is overwritten). The blobs of `manifest` must be there.
    pub fn interfere_with_next_put(&self, repository: &str, tag: &str, manifest: &[u8]) {
        self.shared
            .inner
            .lock()
            .expect("lock")
            .interfere
            .insert((repository.into(), tag.into()), manifest.to_vec());
    }

    /// The manifest stored under `repository:reference` (tag or digest).
    pub fn manifest(&self, repository: &str, reference: &str) -> Option<Vec<u8>> {
        self.shared
            .inner
            .lock()
            .expect("lock")
            .manifests
            .get(&(repository.to_owned(), reference.to_owned()))
            .cloned()
    }

    /// The requests received so far as `<METHOD> <path>`.
    pub fn requests(&self) -> Vec<String> {
        self.shared.requests.lock().expect("lock").clone()
    }
}

impl Drop for TestRegistry {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn respond(status: StatusCode, body: Vec<u8>, headers: &[(&str, String)]) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    for (name, value) in headers {
        response.headers_mut().insert(
            header::HeaderName::from_bytes(name.as_bytes()).expect("header name"),
            HeaderValue::from_str(value).expect("header value"),
        );
    }
    response
}

async fn handle(State(shared): State<Arc<Shared>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path().to_owned();
    let query = parts.uri.query().unwrap_or("").to_owned();
    shared
        .requests
        .lock()
        .expect("lock")
        .push(format!("{} {path}", parts.method));
    let authorization = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if path == "/token" {
        return token(&shared, authorization.as_deref(), &query);
    }
    let Some(rest) = path.strip_prefix("/v2/") else {
        return respond(StatusCode::NOT_FOUND, Vec::new(), &[]);
    };
    let rest = rest.to_owned();
    let rest = rest.as_str();
    let writes = matches!(
        parts.method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );
    let repository = ["/manifests/", "/blobs/", "/tags/list"]
        .iter()
        .filter_map(|marker| rest.find(marker))
        .min()
        .map_or(rest, |at| &rest[..at]);
    if shared.token_auth.is_some() {
        let allowed = match authorization.as_deref() {
            Some(value) if value == format!("Bearer {PUSH_TOKEN}") => true,
            Some(value) if value == format!("Bearer {TOKEN}") => !writes,
            _ => false,
        };
        if !allowed {
            let addr = shared.addr.lock().expect("lock").expect("address");
            let actions = if writes { "push,pull" } else { "pull" };
            let challenge = format!(
                "Bearer realm=\"http://{addr}/token\",service=\"test-registry\",scope=\"repository:{repository}:{actions}\""
            );
            return respond(
                StatusCode::UNAUTHORIZED,
                Vec::new(),
                &[("www-authenticate", challenge)],
            );
        }
    }
    let Ok(body) = axum::body::to_bytes(body, 64 * 1024 * 1024).await else {
        return respond(StatusCode::BAD_REQUEST, Vec::new(), &[]);
    };
    let content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let header = |name: header::HeaderName| {
        parts
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let accept = header(header::ACCEPT).unwrap_or_default();
    let condition = (header(header::IF_MATCH), header(header::IF_NONE_MATCH));
    let mut inner = shared.inner.lock().expect("lock");
    if rest == "_catalog" {
        return list_repositories(&inner);
    }
    if let Some(repository) = rest.strip_suffix("/tags/list") {
        return list_tags(&inner, repository);
    }
    if let Some(at) = rest.rfind("/manifests/") {
        let reference = &rest[at + "/manifests/".len()..];
        return if parts.method == Method::PUT {
            let interfering = inner
                .interfere
                .remove(&(repository.to_owned(), reference.to_owned()));
            let conditional = inner.conditional;
            if let (Some(other), true) = (&interfering, conditional) {
                store_manifest(&mut inner, repository, reference, other);
            }
            if conditional && !condition_holds(&inner, repository, reference, &condition) {
                return respond(StatusCode::PRECONDITION_FAILED, Vec::new(), &[]);
            }
            let response = put_manifest(&mut inner, repository, reference, content_type, &body);
            if let (Some(other), false) = (&interfering, conditional) {
                store_manifest(&mut inner, repository, reference, other);
            }
            response
        } else {
            get_manifest(&inner, repository, reference, &accept)
        };
    }
    if let Some(at) = rest.find("/blobs/uploads/") {
        let session = &rest[at + "/blobs/uploads/".len()..];
        return upload(
            &mut inner,
            &parts.method,
            repository,
            session,
            &query,
            &body,
        );
    }
    if let Some(at) = rest.rfind("/blobs/") {
        let digest = &rest[at + "/blobs/".len()..];
        return match inner.blobs.get(digest) {
            // Like a real registry, `HEAD` has no body but announces the blob's size.
            Some(blob) if parts.method == Method::HEAD => respond(
                StatusCode::OK,
                Vec::new(),
                &[("content-length", blob.len().to_string())],
            ),
            Some(blob) => respond(StatusCode::OK, blob.clone(), &[]),
            None => respond(StatusCode::NOT_FOUND, Vec::new(), &[]),
        };
    }
    respond(StatusCode::NOT_FOUND, Vec::new(), &[])
}

fn store_manifest(inner: &mut Inner, repository: &str, reference: &str, manifest: &[u8]) {
    for key in [reference.to_owned(), sha256_digest(manifest)] {
        inner
            .manifests
            .insert((repository.to_owned(), key), manifest.to_vec());
    }
}

/// `If-Match: "<digest>"` names what the tag points to; `If-None-Match: *` that it points to
/// nothing.
fn condition_holds(
    inner: &Inner,
    repository: &str,
    reference: &str,
    (if_match, if_none_match): &(Option<String>, Option<String>),
) -> bool {
    let current = inner
        .manifests
        .get(&(repository.to_owned(), reference.to_owned()))
        .map(|manifest| sha256_digest(manifest));
    if let Some(expected) = if_match {
        return current.as_deref() == Some(expected.trim_matches('"'));
    }
    if if_none_match.as_deref() == Some("*") {
        return current.is_none();
    }
    true
}

fn list_tags(inner: &Inner, repository: &str) -> Response {
    let mut tags: Vec<&str> = inner
        .manifests
        .keys()
        .filter(|(repo, key)| repo == repository && !key.starts_with("sha256:"))
        .map(|(_, key)| key.as_str())
        .collect();
    if tags.is_empty() {
        return respond(StatusCode::NOT_FOUND, Vec::new(), &[]);
    }
    tags.sort_unstable();
    let body = json!({ "name": repository, "tags": tags }).to_string();
    respond(StatusCode::OK, body.into_bytes(), &[])
}

fn list_repositories(inner: &Inner) -> Response {
    if !inner.catalog {
        return respond(StatusCode::NOT_FOUND, Vec::new(), &[]);
    }
    let mut repositories: Vec<&str> = inner
        .manifests
        .keys()
        .map(|(repository, _)| repository.as_str())
        .collect();
    repositories.sort_unstable();
    repositories.dedup();
    let body = json!({ "repositories": repositories }).to_string();
    respond(StatusCode::OK, body.into_bytes(), &[])
}

/// Like the distribution registry: a manifest whose media type the caller does not accept is
/// answered in another representation (here a schema 1 stand-in), while the digest header names
/// the stored manifest – so the content does not match the digest.
fn get_manifest(inner: &Inner, repository: &str, reference: &str, accept: &str) -> Response {
    match inner
        .manifests
        .get(&(repository.to_owned(), reference.to_owned()))
    {
        Some(manifest)
            if serde_json::from_slice::<Value>(manifest)
                .ok()
                .and_then(|value| value["mediaType"].as_str().map(str::to_owned))
                .is_some_and(|media_type| {
                    media_type != IMAGE_MANIFEST && !accept.contains(&media_type)
                }) =>
        {
            respond(
                StatusCode::OK,
                br#"{"schemaVersion":1,"name":"converted"}"#.to_vec(),
                &[
                    (
                        "content-type",
                        "application/vnd.docker.distribution.manifest.v1+json".into(),
                    ),
                    ("docker-content-digest", sha256_digest(manifest)),
                ],
            )
        }
        Some(manifest) => respond(
            StatusCode::OK,
            manifest.clone(),
            &[
                ("content-type", IMAGE_MANIFEST.into()),
                ("docker-content-digest", sha256_digest(manifest)),
            ],
        ),
        None => respond(StatusCode::NOT_FOUND, Vec::new(), &[]),
    }
}

/// Stores the manifest under its tag and its digest if it is an image manifest whose config and
/// layers are all known blobs.
fn put_manifest(
    inner: &mut Inner,
    repository: &str,
    reference: &str,
    content_type: &str,
    body: &[u8],
) -> Response {
    if content_type != IMAGE_MANIFEST {
        return respond(StatusCode::UNSUPPORTED_MEDIA_TYPE, Vec::new(), &[]);
    }
    let digest = sha256_digest(body);
    if reference.starts_with("sha256:") && reference != digest {
        return respond(StatusCode::BAD_REQUEST, Vec::new(), &[]);
    }
    let Ok(manifest) = serde_json::from_slice::<Value>(body) else {
        return respond(StatusCode::BAD_REQUEST, Vec::new(), &[]);
    };
    let referenced = manifest["layers"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(std::iter::once(&manifest["config"]));
    for blob in referenced {
        let known = blob["digest"]
            .as_str()
            .is_some_and(|digest| inner.blobs.contains_key(digest));
        if !known {
            return respond(StatusCode::BAD_REQUEST, Vec::new(), &[]);
        }
    }
    for key in [reference.to_owned(), digest.clone()] {
        inner
            .manifests
            .insert((repository.to_owned(), key), body.to_vec());
    }
    respond(
        StatusCode::CREATED,
        Vec::new(),
        &[("docker-content-digest", digest)],
    )
}

/// `POST …/blobs/uploads/` opens a session, `PATCH` appends, `PUT ?digest=` completes it.
fn upload(
    inner: &mut Inner,
    method: &Method,
    repository: &str,
    session: &str,
    query: &str,
    body: &[u8],
) -> Response {
    let location = |id: &str| format!("/v2/{repository}/blobs/uploads/{id}");
    match *method {
        Method::POST => {
            let id = format!("upload-{}", inner.uploads.len());
            inner.uploads.insert(id.clone(), Vec::new());
            respond(
                StatusCode::ACCEPTED,
                Vec::new(),
                &[("location", location(&id))],
            )
        }
        Method::PATCH => match inner.uploads.get_mut(session) {
            Some(data) => {
                data.extend_from_slice(body);
                respond(
                    StatusCode::ACCEPTED,
                    Vec::new(),
                    &[("location", location(session))],
                )
            }
            None => respond(StatusCode::NOT_FOUND, Vec::new(), &[]),
        },
        Method::PUT => {
            let Some(mut data) = inner.uploads.remove(session) else {
                return respond(StatusCode::NOT_FOUND, Vec::new(), &[]);
            };
            data.extend_from_slice(body);
            let digest = query
                .split('&')
                .find_map(|pair| pair.strip_prefix("digest="))
                .unwrap_or("");
            if sha256_digest(&data) != digest {
                return respond(StatusCode::BAD_REQUEST, Vec::new(), &[]);
            }
            inner.blobs.insert(digest.to_owned(), data);
            respond(
                StatusCode::CREATED,
                Vec::new(),
                &[
                    ("location", format!("/v2/{repository}/blobs/{digest}")),
                    ("docker-content-digest", digest.to_owned()),
                ],
            )
        }
        _ => respond(StatusCode::METHOD_NOT_ALLOWED, Vec::new(), &[]),
    }
}

fn token(shared: &Shared, authorization: Option<&str>, query: &str) -> Response {
    use base64::Engine;
    let wants_push = query
        .split('&')
        .filter_map(|pair| pair.strip_prefix("scope="))
        .any(|scope| scope.contains("push"));
    if let Some(Some((user, password))) = &shared.token_auth {
        let expected = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
        );
        if authorization != Some(expected.as_str()) {
            return respond(StatusCode::UNAUTHORIZED, Vec::new(), &[]);
        }
    }
    let granted_push = wants_push && matches!(shared.token_auth, None | Some(Some(_)));
    let token = if granted_push { PUSH_TOKEN } else { TOKEN };
    respond(
        StatusCode::OK,
        json!({ "token": token }).to_string().into_bytes(),
        &[("content-type", "application/json".into())],
    )
}
