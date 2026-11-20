//! rustls configurations from the live source: the own SVID is presented through resolvers, the
//! peer is verified against the bundle of its trust domain (not the web PKI, not a host name) and
//! its SPIFFE ID is checked. Everything reads the source at handshake time.

use std::fmt;
use std::sync::Arc;

use rustls::client::ResolvesClientCert;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerifier};
use rustls::crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, DistinguishedName, Error, ServerConfig,
    SignatureScheme,
};

use crate::der::spiffe_id_of_certificate;
use crate::id::{SpiffeId, SpiffeMatch};
use crate::source::X509Source;

#[derive(Debug)]
struct Refusal(String);

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refusal {}

fn refuse(reason: impl Into<String>) -> Error {
    Error::InvalidCertificate(CertificateError::Other(rustls::OtherError(Arc::new(
        Refusal(reason.into()),
    ))))
}

/// Verifies `end_entity` against the bundle of `domain`; returns the certificate's SPIFFE ID.
fn verify_chain(
    source: &X509Source,
    algorithms: &WebPkiSupportedAlgorithms,
    end_entity: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
    now: UnixTime,
    server: bool,
) -> Result<SpiffeId, Error> {
    let id = spiffe_id_of_certificate(end_entity).map_err(refuse)?;
    let bundle = source.bundle(id.trust_domain()).ok_or_else(|| {
        refuse(format!(
            "no trust bundle for the trust domain {}",
            id.trust_domain()
        ))
    })?;
    let anchors: Vec<_> = bundle
        .iter()
        .filter_map(|certificate| webpki::anchor_from_trusted_cert(certificate).ok())
        .collect();
    let cert = webpki::EndEntityCert::try_from(end_entity)
        .map_err(|error| refuse(format!("certificate of {id}: {error}")))?;
    let usage = if server {
        webpki::KeyUsage::server_auth()
    } else {
        webpki::KeyUsage::client_auth()
    };
    cert.verify_for_usage(
        algorithms.all,
        &anchors,
        intermediates,
        now,
        usage,
        None,
        None,
    )
    .map_err(|error| refuse(format!("certificate of {id}: {error}")))?;
    Ok(id)
}

struct ServerVerifier {
    source: Arc<X509Source>,
    expected: SpiffeMatch,
    algorithms: WebPkiSupportedAlgorithms,
}

impl fmt::Debug for ServerVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerVerifier")
            .field("expected", &self.expected)
            .finish()
    }
}

impl ServerCertVerifier for ServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, Error> {
        let id = verify_chain(
            &self.source,
            &self.algorithms,
            end_entity,
            intermediates,
            now,
            true,
        )?;
        if !self.expected.accepts(&id) {
            return Err(refuse(format!(
                "the server is {id}, expected {}",
                self.expected
            )));
        }
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

struct ClientVerifier {
    source: Arc<X509Source>,
    /// Trust domains whose callers are accepted.
    accepted: Vec<String>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl fmt::Debug for ClientVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientVerifier")
            .field("accepted", &self.accepted)
            .finish()
    }
}

impl ClientCertVerifier for ClientVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        let id = verify_chain(
            &self.source,
            &self.algorithms,
            end_entity,
            intermediates,
            now,
            false,
        )?;
        if !self
            .accepted
            .iter()
            .any(|domain| domain == id.trust_domain())
        {
            return Err(refuse(format!(
                "the trust domain {} of {id} is not accepted",
                id.trust_domain()
            )));
        }
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// Presents the current SVID.
#[derive(Debug)]
struct LiveCertificate(Arc<X509Source>);

impl LiveCertificate {
    fn current(&self) -> Option<Arc<CertifiedKey>> {
        self.0.svid().map(|svid| svid.certified_key())
    }
}

impl fmt::Debug for X509Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("X509Source")
    }
}

impl ResolvesServerCert for LiveCertificate {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.current()
    }
}

impl ResolvesClientCert for LiveCertificate {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _sigschemes: &[SignatureScheme],
    ) -> Option<Arc<CertifiedKey>> {
        self.current()
    }

    fn has_certs(&self) -> bool {
        self.0.svid().is_some()
    }
}

/// A client configuration that presents the own SVID and accepts servers that `expected` names.
pub fn client_config(
    source: Arc<X509Source>,
    expected: SpiffeMatch,
) -> Result<ClientConfig, Error> {
    let provider = crate::provider();
    let verifier = ServerVerifier {
        source: Arc::clone(&source),
        expected,
        algorithms: provider.signature_verification_algorithms,
    };
    Ok(ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_client_cert_resolver(Arc::new(LiveCertificate(source))))
}

/// A server configuration with the own SVID that requires a client certificate of one of the
/// trust domains `accepted`.
pub fn server_config(
    source: Arc<X509Source>,
    accepted: Vec<String>,
) -> Result<ServerConfig, Error> {
    let provider = crate::provider();
    let verifier = ClientVerifier {
        source: Arc::clone(&source),
        accepted,
        algorithms: provider.signature_verification_algorithms,
    };
    Ok(ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(verifier))
        .with_cert_resolver(Arc::new(LiveCertificate(source))))
}

/// Why a TLS handshake failed, if `error` (or an error it wraps) is a refused handshake: the
/// reason of this crate's verifiers ("the server is …, expected …") or rustls' own message.
/// Clients use it to name the cause instead of reporting a missing connection.
pub fn tls_refusal(error: &(dyn std::error::Error + 'static)) -> Option<String> {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(tls) = error.downcast_ref::<Error>() {
            return Some(match tls {
                Error::InvalidCertificate(CertificateError::Other(other)) => other.0.to_string(),
                other => other.to_string(),
            });
        }
        // `std::io::Error` hides the error it wraps from `source()`; look inside.
        if let Some(inner) = error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            .and_then(|inner| tls_refusal(inner))
        {
            return Some(inner);
        }
        current = error.source();
    }
    None
}
