/// Why a registry access failed or an image could not be read.
#[derive(Debug, thiserror::Error)]
pub enum OciError {
    /// Network error or unusable answer of the registry.
    #[error("registry {registry}: {reason}")]
    Registry { registry: String, reason: String },
    /// The registry answered 404.
    #[error("{what} not found in {registry}")]
    NotFound { registry: String, what: String },
    /// The registry requires credentials that are missing or wrong.
    #[error(
        "registry {registry} refused access: set credentials in GRAUWOLF_REGISTRY_AUTH \
         (JSON map registry -> \"user:token\" or bearer token) or log in with the Docker \
         configuration ($DOCKER_CONFIG/config.json or ~/.docker/config.json: auths, \
         credHelpers, credsStore)"
    )]
    Unauthorized { registry: String },
    /// Content does not have the digest it was requested by or announced with.
    #[error("digest mismatch for {what}: expected {expected}, got {actual}")]
    DigestMismatch {
        what: String,
        expected: String,
        actual: String,
    },
    /// The image, its manifest or its layout directory is inconsistent.
    #[error("{0}")]
    Invalid(String),
    /// A conditional write found the manifest changed since it was read (`412`).
    #[error("{what} changed in {registry} since it was read")]
    Conflict { registry: String, what: String },
    /// File system error in an image layout.
    #[error("{path}: {reason}")]
    Io { path: String, reason: String },
}
