//! PostgreSQL with pgvector for knowledge and long-term memory (ADR-0106): the connection from
//! the environment variable that the spec names.

use std::sync::Arc;

use grauwolf_store_pgvector::PgVector;

use crate::config::Env;

/// Connects to the database whose URL is in the environment variable `variable` (the spec's
/// `url_env`). Knowledge and memory with the same URL share one pool. Errors name the variable,
/// never the URL or the password.
pub async fn connect(variable: &str, env: &dyn Env) -> Result<Arc<PgVector>, String> {
    let url = env
        .var(variable)
        .filter(|url| !url.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "the environment variable {variable} (named by `url_env`) is not set; it must \
                 hold the PostgreSQL connection URL"
            )
        })?;
    PgVector::connect(variable, &url)
        .await
        .map_err(|error| error.to_string())
}
