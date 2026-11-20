//! What an outbound HTTP client needs from the identity (ADR-0109): a rustls configuration for
//! mTLS and a fresh JWT-SVID as bearer, chosen by the spec's `auth.spiffe`.

use std::sync::Arc;

use crate::{Identity, SpiffeError, SpiffeMatch};

/// How the client authenticates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// X.509-SVID in the TLS handshake; the server is checked against the bundle.
    Mtls,
    /// JWT-SVID as `Authorization: Bearer`.
    Jwt,
}

/// The authentication of one outbound target.
#[derive(Clone)]
pub struct ClientAuth {
    identity: Arc<dyn Identity>,
    mode: AuthMode,
    expected: Option<SpiffeMatch>,
    audience: Option<String>,
}

impl std::fmt::Debug for ClientAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientAuth")
            .field("mode", &self.mode)
            .field("expected", &self.expected)
            .field("audience", &self.audience)
            .finish_non_exhaustive()
    }
}

impl ClientAuth {
    /// `mtls` needs `expected`; `jwt` needs `audience` (and takes `expected` to verify the TLS
    /// connection against the SPIFFE bundle instead of the web PKI).
    pub fn new(
        identity: Arc<dyn Identity>,
        mode: AuthMode,
        expected: Option<SpiffeMatch>,
        audience: Option<String>,
    ) -> Result<Self, SpiffeError> {
        match mode {
            AuthMode::Mtls if expected.is_none() => Err(SpiffeError::Invalid(
                "mode mtls needs id, ids or trust_domain".into(),
            )),
            AuthMode::Jwt if audience.is_none() => {
                Err(SpiffeError::Invalid("mode jwt needs an audience".into()))
            }
            _ => Ok(Self {
                identity,
                mode,
                expected,
                audience,
            }),
        }
    }

    /// The TLS configuration to give the HTTP client: with mTLS, and with JWT when the server's
    /// ID is to be checked. `None`: the web PKI as usual. The URL must then be `https://`.
    pub fn tls_config(&self) -> Result<Option<rustls::ClientConfig>, SpiffeError> {
        match &self.expected {
            Some(expected) => self.identity.client_tls(expected).map(Some),
            None => Ok(None),
        }
    }

    /// A fresh JWT-SVID for the audience (JWT mode), else `None`. A credential: never log it.
    pub async fn bearer(&self) -> Result<Option<String>, SpiffeError> {
        match (&self.mode, &self.audience) {
            (AuthMode::Jwt, Some(audience)) => self
                .identity
                .jwt_svid(audience)
                .await
                .map(|svid| Some(svid.token().to_owned())),
            _ => Ok(None),
        }
    }
}
