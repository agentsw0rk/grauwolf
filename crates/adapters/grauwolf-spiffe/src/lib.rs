//! The workload identity of the agent (ADR-0109): the port [`WorkloadIdentity`] and the Workload
//! API client behind it: JWT-SVIDs and their validation, and the X.509-SVID and trust bundles as
//! a live source for mTLS with rustls.

mod auth;
mod client;
mod der;
mod id;
mod proto;
mod source;
#[cfg(feature = "testing")]
mod test_ca;
#[cfg(feature = "testing")]
pub mod testing;
mod tls;

use std::fmt;
use std::sync::Arc;

use futures_util::future::BoxFuture;

pub use auth::{AuthMode, ClientAuth};
pub use client::{ENDPOINT_VARIABLE, FETCH_TIMEOUT_VARIABLE, WorkloadApiClient};
pub use der::spiffe_id_of_certificate;
pub use id::{IdError, SpiffeId, SpiffeMatch, validate_trust_domain};
pub use source::{X509Source, X509Svid};
pub use tls::tls_refusal;

/// The crypto provider of the TLS configurations (the one `reqwest` and `rustls` use).
pub(crate) fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// A JWT-SVID: the compact token and the SPIFFE ID it names.
#[derive(Clone, PartialEq, Eq)]
pub struct JwtSvid {
    token: String,
    spiffe_id: String,
}

impl JwtSvid {
    /// A token for `spiffe_id`.
    pub fn new(token: impl Into<String>, spiffe_id: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            spiffe_id: spiffe_id.into(),
        }
    }

    /// The compact JWT. A credential: never log it.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The SPIFFE ID of the token (`spiffe://example.org/ns/agents/sa/reviewer`), for logs and
    /// errors.
    pub fn spiffe_id(&self) -> &str {
        &self.spiffe_id
    }
}

impl fmt::Debug for JwtSvid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwtSvid")
            .field("spiffe_id", &self.spiffe_id)
            .finish_non_exhaustive()
    }
}

/// Why an identity could not be fetched. Never contains a token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpiffeError {
    /// `SPIFFE_ENDPOINT_SOCKET` is not a `unix:` address.
    #[error("{ENDPOINT_VARIABLE}: '{0}' is not a unix socket address (unix:///path)")]
    BadEndpoint(String),
    /// `SPIFFE_FETCH_TIMEOUT` is not a duration.
    #[error("{FETCH_TIMEOUT_VARIABLE}: '{0}' is not a duration like 250ms, 30s or 1m")]
    BadTimeout(String),
    /// The identity was still not available when the start-up deadline passed.
    #[error(
        "{last} (gave up after waiting {waited}; the pod's registration entry may be missing or not synced yet)"
    )]
    GaveUp {
        /// How long it waited.
        waited: String,
        /// The last error.
        last: Box<SpiffeError>,
    },
    /// A value from the Workload API or the caller is not usable (a bad certificate, an ID).
    #[error("{0}")]
    Invalid(String),
    /// The Workload API could not be reached or answered wrongly.
    #[error("workload API at {endpoint}: {reason}")]
    Unavailable {
        /// The socket.
        endpoint: String,
        /// What went wrong.
        reason: String,
    },
    /// The Workload API refused (gRPC status other than OK), e.g. the workload is not registered.
    #[error("workload API at {endpoint}: status {status}: {message}")]
    Refused {
        /// The socket.
        endpoint: String,
        /// The gRPC status code.
        status: u32,
        /// The status message.
        message: String,
    },
}

impl SpiffeError {
    /// Did the Workload API reject a token as invalid (bad signature, expired, wrong audience)?
    pub fn is_invalid_token(&self) -> bool {
        matches!(self, Self::Refused { status: 3 | 16, .. })
    }
}

/// JWT-SVIDs (ADR-0109): fetching one for an audience and validating a received one.
pub trait WorkloadIdentity: Send + Sync {
    /// A JWT-SVID for `audience`.
    fn jwt_svid<'a>(&'a self, audience: &'a str) -> BoxFuture<'a, Result<JwtSvid, SpiffeError>>;

    /// Validates a received JWT-SVID for `audience` (signature, expiry, audience: done by the
    /// SPIRE agent) and returns the SPIFFE ID it names. A rejected token gives an error for
    /// which [`SpiffeError::is_invalid_token`] is true.
    fn validate_jwt_svid<'a>(
        &'a self,
        token: &'a str,
        audience: &'a str,
    ) -> BoxFuture<'a, Result<SpiffeId, SpiffeError>>;
}

/// The X.509 side (ADR-0109): rustls configurations from the live source.
pub trait TlsIdentity: Send + Sync {
    /// The own SPIFFE ID.
    fn id(&self) -> Option<SpiffeId>;

    /// A client configuration that presents the own SVID and accepts a server only if its
    /// certificate chains to the bundle of its trust domain and its SPIFFE ID satisfies
    /// `expected`. Rotations are picked up by new handshakes.
    fn client_tls(&self, expected: &SpiffeMatch) -> Result<rustls::ClientConfig, SpiffeError>;

    /// A server configuration with the own SVID that **requires** a client certificate of one
    /// of the trust domains `accepted`. Read the caller's ID from the handshake with
    /// [`spiffe_id_of_certificate`].
    fn server_tls(&self, accepted: &[String]) -> Result<rustls::ServerConfig, SpiffeError>;
}

/// Everything the port offers; what the host and the server hold.
pub trait Identity: WorkloadIdentity + TlsIdentity {}

impl<T: WorkloadIdentity + TlsIdentity> Identity for T {}

/// The identity at a Workload API: the client for JWT-SVIDs plus the live X.509 source.
pub struct SpiffeWorkload {
    client: WorkloadApiClient,
    source: Arc<X509Source>,
}

impl SpiffeWorkload {
    /// Starts the X.509 streams and waits (up to the client's fetch timeout) for the first SVID
    /// and bundle.
    pub async fn connect(client: WorkloadApiClient) -> Result<Self, SpiffeError> {
        let source = Arc::new(X509Source::start(&client).await?);
        Ok(Self { client, source })
    }

    /// The live source.
    pub fn source(&self) -> &Arc<X509Source> {
        &self.source
    }
}

impl WorkloadIdentity for SpiffeWorkload {
    fn jwt_svid<'a>(&'a self, audience: &'a str) -> BoxFuture<'a, Result<JwtSvid, SpiffeError>> {
        self.client.jwt_svid(audience)
    }

    fn validate_jwt_svid<'a>(
        &'a self,
        token: &'a str,
        audience: &'a str,
    ) -> BoxFuture<'a, Result<SpiffeId, SpiffeError>> {
        self.client.validate_jwt_svid(token, audience)
    }
}

impl TlsIdentity for SpiffeWorkload {
    fn id(&self) -> Option<SpiffeId> {
        self.source.svid().map(|svid| svid.id().clone())
    }

    fn client_tls(&self, expected: &SpiffeMatch) -> Result<rustls::ClientConfig, SpiffeError> {
        tls::client_config(Arc::clone(&self.source), expected.clone())
            .map_err(|error| SpiffeError::Invalid(error.to_string()))
    }

    fn server_tls(&self, accepted: &[String]) -> Result<rustls::ServerConfig, SpiffeError> {
        tls::server_config(Arc::clone(&self.source), accepted.to_vec())
            .map_err(|error| SpiffeError::Invalid(error.to_string()))
    }
}
