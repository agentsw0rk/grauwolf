//! Test doubles: tokens and X.509-SVIDs made at test time (a [`TestCa`]) and a Workload API server
//! on a Unix socket. No SPIRE needed. The tokens are shaped like JWT-SVIDs (claims `sub`, `aud`,
//! `exp`) with a placeholder signature that only the fakes in this module accept.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::{Buf, BytesMut};
use futures_util::future::BoxFuture;
use prost::Message;
use tokio::net::UnixListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::proto::{
    JwtSvidMessage, JwtSvidRequest, JwtSvidResponse, ValidateJwtSvidRequest,
    ValidateJwtSvidResponse, X509BundlesResponse, X509SvidMessage, X509SvidResponse,
};
use crate::{
    JwtSvid, SpiffeError, SpiffeId, SpiffeMatch, TlsIdentity, WorkloadIdentity, X509Source,
};

pub use crate::test_ca::TestCa;

const SIGNATURE: &str = "test-signature-not-verifiable";

/// The seconds since the epoch.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// What the Workload API's `ValidateJWTSVID` does with a test token: the placeholder signature,
/// the audience and the expiry are checked.
pub fn validate_test_token(token: &str, audience: &str) -> Result<SpiffeId, String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 || parts[2] != URL_SAFE_NO_PAD.encode(SIGNATURE) {
        return Err("signature verification failed".into());
    }
    let claims: serde_json::Value = URL_SAFE_NO_PAD
        .decode(parts[1])
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or("malformed token")?;
    if claims["exp"].as_u64().is_none_or(|exp| exp <= now_secs()) {
        return Err("token has expired".into());
    }
    let audiences = claims["aud"].as_array().cloned().unwrap_or_default();
    if !audiences.iter().any(|a| a == audience) {
        return Err(format!("expected audience in [{audience}]"));
    }
    SpiffeId::parse(claims["sub"].as_str().unwrap_or_default()).map_err(|e| e.to_string())
}

/// A token for `spiffe_id` and `audience` that expires `lifetime_secs` after `now_secs`.
pub fn test_token(spiffe_id: &str, audience: &str, now_secs: u64, lifetime_secs: u64) -> String {
    let part = |value: serde_json::Value| URL_SAFE_NO_PAD.encode(value.to_string());
    format!(
        "{}.{}.{}",
        part(serde_json::json!({"alg": "ES256", "typ": "JWT", "kid": "test"})),
        part(serde_json::json!({
            "sub": spiffe_id,
            "aud": [audience],
            "iat": now_secs,
            "exp": now_secs + lifetime_secs,
        })),
        URL_SAFE_NO_PAD.encode(SIGNATURE),
    )
}

/// The claims of a token made by [`test_token`].
pub fn test_token_claims(token: &str) -> serde_json::Value {
    let payload = token.split('.').nth(1).expect("a JWT has three parts");
    let bytes = URL_SAFE_NO_PAD.decode(payload).expect("base64url payload");
    serde_json::from_slice(&bytes).expect("JSON claims")
}

/// An in-process identity: issues test tokens, validates them, and has an X.509-SVID from a
/// [`TestCa`] for the TLS methods of the port.
#[derive(Clone)]
pub struct FakeIdentity {
    spiffe_id: String,
    audiences: Arc<Mutex<Vec<String>>>,
    fail: Arc<Mutex<Option<String>>>,
    ca: TestCa,
    source: Arc<X509Source>,
}

impl std::fmt::Debug for FakeIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeIdentity")
            .field("spiffe_id", &self.spiffe_id)
            .finish()
    }
}

impl FakeIdentity {
    /// An identity with `spiffe_id`, in a trust domain with a CA of its own.
    pub fn new(spiffe_id: &str) -> Self {
        let domain = SpiffeId::parse(spiffe_id).expect("a valid SPIFFE ID");
        Self::with_ca(&TestCa::new(domain.trust_domain()), spiffe_id)
    }

    /// An identity with `spiffe_id` issued by `ca` (share a CA to let identities trust each
    /// other).
    pub fn with_ca(ca: &TestCa, spiffe_id: &str) -> Self {
        let source = X509Source::manual();
        source.set_svid(ca.issue(spiffe_id));
        source.set_bundle(ca.domain(), ca.bundle());
        Self {
            spiffe_id: spiffe_id.to_owned(),
            audiences: Arc::default(),
            fail: Arc::default(),
            ca: ca.clone(),
            source: Arc::new(source),
        }
    }

    /// An identity whose certificate carries `id` and further URI SANs (`extra`), which a
    /// verifier must refuse.
    pub fn with_extra_uris(ca: &TestCa, id: &str, extra: &[&str]) -> Self {
        let identity = Self::with_ca(ca, id);
        let uris: Vec<&str> = std::iter::once(id).chain(extra.iter().copied()).collect();
        identity
            .source
            .set_svid(ca.issue_with_uris(id, &uris, 86_400));
        identity
    }

    /// Also trusts the bundle of another CA (a federated trust domain).
    pub fn trust(&self, ca: &TestCa) {
        self.source.set_bundle(ca.domain(), ca.bundle());
    }

    /// Issues a new certificate for the same ID (a rotation); new handshakes use it.
    pub fn rotate(&self) {
        self.source.set_svid(self.ca.issue(&self.spiffe_id));
    }

    /// Swaps the SVID for one that has already expired.
    pub fn expire(&self) {
        self.source
            .set_svid(self.ca.issue_valid_for(&self.spiffe_id, -60));
    }

    /// The DER of the certificate presented now.
    pub fn certificate(&self) -> Vec<u8> {
        self.source.svid().unwrap().certified_key().cert[0]
            .as_ref()
            .to_vec()
    }

    /// Every following token call fails with `reason` (`None`: works again).
    pub fn fail_with(&self, reason: Option<&str>) {
        *self.fail.lock().unwrap() = reason.map(str::to_owned);
    }

    /// The audiences requested so far, in order.
    pub fn audiences(&self) -> Vec<String> {
        self.audiences.lock().unwrap().clone()
    }
}

impl WorkloadIdentity for FakeIdentity {
    fn jwt_svid<'a>(&'a self, audience: &'a str) -> BoxFuture<'a, Result<JwtSvid, SpiffeError>> {
        Box::pin(async move {
            self.audiences.lock().unwrap().push(audience.to_owned());
            if let Some(reason) = self.fail.lock().unwrap().clone() {
                return Err(SpiffeError::Unavailable {
                    endpoint: "fake".into(),
                    reason,
                });
            }
            Ok(JwtSvid::new(
                test_token(&self.spiffe_id, audience, now_secs(), 300),
                self.spiffe_id.clone(),
            ))
        })
    }

    fn validate_jwt_svid<'a>(
        &'a self,
        token: &'a str,
        audience: &'a str,
    ) -> BoxFuture<'a, Result<SpiffeId, SpiffeError>> {
        Box::pin(async move {
            validate_test_token(token, audience).map_err(|message| SpiffeError::Refused {
                endpoint: "fake".into(),
                status: 3,
                message,
            })
        })
    }
}

impl TlsIdentity for FakeIdentity {
    fn id(&self) -> Option<SpiffeId> {
        SpiffeId::parse(&self.spiffe_id).ok()
    }

    fn client_tls(&self, expected: &SpiffeMatch) -> Result<rustls::ClientConfig, SpiffeError> {
        crate::tls::client_config(Arc::clone(&self.source), expected.clone())
            .map_err(|error| SpiffeError::Invalid(error.to_string()))
    }

    fn server_tls(&self, accepted: &[String]) -> Result<rustls::ServerConfig, SpiffeError> {
        crate::tls::server_config(Arc::clone(&self.source), accepted.to_vec())
            .map_err(|error| SpiffeError::Invalid(error.to_string()))
    }
}

struct State {
    spiffe_id: String,
    ca: TestCa,
    svid: Mutex<(Vec<u8>, Vec<u8>)>,
    extra_bundles: Mutex<HashMap<String, Vec<u8>>>,
    /// Bumped on every change of SVID or bundles.
    version: watch::Sender<u64>,
    /// Bumped to end the open streams.
    kick: watch::Sender<u64>,
    denials: AtomicUsize,
    jwt_requests: Mutex<Vec<Vec<String>>>,
    validations: Mutex<Vec<String>>,
    streams_opened: AtomicUsize,
}

/// A Workload API server on a Unix socket: JWT-SVIDs (test tokens), their validation, and
/// X.509-SVIDs and bundles as streams from a [`TestCa`].
pub struct FakeWorkloadApi {
    socket: PathBuf,
    state: Arc<State>,
    task: JoinHandle<()>,
}

impl FakeWorkloadApi {
    /// Listens on `socket` and issues identities for `spiffe_id`. Calls without the
    /// `workload.spiffe.io: true` header are refused, as the real API does. With
    /// `registered: false` every call is answered with `PERMISSION_DENIED`, like a workload
    /// without an entry.
    pub fn start(socket: &Path, spiffe_id: &str, registered: bool) -> Self {
        Self::start_denying(socket, spiffe_id, if registered { 0 } else { usize::MAX })
    }

    /// Like [`start`](Self::start), but the first `denials` calls are answered with
    /// `PERMISSION_DENIED` ("no identity issued"), as the real API does until the registration
    /// entry of a new pod has been synced; then it issues tokens.
    pub fn start_denying(socket: &Path, spiffe_id: &str, denials: usize) -> Self {
        let domain = SpiffeId::parse(spiffe_id).expect("a valid SPIFFE ID");
        Self::start_with_ca(
            socket,
            spiffe_id,
            denials,
            &TestCa::new(domain.trust_domain()),
        )
    }

    /// Like [`start_denying`](Self::start_denying) with the identity issued by `ca`.
    pub fn start_with_ca(socket: &Path, spiffe_id: &str, denials: usize, ca: &TestCa) -> Self {
        let listener = UnixListener::bind(socket).expect("bind the fake Workload API socket");
        let state = Arc::new(State {
            spiffe_id: spiffe_id.to_owned(),
            ca: ca.clone(),
            svid: Mutex::new(ca.issue_raw(spiffe_id)),
            extra_bundles: Mutex::default(),
            version: watch::channel(0).0,
            kick: watch::channel(0).0,
            denials: AtomicUsize::new(denials),
            jwt_requests: Mutex::default(),
            validations: Mutex::default(),
            streams_opened: AtomicUsize::new(0),
        });
        let shared = Arc::clone(&state);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let state = Arc::clone(&shared);
                tokio::spawn(async move {
                    let Ok(mut connection) = h2::server::handshake(stream).await else {
                        return;
                    };
                    while let Some(Ok((request, respond))) = connection.accept().await {
                        tokio::spawn(serve(request, respond, Arc::clone(&state)));
                    }
                });
            }
        });
        Self {
            socket: socket.to_owned(),
            state,
            task,
        }
    }

    /// The endpoint as `SPIFFE_ENDPOINT_SOCKET` would hold it.
    pub fn endpoint(&self) -> String {
        format!("unix://{}", self.socket.display())
    }

    /// The audiences of the JWT-SVID requests received so far.
    pub fn requests(&self) -> Vec<Vec<String>> {
        self.state.jwt_requests.lock().unwrap().clone()
    }

    /// The audiences of the `ValidateJWTSVID` calls so far.
    pub fn validations(&self) -> Vec<String> {
        self.state.validations.lock().unwrap().clone()
    }

    /// How many X.509 streams were opened so far (to see a reconnect).
    pub fn streams_opened(&self) -> usize {
        self.state.streams_opened.load(Ordering::SeqCst)
    }

    /// The CA that issues the identity and is the bundle of its trust domain.
    pub fn ca(&self) -> &TestCa {
        &self.state.ca
    }

    /// Issues a new certificate and pushes it to the open streams (a rotation).
    pub fn rotate(&self) {
        *self.state.svid.lock().unwrap() = self.state.ca.issue_raw(&self.state.spiffe_id);
        self.state.version.send_modify(|n| *n += 1);
    }

    /// Adds the bundle of another trust domain (federation) and pushes it.
    pub fn add_bundle(&self, ca: &TestCa) {
        let der = ca.bundle().concat();
        self.state
            .extra_bundles
            .lock()
            .unwrap()
            .insert(format!("spiffe://{}", ca.domain()), der);
        self.state.version.send_modify(|n| *n += 1);
    }

    /// Ends every open stream; clients reconnect.
    pub fn disconnect_streams(&self) {
        self.state.kick.send_modify(|n| *n += 1);
    }
}

impl Drop for FakeWorkloadApi {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn framed(message: &impl Message) -> bytes::Bytes {
    let encoded = message.encode_to_vec();
    let mut framed = BytesMut::new();
    framed.extend_from_slice(&[0]);
    framed.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
    framed.extend_from_slice(&encoded);
    framed.freeze()
}

fn x509_response(state: &State) -> X509SvidResponse {
    let (chain, key) = state.svid.lock().unwrap().clone();
    X509SvidResponse {
        svids: vec![X509SvidMessage {
            spiffe_id: state.spiffe_id.clone(),
            x509_svid: chain,
            x509_svid_key: key,
            bundle: state.ca.bundle().concat(),
            hint: String::new(),
        }],
        crl: vec![],
        federated_bundles: state.extra_bundles.lock().unwrap().clone(),
    }
}

fn bundles_response(state: &State) -> X509BundlesResponse {
    let mut bundles = state.extra_bundles.lock().unwrap().clone();
    bundles.insert(
        format!("spiffe://{}", state.ca.domain()),
        state.ca.bundle().concat(),
    );
    X509BundlesResponse {
        crl: vec![],
        bundles,
    }
}

/// Sends `make()` now and after every change until the client goes away or the streams are
/// kicked.
async fn stream_changes<M: Message>(
    mut send: h2::SendStream<bytes::Bytes>,
    state: Arc<State>,
    make: impl Fn(&State) -> M,
) {
    state.streams_opened.fetch_add(1, Ordering::SeqCst);
    let mut version = state.version.subscribe();
    let mut kick = state.kick.subscribe();
    loop {
        if send.send_data(framed(&make(&state)), false).is_err() {
            return;
        }
        tokio::select! {
            changed = version.changed() => if changed.is_err() { return },
            _ = kick.changed() => {
                let mut trailers = http::HeaderMap::new();
                trailers.insert("grpc-status", "14".parse().unwrap());
                let _ = send.send_trailers(trailers);
                return;
            }
        }
    }
}

async fn serve(
    request: http::Request<h2::RecvStream>,
    mut respond: h2::server::SendResponse<bytes::Bytes>,
    state: Arc<State>,
) {
    let marked = request
        .headers()
        .get("workload.spiffe.io")
        .is_some_and(|value| value == "true");
    let path = request.uri().path().to_owned();
    let mut body = request.into_body();
    let mut data = BytesMut::new();
    while let Some(Ok(chunk)) = body.data().await {
        let _ = body.flow_control().release_capacity(chunk.len());
        data.extend_from_slice(&chunk);
    }
    let failure = if !marked {
        Some((3, "security header missing from request"))
    } else if !path.starts_with("/SpiffeWorkloadAPI/") {
        Some((12, "unimplemented"))
    } else if take_denial(&state.denials) {
        Some((7, "no identity issued"))
    } else {
        None
    };
    let reply = |status: u32, message: &str| {
        http::Response::builder()
            .status(200)
            .header("content-type", "application/grpc")
            .header("grpc-status", status.to_string())
            .header("grpc-message", message.to_owned())
            .body(())
            .unwrap()
    };
    if let Some((status, message)) = failure {
        let _ = respond.send_response(reply(status, message), true);
        return;
    }
    let head = http::Response::builder()
        .status(200)
        .header("content-type", "application/grpc")
        .body(())
        .unwrap();
    if data.len() >= 5 {
        data.advance(5);
    }
    let data = data.freeze();
    let answer: Option<bytes::Bytes> = match path.as_str() {
        "/SpiffeWorkloadAPI/FetchJWTSVID" => {
            let Ok(parsed) = JwtSvidRequest::decode(data) else {
                let _ = respond.send_response(reply(3, "bad request"), true);
                return;
            };
            state
                .jwt_requests
                .lock()
                .unwrap()
                .push(parsed.audience.clone());
            let audience = parsed.audience.first().cloned().unwrap_or_default();
            Some(framed(&JwtSvidResponse {
                svids: vec![JwtSvidMessage {
                    spiffe_id: state.spiffe_id.clone(),
                    svid: test_token(&state.spiffe_id, &audience, now_secs(), 300),
                    hint: String::new(),
                }],
            }))
        }
        "/SpiffeWorkloadAPI/ValidateJWTSVID" => {
            let Ok(parsed) = ValidateJwtSvidRequest::decode(data) else {
                let _ = respond.send_response(reply(3, "bad request"), true);
                return;
            };
            state
                .validations
                .lock()
                .unwrap()
                .push(parsed.audience.clone());
            match validate_test_token(&parsed.svid, &parsed.audience) {
                Ok(id) => Some(framed(&ValidateJwtSvidResponse {
                    spiffe_id: id.to_string(),
                })),
                Err(message) => {
                    let _ = respond.send_response(reply(3, &message), true);
                    return;
                }
            }
        }
        "/SpiffeWorkloadAPI/FetchX509SVID" | "/SpiffeWorkloadAPI/FetchX509Bundles" => {
            let Ok(send) = respond.send_response(head, false) else {
                return;
            };
            if path.ends_with("SVID") {
                stream_changes(send, state, x509_response).await;
            } else {
                stream_changes(send, state, bundles_response).await;
            }
            return;
        }
        _ => {
            let _ = respond.send_response(reply(12, "unimplemented"), true);
            return;
        }
    };
    let Ok(mut send) = respond.send_response(head, false) else {
        return;
    };
    let _ = send.send_data(answer.unwrap_or_default(), false);
    let mut trailers = http::HeaderMap::new();
    trailers.insert("grpc-status", "0".parse().unwrap());
    let _ = send.send_trailers(trailers);
}

/// Uses up one denial; `usize::MAX` denies forever. `false` when none is left.
fn take_denial(denials: &AtomicUsize) -> bool {
    let mut left = denials.load(Ordering::SeqCst);
    loop {
        match left {
            0 => return false,
            usize::MAX => return true,
            _ => match denials.compare_exchange(left, left - 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return true,
                Err(current) => left = current,
            },
        }
    }
}
