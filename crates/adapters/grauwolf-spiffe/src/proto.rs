//! The protobuf messages of `proto/workload.proto` (prost derive, no code generation).

/// `JWTSVIDRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct JwtSvidRequest {
    /// The audiences of the token.
    #[prost(string, repeated, tag = "1")]
    pub audience: Vec<String>,
    /// The SPIFFE ID to fetch for; empty selects every identity of the workload.
    #[prost(string, tag = "2")]
    pub spiffe_id: String,
}

/// `JWTSVIDResponse`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct JwtSvidResponse {
    /// One token per identity of the workload.
    #[prost(message, repeated, tag = "1")]
    pub svids: Vec<JwtSvidMessage>,
}

/// `JWTSVID`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct JwtSvidMessage {
    /// The SPIFFE ID of the token.
    #[prost(string, tag = "1")]
    pub spiffe_id: String,
    /// The compact JWT.
    #[prost(string, tag = "2")]
    pub svid: String,
    /// An operator hint.
    #[prost(string, tag = "3")]
    pub hint: String,
}

/// `X509SVIDRequest` (empty).
#[derive(Clone, PartialEq, prost::Message)]
pub struct X509SvidRequest {}

/// `X509SVIDResponse`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct X509SvidResponse {
    /// One SVID per identity of the workload.
    #[prost(message, repeated, tag = "1")]
    pub svids: Vec<X509SvidMessage>,
    /// Revocation lists (unused).
    #[prost(bytes = "vec", repeated, tag = "2")]
    pub crl: Vec<Vec<u8>>,
    /// Bundles of federated trust domains, by `spiffe://<domain>`, concatenated DER.
    #[prost(map = "string, bytes", tag = "3")]
    pub federated_bundles: std::collections::HashMap<String, Vec<u8>>,
}

/// `X509SVID`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct X509SvidMessage {
    /// The SPIFFE ID.
    #[prost(string, tag = "1")]
    pub spiffe_id: String,
    /// The certificate chain, concatenated DER, leaf first.
    #[prost(bytes = "vec", tag = "2")]
    pub x509_svid: Vec<u8>,
    /// The private key, PKCS#8 DER.
    #[prost(bytes = "vec", tag = "3")]
    pub x509_svid_key: Vec<u8>,
    /// The bundle of the own trust domain, concatenated DER.
    #[prost(bytes = "vec", tag = "4")]
    pub bundle: Vec<u8>,
    /// An operator hint.
    #[prost(string, tag = "5")]
    pub hint: String,
}

/// `X509BundlesRequest` (empty).
#[derive(Clone, PartialEq, prost::Message)]
pub struct X509BundlesRequest {}

/// `X509BundlesResponse`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct X509BundlesResponse {
    /// Revocation lists (unused).
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub crl: Vec<Vec<u8>>,
    /// Bundles by `spiffe://<domain>`, concatenated DER.
    #[prost(map = "string, bytes", tag = "2")]
    pub bundles: std::collections::HashMap<String, Vec<u8>>,
}

/// `ValidateJWTSVIDRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ValidateJwtSvidRequest {
    /// The audience the token must name.
    #[prost(string, tag = "1")]
    pub audience: String,
    /// The compact JWT.
    #[prost(string, tag = "2")]
    pub svid: String,
}

/// `ValidateJWTSVIDResponse` (the claims are not read).
#[derive(Clone, PartialEq, prost::Message)]
pub struct ValidateJwtSvidResponse {
    /// The SPIFFE ID of the token.
    #[prost(string, tag = "1")]
    pub spiffe_id: String,
}
