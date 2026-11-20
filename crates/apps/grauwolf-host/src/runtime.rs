//! tokio adapter for the runtime ports of the domain (ADR-0026).

use std::time::Duration;

use futures_util::future::BoxFuture;
use grauwolf_llm::Clock;

/// [`Clock`] based on `tokio::time`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioClock;

impl Clock for TokioClock {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(duration))
    }
}
