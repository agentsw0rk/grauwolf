//! Credentials from files and from a secret broker behind the central configuration
//! (ADR-0110 section 3): the `[credentials]` section ([`CredentialsConfig`]) and
//! [`CredentialEnv`], an `Env` that answers the mapped variables from their sources.

mod config;
mod env;
mod openbao;

pub use config::{CredentialsConfig, DEFAULT_REFRESH, OpenBaoConfig, Source, SpiffeLogin};
pub use env::{
    CREDENTIALS_FILE, CREDENTIALS_VARIABLE, CredentialEnv, CredentialError, VariableCheck,
};
pub use openbao::OpenBaoClient;
