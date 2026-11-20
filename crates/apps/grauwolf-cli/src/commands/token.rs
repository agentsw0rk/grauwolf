//! `grauwolf token <user>`: a new user token for `spec serve --users` (ADR-0068).

use std::process::ExitCode;

use crate::output::{Io, outln};

/// Prints the token (visible only now) and the line for the users file.
pub fn run(io: &Io, user: String) -> ExitCode {
    let token = grauwolf_server::new_user_token();
    outln!(io, "{token}");
    outln!(
        io,
        "{}: \"sha256:{}\"",
        serde_json::Value::String(user),
        grauwolf_server::token_hash(&token)
    );
    ExitCode::SUCCESS
}
