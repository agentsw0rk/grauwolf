//! Delegation (ADR-0111): the Txn-Token (claims, signing, verification), people's access tokens
//! from Entra behind a port, the signing key behind a port (in memory or OpenBao Transit), the
//! identity service's decisions and policy, and the agent's client of the identity service.

mod agent;
mod device;
mod entra;
mod grant;
mod grant_store;
mod jose;
mod keys;
mod people;
mod policy;
mod refresh;
mod service;
mod service_grants;
mod signer;
#[cfg(feature = "testing")]
pub mod testing;
mod txn;

pub use agent::{IdentityClient, RENEW_BEFORE, RemoteConfig, RunDelegation, TxnToken};
pub use device::{
    DEVICE_CODE_GRANT, DeviceCode, DeviceEndpoints, DeviceLogin, OAuthTokens, SLOW_DOWN_STEP,
};
pub use entra::{EntraSettings, EntraVerifier, Person, PersonTokenVerifier};
pub use grant::{
    DEFAULT_GRANT_VALIDITY, DEFAULT_MAX_RUN, GRANT_HEADER, GRANT_ID_PREFIX, GRANT_TYPE, Grant,
    GrantOwner, GrantRequest, GrantStore, MAX_GRANT_VALIDITY, MemoryGrantStore, is_grant_id,
};
pub use grant_store::{GRANT_TOKENS_PATH, GRANTS_PATH, OpenBaoGrantStore};
pub use jose::{Jwk, Jwks, peek_claims};
pub use keys::MISS_COOLDOWN;
pub use people::{CallAnswer, CallerCredential, GrantsClient, call_agent};
pub use policy::{IdentityPolicy, POLICY_RELOAD_INTERVAL};
pub use refresh::{PublicClientRefresh, Redeemed, RefreshRedeemer, TokenEndpoint};
pub use service::{IdentityService, ServiceSettings, TokenRequest, TokenResponse};
pub use service_grants::CreateGrant;
pub use signer::{MemorySigner, TRANSIT_KEYS_TTL, TokenSigner, TransitSigner};
pub use txn::{
    ACCESS_TOKEN_TYPE, DEFAULT_LIFETIME, LEEWAY_SECS, MAX_CHAIN_LIFETIME, TXN_TOKEN_TYP,
    TXN_TOKEN_TYPE, TxnClaims, TxnTokenVerifier, check, issue,
};

/// Why a token was not issued or not accepted. The text never contains a token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    /// The request is malformed (`400`).
    #[error("{0}")]
    BadRequest(String),
    /// The token is invalid, expired, or for another tenant, audience or trust domain (`401`).
    #[error("{0}")]
    Unauthorized(String),
    /// The caller may not do this (`403`).
    #[error("{0}")]
    Forbidden(String),
    /// There is no such thing for the caller (`404`), e.g. a grant of another person.
    #[error("{0}")]
    NotFound(String),
    /// A service this one needs cannot be reached (`503`): the issuer's keys, OpenBao, the
    /// identity service.
    #[error("{0}")]
    Unavailable(String),
}

impl IdentityError {
    /// The HTTP status of the error.
    pub fn status(&self) -> u16 {
        match self {
            Self::BadRequest(_) => 400,
            Self::Unauthorized(_) => 401,
            Self::Forbidden(_) => 403,
            Self::NotFound(_) => 404,
            Self::Unavailable(_) => 503,
        }
    }
}
