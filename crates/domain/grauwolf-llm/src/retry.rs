//! Retrying failed LLM requests (F02.11, ADR-0026).
//!
//! The policy only computes wait times; waiting happens via the port [`Clock`], so the
//! domain needs no runtime and tests do not really wait.

use std::time::Duration;

use futures_util::future::BoxFuture;

/// Port for time: waiting without being tied to a runtime (ADR-0026).
pub trait Clock: Send + Sync {
    /// Waits for `duration`.
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()>;
}

/// When and how often a temporarily failed request is retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Maximum number of retries after the first attempt.
    pub max_retries: u32,
    /// Wait time before the first retry; doubles with each attempt.
    pub initial_delay: Duration,
    /// Upper bound of every wait time, also for hints from the server.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    /// Wait time before retry no. `attempt + 1`, `None` if no more are allowed.
    /// `requested` is the wait time requested by the server (`retry-after`).
    pub fn next_delay(&self, attempt: u32, requested: Option<Duration>) -> Option<Duration> {
        if attempt >= self.max_retries {
            return None;
        }
        let backoff = self
            .initial_delay
            .saturating_mul(2u32.saturating_pow(attempt));
        Some(requested.unwrap_or(backoff).min(self.max_delay))
    }
}
