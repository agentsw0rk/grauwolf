use grauwolf_oci::OciError;

/// Why a package could not be fetched, verified or resolved.
#[derive(Debug, thiserror::Error)]
pub enum PackageFetchError {
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
    /// Media type of the manifest, config or a layer is not the one of a tool package.
    #[error("not a grauwolf tool package: {0}")]
    WrongMediaType(String),
    /// The package contents are inconsistent (missing module, broken manifest, ...).
    #[error("invalid tool package: {0}")]
    InvalidPackage(String),
    /// Fetching or building a git source failed; the text never contains the token.
    #[error("git source `{entry}`: {reason}")]
    Git { entry: String, reason: String },
    /// The tool policy refuses the package, or its signature does not satisfy it.
    #[error("{0}")]
    Policy(String),
    /// The lockfile and the spec or the registry disagree.
    #[error("{0}")]
    Lock(String),
    /// File system error in the cache or the lockfile.
    #[error("{path}: {reason}")]
    Io { path: String, reason: String },
}

impl From<grauwolf_tools::PackageError> for PackageFetchError {
    fn from(error: grauwolf_tools::PackageError) -> Self {
        Self::InvalidPackage(error.to_string())
    }
}

impl From<OciError> for PackageFetchError {
    fn from(error: OciError) -> Self {
        match error {
            OciError::Registry { registry, reason } => Self::Registry { registry, reason },
            OciError::NotFound { registry, what } => Self::NotFound { registry, what },
            OciError::Unauthorized { registry } => Self::Unauthorized { registry },
            OciError::DigestMismatch {
                what,
                expected,
                actual,
            } => Self::DigestMismatch {
                what,
                expected,
                actual,
            },
            OciError::Invalid(reason) => Self::InvalidPackage(reason),
            OciError::Conflict { registry, what } => Self::Registry {
                registry,
                reason: format!("{what} changed since it was read"),
            },
            OciError::Io { path, reason } => Self::Io { path, reason },
        }
    }
}
