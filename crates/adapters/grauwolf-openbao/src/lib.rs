//! The OpenBao HTTP API shared by the agents' credential source and the operator program
//! (ADR-0120): JWT login, KV v2 read, the admin calls of the setup, and Transit signing for the
//! identity service (ADR-0111).

mod admin;
mod error;
mod transit;
mod transport;

pub use admin::{Bao, Health, Outcome, Token, jwt_role, kv_read_policy};
pub use error::BaoError;
pub use transit::{StaticToken, TokenSource, TransitKey};
pub use transport::{HttpTransport, Method, Request, Response, Transport};
