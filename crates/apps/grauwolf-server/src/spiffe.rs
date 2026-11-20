//! Callers with a SPIFFE identity (ADR-0109): the mapping file from SPIFFE IDs to users, the
//! gate that decides who is calling, and the mTLS listener of the second port.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use grauwolf_host::config::{Env, SpiffeServiceConfig};
use grauwolf_identity::IdentityClient;
use grauwolf_spiffe::{Identity, SpiffeError, SpiffeId, spiffe_id_of_certificate};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

/// How often the mapping file is checked for changes.
pub const RELOAD_INTERVAL: Duration = Duration::from_secs(2);

/// The ID of the peer of an accepted mTLS connection (the `Addr` of [`TlsListener`], so the
/// router sees it as `ConnectInfo<PeerSpiffe>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerSpiffe(pub SpiffeId);

/// The SPIFFE ID of the caller, in the request extensions after authentication (for spans and
/// events; never a token).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerSpiffe(pub SpiffeId);

#[derive(Debug, Default)]
struct Entries {
    exact: HashMap<String, String>,
    /// `(prefix without the trailing /*, user)`.
    prefixes: Vec<(String, String)>,
}

impl Entries {
    fn from_json(text: &str) -> Result<Self, String> {
        let map: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(text).map_err(|error| format!("callers file: {error}"))?;
        let mut entries = Self::default();
        for (key, value) in map {
            let user = value
                .as_str()
                .ok_or_else(|| format!("callers file: the entry for '{key}' is not a user name"))?;
            grauwolf_agents::memory::check_user_id(user)
                .map_err(|error| format!("callers file: {error}"))?;
            match key.strip_suffix("/*") {
                Some(prefix) => {
                    SpiffeId::parse(prefix).map_err(|error| format!("callers file: {error}"))?;
                    entries.prefixes.push((prefix.to_owned(), user.to_owned()));
                }
                None => {
                    SpiffeId::parse(&key).map_err(|error| format!("callers file: {error}"))?;
                    entries.exact.insert(key, user.to_owned());
                }
            }
        }
        Ok(entries)
    }

    /// The exact entry, else the longest prefix that matches whole segments.
    fn user(&self, id: &str) -> Option<&str> {
        if let Some(user) = self.exact.get(id) {
            return Some(user);
        }
        self.prefixes
            .iter()
            .filter(|(prefix, _)| {
                id.strip_prefix(prefix.as_str())
                    .is_some_and(|rest| rest.starts_with('/') && rest.len() > 1)
            })
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, user)| user.as_str())
    }
}

/// What identifies the file's content: the resolved path (a ConfigMap update swaps the `..data`
/// symlink, so the resolved path changes), modification time and size.
fn fingerprint(path: &Path) -> Option<(PathBuf, SystemTime, u64)> {
    let resolved = std::fs::canonicalize(path).ok()?;
    let meta = std::fs::metadata(&resolved).ok()?;
    Some((resolved, meta.modified().ok()?, meta.len()))
}

/// The mapping from SPIFFE IDs to users, read at start and again when the file changes.
pub struct CallerMap {
    entries: Arc<RwLock<Entries>>,
    task: Option<JoinHandle<()>>,
}

impl CallerMap {
    /// A map without entries: every authenticated caller is refused.
    pub fn empty() -> Self {
        Self {
            entries: Arc::default(),
            task: None,
        }
    }

    /// A fixed map from the JSON `text` (tests, no reload).
    pub fn from_json(text: &str) -> Result<Self, String> {
        Ok(Self {
            entries: Arc::new(RwLock::new(Entries::from_json(text)?)),
            task: None,
        })
    }

    /// Reads `path` now (an error stops the start) and checks it every `interval` for changes;
    /// a changed file that cannot be read or is invalid is reported and the old map stays.
    pub fn watch(path: &Path, interval: Duration) -> Result<Self, String> {
        let read = |path: &Path| {
            std::fs::read_to_string(path)
                .map_err(|error| format!("{}: {error}", path.display()))
                .and_then(|text| {
                    Entries::from_json(&text)
                        .map_err(|error| format!("{}: {error}", path.display()))
                })
        };
        let entries = Arc::new(RwLock::new(read(path)?));
        let (shared, path) = (Arc::clone(&entries), path.to_owned());
        let task = tokio::spawn(async move {
            let mut seen = fingerprint(&path);
            loop {
                tokio::time::sleep(interval).await;
                let now = fingerprint(&path);
                if now == seen {
                    continue;
                }
                match read(&path) {
                    Ok(fresh) => {
                        *shared.write().unwrap() = fresh;
                        seen = now;
                        eprintln!("grauwolf: callers file {} reloaded", path.display());
                    }
                    Err(error) => {
                        seen = now;
                        eprintln!(
                            "grauwolf: callers file not reloaded, keeping the old one: {error}"
                        );
                    }
                }
            }
        });
        Ok(Self {
            entries,
            task: Some(task),
        })
    }

    /// The user for `id`: the exact entry, else the longest matching prefix.
    pub fn user(&self, id: &SpiffeId) -> Option<String> {
        self.entries
            .read()
            .unwrap()
            .user(id.as_str())
            .map(str::to_owned)
    }
}

impl Drop for CallerMap {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// Decides who a caller with a SPIFFE identity is.
pub struct SpiffeGate {
    identity: Arc<dyn Identity>,
    callers: CallerMap,
    audience: String,
    /// Requests without any credential pass as anonymous on the HTTP port (no token and no
    /// users file are configured and anonymous access is allowed or the bind is local).
    pub(crate) anonymous: bool,
    /// The identity service (`GRAUWOLF_IDENTITY_URL`, ADR-0111): people's tokens are exchanged
    /// there at the entry, Txn-Tokens of callers are verified with its keys. Without it the
    /// agent accepts no delegation.
    delegation: Option<Arc<IdentityClient>>,
}

impl SpiffeGate {
    /// A gate that validates JWT-SVIDs for `audience` with `identity` and maps IDs with
    /// `callers`.
    pub fn new(
        identity: Arc<dyn Identity>,
        callers: CallerMap,
        audience: String,
        anonymous: bool,
    ) -> Self {
        Self {
            identity,
            callers,
            audience,
            anonymous,
            delegation: None,
        }
    }

    /// The same gate, accepting delegation through the identity service `client` (ADR-0111).
    pub fn delegating_to(mut self, client: Arc<IdentityClient>) -> Self {
        self.delegation = Some(client);
        self
    }

    /// The identity service, if the agent accepts delegation.
    pub(crate) fn delegation(&self) -> Option<&Arc<IdentityClient>> {
        self.delegation.as_ref()
    }

    /// The user for an authenticated ID, if it is mapped.
    pub fn user(&self, id: &SpiffeId) -> Option<String> {
        self.callers.user(id)
    }

    /// Validates a JWT-SVID through the Workload API.
    pub(crate) async fn validate(&self, token: &str) -> Result<SpiffeId, SpiffeError> {
        self.identity.validate_jwt_svid(token, &self.audience).await
    }
}

/// Everything inbound SPIFFE needs at start: the gate for both ports and the TLS configuration
/// of the second one.
pub struct SpiffeInbound {
    /// The port of the mTLS listener.
    pub port: u16,
    /// Who is calling.
    pub gate: Arc<SpiffeGate>,
    tls: Arc<tokio_rustls::rustls::ServerConfig>,
}

impl SpiffeInbound {
    /// Starts from the settings: connects to the Workload API (waiting for the first SVID up to
    /// the start-up deadline), reads the callers file. Fails with a clear message without
    /// `SPIFFE_ENDPOINT_SOCKET`.
    pub async fn start(
        settings: &SpiffeServiceConfig,
        env: &dyn Env,
        anonymous: bool,
    ) -> Result<Self, String> {
        let identity = grauwolf_host::spiffe::workload(env).await?.ok_or_else(|| {
            format!(
                "GRAUWOLF_SPIFFE is set, but {} is not: the service has no workload identity",
                grauwolf_spiffe::ENDPOINT_VARIABLE
            )
        })?;
        let own = identity
            .id()
            .ok_or("the workload identity has no SPIFFE ID")?;
        let callers = match &settings.users_file {
            Some(file) => CallerMap::watch(Path::new(file), RELOAD_INTERVAL)?,
            None => {
                eprintln!(
                    "grauwolf: GRAUWOLF_SPIFFE_USERS is not set: every caller with a SPIFFE ID is refused (403)"
                );
                CallerMap::empty()
            }
        };
        let mut accepted = vec![own.trust_domain().to_owned()];
        accepted.extend(settings.trust_domains.iter().cloned());
        accepted.dedup();
        let tls = identity
            .server_tls(&accepted)
            .map_err(|error| error.to_string())?;
        let audience = settings.audience.clone().unwrap_or_else(|| own.to_string());
        // Delegation (ADR-0111): the identity service, called with this workload identity.
        let delegation = grauwolf_host::identity::agent_client(settings, Arc::clone(&identity))?;
        let mut gate = SpiffeGate::new(identity, callers, audience, anonymous);
        if let Some(client) = delegation {
            eprintln!("grauwolf: delegation through the identity service {client:?}");
            gate = gate.delegating_to(client);
        }
        Ok(Self {
            port: settings.port,
            gate: Arc::new(gate),
            tls: Arc::new(tls),
        })
    }

    /// The same, accepting delegation through the identity service `client` (ADR-0111; tests –
    /// [`start`](Self::start) builds the client from `GRAUWOLF_IDENTITY_URL`).
    pub fn delegating_to(mut self, client: Arc<IdentityClient>) -> Self {
        Arc::get_mut(&mut self.gate)
            .expect("the gate is not shared before the service starts")
            .delegation = Some(client);
        self
    }

    /// The same, from an identity at hand (tests).
    pub fn with_identity(
        port: u16,
        identity: Arc<dyn Identity>,
        callers: CallerMap,
        audience: Option<String>,
        accepted: &[String],
        anonymous: bool,
    ) -> Result<Self, String> {
        let own = identity.id().ok_or("the identity has no SPIFFE ID")?;
        let tls = identity
            .server_tls(accepted)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            port,
            gate: Arc::new(SpiffeGate::new(
                identity,
                callers,
                audience.unwrap_or_else(|| own.to_string()),
                anonymous,
            )),
            tls: Arc::new(tls),
        })
    }

    /// Serves `router` on `listener` over mTLS until `shutdown` completes. The router must
    /// already have the authentication layer ([`crate::with_spiffe_auth`]); the caller's ID
    /// comes from the handshake.
    pub async fn serve(
        &self,
        listener: TcpListener,
        router: axum::Router,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Result<(), String> {
        let listener = TlsListener::new(listener, Arc::clone(&self.tls));
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<PeerSpiffe>(),
        )
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|error| error.to_string())
    }
}

/// Accepts TCP connections, does the TLS handshakes concurrently (a slow client does not block
/// the others) and hands out the finished connections with the caller's SPIFFE ID.
pub struct TlsListener {
    ready: mpsc::Receiver<(TlsStream<tokio::net::TcpStream>, PeerSpiffe)>,
    local: SocketAddr,
    task: JoinHandle<()>,
}

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

impl TlsListener {
    fn new(tcp: TcpListener, config: Arc<tokio_rustls::rustls::ServerConfig>) -> Self {
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
                        tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await
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
        // There is no ID for the listener itself; the address is not used by axum::serve.
        Err(std::io::Error::other(format!(
            "mTLS listener on {}",
            self.local
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(json: &str) -> CallerMap {
        CallerMap::from_json(json).unwrap()
    }

    fn id(text: &str) -> SpiffeId {
        SpiffeId::parse(text).unwrap()
    }

    #[test]
    fn the_exact_entry_wins_over_a_prefix() {
        let callers = map(
            r#"{ "spiffe://d.de/ns/batch/*": "batch-jobs", "spiffe://d.de/ns/batch/sa/special": "special" }"#,
        );
        assert_eq!(
            callers
                .user(&id("spiffe://d.de/ns/batch/sa/special"))
                .as_deref(),
            Some("special")
        );
        assert_eq!(
            callers
                .user(&id("spiffe://d.de/ns/batch/sa/other"))
                .as_deref(),
            Some("batch-jobs")
        );
    }

    #[test]
    fn the_longest_prefix_wins_and_prefixes_match_whole_segments() {
        let callers =
            map(r#"{ "spiffe://d.de/ns/*": "all", "spiffe://d.de/ns/batch/*": "batch" }"#);
        assert_eq!(
            callers.user(&id("spiffe://d.de/ns/batch/sa/x")).as_deref(),
            Some("batch")
        );
        assert_eq!(
            callers.user(&id("spiffe://d.de/ns/other/sa/x")).as_deref(),
            Some("all")
        );
        let callers = map(r#"{ "spiffe://d.de/ns/batch/*": "batch" }"#);
        assert_eq!(callers.user(&id("spiffe://d.de/ns/batchy/sa/x")), None);
        assert_eq!(
            callers.user(&id("spiffe://d.de/ns/batch")),
            None,
            "the prefix itself is not covered"
        );
        assert_eq!(callers.user(&id("spiffe://e.de/ns/batch/sa/x")), None);
    }

    #[test]
    fn invalid_files_are_refused() {
        for bad in [
            r#"{ "https://x/y": "u" }"#,
            r#"{ "spiffe://d.de/a": 1 }"#,
            "[]",
        ] {
            assert!(CallerMap::from_json(bad).is_err(), "{bad}");
        }
    }
}
