//! A smart-HTTP git server for tests (feature `test-git-server`): serves one repository on
//! disk over `GET …/info/refs?service=…`, `POST …/git-upload-pack` and `POST …/git-receive-pack`
//! under any path, records the requests, and can require a token or, in the mTLS mode
//! ([`TestGitServer::start_spiffe`], ADR-0110), a client certificate with a SPIFFE ID that a
//! mapping ([`SpiffeAccess`]) allows to read or to push. No shallow support (clients fall back
//! to full fetches), no `git` program: packs are built and indexed with libgit2.
//!
//! The mTLS listener and the ID-to-user mapping are a small copy of what `grauwolf-server` has
//! (`TlsListener`, `CallerMap`): an adapter must not depend on an app, and the copy only has to
//! serve tests and the live-check image.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use git2::{Buf, Oid, Repository, Signature};
use grauwolf_spiffe::{SpiffeId, TlsIdentity, spiffe_id_of_certificate};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

/// A request the server received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRequest {
    /// `METHOD path?query`.
    pub line: String,
    /// The `Authorization` header, if any.
    pub authorization: Option<String>,
    /// The SPIFFE ID of the client certificate (mTLS mode), if any.
    pub spiffe_id: Option<String>,
    /// The status the server answered.
    pub status: u16,
}

/// What a mapped SPIFFE ID may do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// The git user the ID maps to.
    pub user: String,
    /// May the user push (otherwise read only)?
    pub push: bool,
}

/// The mapping from SPIFFE IDs to git users: `{ "<spiffe id or prefix/*>": { "user": "<name>",
/// "push": true|false } }`. An exact entry wins over a prefix, the longest prefix over a shorter
/// one; prefixes match whole path segments. From a file it is read again at every request, so a
/// changed ConfigMap takes effect without a restart.
#[derive(Debug, Clone)]
pub struct SpiffeAccess {
    source: AccessSource,
}

#[derive(Debug, Clone)]
enum AccessSource {
    Fixed(Entries),
    File(PathBuf),
}

#[derive(Debug, Clone, Default)]
struct Entries {
    exact: BTreeMap<String, Grant>,
    prefixes: Vec<(String, Grant)>,
}

impl Entries {
    fn parse(text: &str) -> Result<Self, String> {
        let map: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(text).map_err(|error| format!("mapping file: {error}"))?;
        let mut entries = Self::default();
        for (key, value) in map {
            let user = value["user"]
                .as_str()
                .filter(|user| !user.is_empty())
                .ok_or_else(|| format!("mapping file: the entry for '{key}' needs a `user`"))?;
            let push = match &value["push"] {
                serde_json::Value::Null => false,
                serde_json::Value::Bool(push) => *push,
                _ => return Err(format!("mapping file: `push` of '{key}' is not a boolean")),
            };
            let grant = Grant {
                user: user.to_owned(),
                push,
            };
            match key.strip_suffix("/*") {
                Some(prefix) => {
                    SpiffeId::parse(prefix).map_err(|error| format!("mapping file: {error}"))?;
                    entries.prefixes.push((prefix.to_owned(), grant));
                }
                None => {
                    SpiffeId::parse(&key).map_err(|error| format!("mapping file: {error}"))?;
                    entries.exact.insert(key, grant);
                }
            }
        }
        Ok(entries)
    }

    fn grant(&self, id: &str) -> Option<Grant> {
        if let Some(grant) = self.exact.get(id) {
            return Some(grant.clone());
        }
        self.prefixes
            .iter()
            .filter(|(prefix, _)| {
                id.strip_prefix(prefix.as_str())
                    .is_some_and(|rest| rest.starts_with('/') && rest.len() > 1)
            })
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, grant)| grant.clone())
    }
}

impl SpiffeAccess {
    /// A fixed mapping from JSON text.
    pub fn from_json(text: &str) -> Result<Self, String> {
        Ok(Self {
            source: AccessSource::Fixed(Entries::parse(text)?),
        })
    }

    /// The mapping in `path`; checked now, read again at every request.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Entries::parse(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Self {
            source: AccessSource::File(path.to_owned()),
        })
    }

    /// What `id` may do; `None` if it is not mapped (also when the file cannot be read).
    pub fn grant(&self, id: &SpiffeId) -> Option<Grant> {
        match &self.source {
            AccessSource::Fixed(entries) => entries.grant(id.as_str()),
            AccessSource::File(path) => {
                let entries = std::fs::read_to_string(path)
                    .map_err(|error| error.to_string())
                    .and_then(|text| Entries::parse(&text))
                    .inspect_err(|error| eprintln!("test git server: {error}"))
                    .ok()?;
                entries.grant(id.as_str())
            }
        }
    }
}

/// The SPIFFE ID of the peer of an accepted mTLS connection.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PeerSpiffe(SpiffeId);

struct SpiffeMode {
    access: SpiffeAccess,
}

struct Shared {
    repository: PathBuf,
    token: Option<String>,
    spiffe: Option<SpiffeMode>,
    requests: Mutex<Vec<RecordedRequest>>,
}

/// The running server; dropping it stops it.
pub struct TestGitServer {
    addr: SocketAddr,
    https: bool,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

impl TestGitServer {
    /// Serves the repository at `repository` on 127.0.0.1; with `token` every request needs
    /// `Authorization: Basic` for the user `x-access-token` and that token.
    pub async fn start(repository: &Path, token: Option<&str>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        Self::serve(listener, repository, token)
    }

    /// Like [`TestGitServer::start`] on the port of `listener`, for restarting a server on the
    /// address that a lockfile or spec names.
    pub fn serve(
        listener: tokio::net::TcpListener,
        repository: &Path,
        token: Option<&str>,
    ) -> Self {
        let addr = listener.local_addr().unwrap();
        let shared = Arc::new(Shared {
            repository: repository.to_owned(),
            token: token.map(str::to_owned),
            spiffe: None,
            requests: Mutex::new(Vec::new()),
        });
        let app = Router::new().fallback(handle).with_state(shared.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            addr,
            https: false,
            shared,
            task,
        }
    }

    /// Serves over mTLS on 127.0.0.1 (ADR-0110): the server presents the SVID of `identity` and
    /// requires a client certificate of a trust domain in `accepted`; `access` decides who may
    /// read and who may push (unmapped IDs and read-only IDs on push get `403`).
    pub async fn start_spiffe(
        repository: &Path,
        identity: &dyn TlsIdentity,
        accepted: &[String],
        access: SpiffeAccess,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        Self::serve_spiffe(listener, repository, identity, accepted, access).unwrap()
    }

    /// Like [`TestGitServer::start_spiffe`] on `listener`.
    pub fn serve_spiffe(
        listener: tokio::net::TcpListener,
        repository: &Path,
        identity: &dyn TlsIdentity,
        accepted: &[String],
        access: SpiffeAccess,
    ) -> Result<Self, String> {
        let config = identity
            .server_tls(accepted)
            .map_err(|error| error.to_string())?;
        let addr = listener.local_addr().unwrap();
        let shared = Arc::new(Shared {
            repository: repository.to_owned(),
            token: None,
            spiffe: Some(SpiffeMode { access }),
            requests: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .fallback(handle)
            .with_state(shared.clone())
            .into_make_service_with_connect_info::<PeerSpiffe>();
        let listener = TlsListener::new(listener, Arc::new(config));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            addr,
            https: true,
            shared,
            task,
        })
    }

    /// `http://127.0.0.1:<port>/<name>.git` (`https` in the mTLS mode).
    pub fn url(&self, name: &str) -> String {
        let scheme = if self.https { "https" } else { "http" };
        format!("{scheme}://{}/{name}.git", self.addr)
    }

    /// The port, for starting the server again on it.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Every request so far.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.shared.requests.lock().unwrap().clone()
    }

    /// Stops the server and waits until its port is free.
    pub async fn stop(self) {
        self.task.abort();
        while !self.task.is_finished() {
            tokio::task::yield_now().await;
        }
    }
}

impl Drop for TestGitServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn handle(State(shared): State<Arc<Shared>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let authorization = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let line = format!(
        "{} {}",
        parts.method,
        parts.uri.path_and_query().map_or("", |p| p.as_str())
    );
    let peer = parts
        .extensions
        .get::<axum::extract::ConnectInfo<PeerSpiffe>>()
        .map(|info| info.0.0.clone());
    let index = {
        let mut requests = shared.requests.lock().unwrap();
        requests.push(RecordedRequest {
            line,
            authorization: authorization.clone(),
            spiffe_id: peer.as_ref().map(|id| id.to_string()),
            status: 0,
        });
        requests.len() - 1
    };
    let response = answer(&shared, parts, body, authorization, peer).await;
    shared.requests.lock().unwrap()[index].status = response.status().as_u16();
    response
}

async fn answer(
    shared: &Shared,
    parts: axum::http::request::Parts,
    body: Body,
    authorization: Option<String>,
    peer: Option<SpiffeId>,
) -> Response {
    if let Some(token) = &shared.token {
        let expected = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"))
        );
        if authorization.as_deref() != Some(expected.as_str()) {
            return respond(StatusCode::UNAUTHORIZED, "text/plain", Vec::new());
        }
    }
    let path = parts.uri.path().to_owned();
    let query = parts.uri.query().unwrap_or_default().to_owned();
    let pushing = path.ends_with("/git-receive-pack") || query == "service=git-receive-pack";
    if let Some(mode) = &shared.spiffe {
        let grant = peer.and_then(|id| mode.access.grant(&id));
        match grant {
            Some(grant) if grant.push || !pushing => {}
            _ => return respond(StatusCode::FORBIDDEN, "text/plain", Vec::new()),
        }
    }
    let repository = shared.repository.clone();
    if parts.method == Method::GET && path.ends_with("/info/refs") {
        let service = match query.as_str() {
            "service=git-upload-pack" => "git-upload-pack",
            "service=git-receive-pack" => "git-receive-pack",
            _ => return respond(StatusCode::FORBIDDEN, "text/plain", Vec::new()),
        };
        let body = tokio::task::spawn_blocking(move || advertisement(&repository, service)).await;
        return match body {
            Ok(Ok(bytes)) => respond(
                StatusCode::OK,
                &format!("application/x-{service}-advertisement"),
                bytes,
            ),
            _ => respond(StatusCode::INTERNAL_SERVER_ERROR, "text/plain", Vec::new()),
        };
    }
    if parts.method == Method::POST && path.ends_with("/git-upload-pack") {
        let Ok(bytes) = axum::body::to_bytes(body, 64 * 1024 * 1024).await else {
            return respond(StatusCode::BAD_REQUEST, "text/plain", Vec::new());
        };
        let body = tokio::task::spawn_blocking(move || upload_pack(&repository, &bytes)).await;
        return match body {
            Ok(Ok(bytes)) => respond(
                StatusCode::OK,
                "application/x-git-upload-pack-result",
                bytes,
            ),
            _ => respond(StatusCode::INTERNAL_SERVER_ERROR, "text/plain", Vec::new()),
        };
    }
    if parts.method == Method::POST && path.ends_with("/git-receive-pack") {
        let Ok(bytes) = axum::body::to_bytes(body, 64 * 1024 * 1024).await else {
            return respond(StatusCode::BAD_REQUEST, "text/plain", Vec::new());
        };
        let body = tokio::task::spawn_blocking(move || receive_pack(&repository, &bytes)).await;
        return match body {
            Ok(Ok(bytes)) => respond(
                StatusCode::OK,
                "application/x-git-receive-pack-result",
                bytes,
            ),
            _ => respond(StatusCode::INTERNAL_SERVER_ERROR, "text/plain", Vec::new()),
        };
    }
    respond(StatusCode::NOT_FOUND, "text/plain", Vec::new())
}

fn respond(status: StatusCode, content_type: &str, body: Vec<u8>) -> Response {
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type);
    if status == StatusCode::UNAUTHORIZED {
        builder = builder.header(header::WWW_AUTHENTICATE, "Basic realm=\"git\"");
    }
    builder.body(Body::from(body)).unwrap()
}

fn pkt(data: &[u8]) -> Vec<u8> {
    let mut out = format!("{:04x}", data.len() + 4).into_bytes();
    out.extend_from_slice(data);
    out
}

fn advertisement(repository: &Path, service: &str) -> Result<Vec<u8>, git2::Error> {
    let receiving = service == "git-receive-pack";
    let repository = Repository::open(repository)?;
    let mut refs: Vec<(String, Oid)> = Vec::new();
    let mut head_target = None;
    if !receiving && let Ok(head) = repository.find_reference("HEAD") {
        head_target = head.symbolic_target().ok().flatten().map(str::to_owned);
        if let Ok(oid) = head
            .resolve()
            .and_then(|r| r.target().ok_or_else(|| git2::Error::from_str("no target")))
        {
            refs.push(("HEAD".to_owned(), oid));
        }
    }
    let mut named = Vec::new();
    for reference in repository.references()? {
        let reference = reference?;
        let Ok(name) = reference.name() else {
            continue;
        };
        if !(name.starts_with("refs/heads/") || name.starts_with("refs/tags/")) {
            continue;
        }
        if let Some(oid) = reference.resolve()?.target() {
            named.push((name.to_owned(), oid));
        }
    }
    named.sort();
    for (name, oid) in named {
        refs.push((name.clone(), oid));
        if let Ok(object) = repository.find_object(oid, None) {
            if object.kind() == Some(git2::ObjectType::Tag) {
                if let Ok(peeled) = object.peel(git2::ObjectType::Commit) {
                    refs.push((format!("{name}^{{}}"), peeled.id()));
                }
            }
        }
    }
    let mut capabilities = if receiving {
        "report-status delete-refs ofs-delta agent=grauwolf-test".to_owned()
    } else {
        "ofs-delta allow-tip-sha1-in-want allow-reachable-sha1-in-want agent=grauwolf-test"
            .to_owned()
    };
    if let Some(target) = head_target {
        capabilities.push_str(&format!(" symref=HEAD:{target}"));
    }
    let mut out = pkt(format!("# service={service}\n").as_bytes());
    out.extend_from_slice(b"0000");
    if refs.is_empty() {
        let line = format!("{} capabilities^{{}}\0{capabilities}\n", "0".repeat(40));
        out.extend(pkt(line.as_bytes()));
    }
    for (index, (name, oid)) in refs.iter().enumerate() {
        let line = if index == 0 {
            format!("{oid} {name}\0{capabilities}\n")
        } else {
            format!("{oid} {name}\n")
        };
        out.extend(pkt(line.as_bytes()));
    }
    out.extend_from_slice(b"0000");
    Ok(out)
}

/// Answers a fetch: `NAK` and a pack of everything reachable from the wanted objects (the `have`
/// lines are ignored; scratch repositories never have any).
fn upload_pack(repository: &Path, request: &[u8]) -> Result<Vec<u8>, git2::Error> {
    let mut wants = Vec::new();
    let mut rest = request;
    while rest.len() >= 4 {
        let Ok(length) = usize::from_str_radix(std::str::from_utf8(&rest[..4]).unwrap_or("x"), 16)
        else {
            break;
        };
        if length < 4 {
            rest = &rest[4..];
            continue;
        }
        if rest.len() < length {
            break;
        }
        let line = String::from_utf8_lossy(&rest[4..length]).into_owned();
        rest = &rest[length..];
        if let Some(want) = line.strip_prefix("want ") {
            wants.push(Oid::from_str(want.get(..40).unwrap_or(want))?);
        }
    }
    let repository = Repository::open(repository)?;
    let mut builder = repository.packbuilder()?;
    for oid in wants {
        let object = repository.find_object(oid, None)?;
        if object.kind() == Some(git2::ObjectType::Tag) {
            builder.insert_object(oid, None)?;
        }
        let commit = object.peel(git2::ObjectType::Commit)?;
        let mut walk = repository.revwalk()?;
        walk.push(commit.id())?;
        builder.insert_walk(&mut walk)?;
    }
    let mut buffer = Buf::new();
    builder.write_buf(&mut buffer)?;
    let mut out = pkt(b"NAK\n");
    out.extend_from_slice(&buffer);
    Ok(out)
}

/// Answers a push: reads the commands (`<old> <new> <ref>`), indexes the pack that follows into
/// the object database, updates the refs and reports the status per ref. A ref whose current
/// value is not `<old>` is refused (`ng`), as git does.
fn receive_pack(repository: &Path, request: &[u8]) -> Result<Vec<u8>, git2::Error> {
    let mut commands = Vec::new();
    let mut rest = request;
    loop {
        if rest.len() < 4 {
            break;
        }
        let length = usize::from_str_radix(std::str::from_utf8(&rest[..4]).unwrap_or("x"), 16)
            .map_err(|_| git2::Error::from_str("bad pkt-line"))?;
        if length == 0 {
            rest = &rest[4..];
            break;
        }
        if length < 4 || rest.len() < length {
            return Err(git2::Error::from_str("bad pkt-line"));
        }
        let line = String::from_utf8_lossy(&rest[4..length]).into_owned();
        rest = &rest[length..];
        let line = line.split('\0').next().unwrap_or("").trim_end().to_owned();
        let mut parts = line.splitn(3, ' ');
        let (Some(old), Some(new), Some(name)) = (parts.next(), parts.next(), parts.next()) else {
            return Err(git2::Error::from_str("bad command"));
        };
        commands.push((Oid::from_str(old)?, Oid::from_str(new)?, name.to_owned()));
    }
    let repository = Repository::open(repository)?;
    if !rest.is_empty() {
        let odb = repository.odb()?;
        let mut writer = odb.packwriter()?;
        std::io::Write::write_all(&mut writer, rest)
            .map_err(|error| git2::Error::from_str(&error.to_string()))?;
        writer.commit()?;
    }
    let mut out = pkt(b"unpack ok\n");
    for (old, new, name) in commands {
        let current = repository
            .find_reference(&name)
            .ok()
            .and_then(|reference| reference.target());
        let expected = (!old.is_zero()).then_some(old);
        let result = if current != expected {
            Err("stale info".to_owned())
        } else if new.is_zero() {
            repository
                .find_reference(&name)
                .and_then(|mut reference| reference.delete())
                .map_err(|error| error.message().to_owned())
        } else {
            repository
                .reference(&name, new, true, "push")
                .map(|_| ())
                .map_err(|error| error.message().to_owned())
        };
        out.extend(match result {
            Ok(()) => pkt(format!("ok {name}\n").as_bytes()),
            Err(reason) => pkt(format!("ng {name} {reason}\n").as_bytes()),
        });
    }
    out.extend_from_slice(b"0000");
    Ok(out)
}

/// Accepts TCP connections, does the TLS handshakes concurrently and hands out the finished
/// connections with the client's SPIFFE ID (connections without one are dropped).
struct TlsListener {
    ready: mpsc::Receiver<(TlsStream<tokio::net::TcpStream>, PeerSpiffe)>,
    local: SocketAddr,
    task: JoinHandle<()>,
}

impl TlsListener {
    fn new(tcp: tokio::net::TcpListener, config: Arc<tokio_rustls::rustls::ServerConfig>) -> Self {
        let local = tcp.local_addr().expect("a bound listener has an address");
        let (sender, ready) = mpsc::channel(64);
        let acceptor = TlsAcceptor::from(config);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = tcp.accept().await else {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                let (acceptor, sender) = (acceptor.clone(), sender.clone());
                tokio::spawn(async move {
                    let Ok(Ok(tls)) =
                        tokio::time::timeout(Duration::from_secs(10), acceptor.accept(stream))
                            .await
                    else {
                        return;
                    };
                    let peer = tls
                        .get_ref()
                        .1
                        .peer_certificates()
                        .and_then(|chain| chain.first())
                        .and_then(|cert| spiffe_id_of_certificate(cert).ok());
                    if let Some(id) = peer {
                        let _ = sender.send((tls, PeerSpiffe(id))).await;
                    }
                });
            }
        });
        Self { ready, local, task }
    }
}

impl Drop for TlsListener {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, TlsListener>>
    for PeerSpiffe
{
    fn connect_info(stream: axum::serve::IncomingStream<'_, TlsListener>) -> Self {
        stream.remote_addr().clone()
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<tokio::net::TcpStream>;
    type Addr = PeerSpiffe;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.ready.recv().await {
            Some(connection) => connection,
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Err(std::io::Error::other(format!(
            "mTLS listener on {}",
            self.local
        )))
    }
}

/// A key that signs commits and tags with an SSH signature (ed25519, namespace `git`), for tests
/// of signature checks; no `ssh-keygen` is needed.
pub struct TestSigner(ssh_key::PrivateKey);

impl Default for TestSigner {
    fn default() -> Self {
        Self::new()
    }
}

impl TestSigner {
    /// A new random key.
    pub fn new() -> Self {
        Self(
            ssh_key::PrivateKey::random(
                &mut ssh_key::rand_core::OsRng,
                ssh_key::Algorithm::Ed25519,
            )
            .expect("generate an ed25519 key"),
        )
    }

    /// A line of an allowed-signers file for this key with the given `principals` and, if
    /// given, the `namespaces` option.
    pub fn allowed_signers(&self, principals: &str, namespaces: Option<&str>) -> String {
        let key = self.0.public_key().to_openssh().expect("encode the key");
        match namespaces {
            Some(namespaces) => format!("{principals} namespaces=\"{namespaces}\" {key}\n"),
            None => format!("{principals} {key}\n"),
        }
    }

    /// `SHA256:…` fingerprint of the public key.
    pub fn fingerprint(&self) -> String {
        self.0.fingerprint(ssh_key::HashAlg::Sha256).to_string()
    }

    fn sign(&self, namespace: &str, payload: &[u8]) -> String {
        self.0
            .sign(namespace, ssh_key::HashAlg::Sha512, payload)
            .expect("sign")
            .to_pem(ssh_key::LineEnding::LF)
            .expect("armor")
    }
}

/// Like [`commit_files`], with an SSH signature of `signer` in the `gpgsig` header.
pub fn commit_files_signed(
    path: &Path,
    branch: &str,
    message: &str,
    files: &BTreeMap<String, Vec<u8>>,
    signer: &TestSigner,
) -> Oid {
    commit(path, branch, message, files, Some(signer))
}

/// Tags `commit` with an annotated tag that carries an SSH signature of `signer` (namespace
/// `git`, appended to the tag message as git does).
pub fn tag_signed(path: &Path, name: &str, commit: Oid, signer: &TestSigner) {
    let repository = Repository::open_bare(path).unwrap();
    let content = format!(
        "object {commit}\ntype commit\ntag {name}\ntagger test <test@example.com> 1700000000 +0000\n\n{name}\n"
    );
    let signature = signer.sign("git", content.as_bytes());
    let signed = format!("{content}{signature}");
    let id = repository
        .odb()
        .unwrap()
        .write(git2::ObjectType::Tag, signed.as_bytes())
        .unwrap();
    repository
        .reference(&format!("refs/tags/{name}"), id, false, "signed tag")
        .unwrap();
}

/// Creates (or opens) a bare repository at `path` and commits `files` (path → content) on
/// `branch` with the given parent chain; returns the commit id. `HEAD` points to `branch`.
pub fn commit_files(
    path: &Path,
    branch: &str,
    message: &str,
    files: &BTreeMap<String, Vec<u8>>,
) -> Oid {
    commit(path, branch, message, files, None)
}

fn commit(
    path: &Path,
    branch: &str,
    message: &str,
    files: &BTreeMap<String, Vec<u8>>,
    signer: Option<&TestSigner>,
) -> Oid {
    let repository = Repository::open_bare(path)
        .or_else(|_| Repository::init_bare(path))
        .unwrap();
    let parent = repository
        .find_reference(&format!("refs/heads/{branch}"))
        .ok()
        .and_then(|r| r.peel_to_commit().ok());
    let tree = build_tree(&repository, files);
    let signature = Signature::new(
        "test",
        "test@example.com",
        &git2::Time::new(1_700_000_000, 0),
    )
    .unwrap();
    let tree = repository.find_tree(tree).unwrap();
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let reference = format!("refs/heads/{branch}");
    let oid = match signer {
        None => repository
            .commit(
                Some(&reference),
                &signature,
                &signature,
                message,
                &tree,
                &parents,
            )
            .unwrap(),
        Some(signer) => {
            let buffer = repository
                .commit_create_buffer(&signature, &signature, message, &tree, &parents)
                .unwrap();
            let armored = signer.sign("git", &buffer);
            let oid = repository
                .commit_signed(std::str::from_utf8(&buffer).unwrap(), &armored, None)
                .unwrap();
            repository
                .reference(&reference, oid, true, "signed commit")
                .unwrap();
            oid
        }
    };
    repository
        .set_head(&format!("refs/heads/{branch}"))
        .unwrap();
    oid
}

/// Tags `commit` in the bare repository at `path`, annotated or lightweight.
pub fn tag(path: &Path, name: &str, commit: Oid, annotated: bool) {
    let repository = Repository::open_bare(path).unwrap();
    let object = repository.find_object(commit, None).unwrap();
    if annotated {
        let signature = Signature::new(
            "test",
            "test@example.com",
            &git2::Time::new(1_700_000_000, 0),
        )
        .unwrap();
        repository
            .tag(name, &object, &signature, name, false)
            .unwrap();
    } else {
        repository.tag_lightweight(name, &object, false).unwrap();
    }
}

/// Builds the tree for `files`, whose paths may contain `/`.
fn build_tree(repository: &Repository, files: &BTreeMap<String, Vec<u8>>) -> Oid {
    let mut builder = repository.treebuilder(None).unwrap();
    let mut subdirs: BTreeMap<&str, BTreeMap<String, Vec<u8>>> = BTreeMap::new();
    for (name, content) in files {
        match name.split_once('/') {
            Some((dir, rest)) => {
                subdirs
                    .entry(dir)
                    .or_default()
                    .insert(rest.to_owned(), content.clone());
            }
            None => {
                let blob = repository.blob(content).unwrap();
                builder.insert(name, blob, 0o100644).unwrap();
            }
        }
    }
    for (dir, children) in subdirs {
        let oid = build_tree(repository, &children);
        builder.insert(dir, oid, 0o040000).unwrap();
    }
    builder.write().unwrap()
}
