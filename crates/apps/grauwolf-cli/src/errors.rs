//! Error plumbing shared by the commands: errors are plain messages (`String`), printed by
//! [`report`](crate::output::report) with the exit code 1.

/// The tokio runtime that commands block on.
pub fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Runtime::new().map_err(|error| error.to_string())
}
