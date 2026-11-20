//! The `system_one` decider of decision nodes (ADR-0100); provisioning picks it (ADR-0114).

use std::sync::Arc;

use grauwolf_agents::decision::{Decider, RetryingDecider};
use grauwolf_llm::RetryPolicy;
use grauwolf_spec::AgentSpec;

pub use grauwolf_systemone::SystemOneClient;

use crate::config::SystemOneConfig;
use crate::runtime::TokioClock;

/// A spec asks for the `system_one` decider, but there is no service to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DeciderError {
    /// `RKEV_BASE_URL` is not set.
    #[error(
        "the spec uses the decider 'system_one', but RKEV_BASE_URL is not set \
         (the address of the System One service, e.g. http://localhost:8090; \
         RKEV_API_KEY is optional)"
    )]
    MissingBaseUrl,
}

/// The decider for the decisions of `spec` that name `system_one`: `provided` if the caller
/// hands one in, otherwise the service at `RKEV_BASE_URL` (key `RKEV_API_KEY`), whose
/// temporary failures are retried with the default retry policy. `None` if the spec has no
/// such decision; without a service the error comes before the run.
pub(crate) fn decider_with(
    spec: &AgentSpec,
    provided: Option<Arc<dyn Decider>>,
    config: &SystemOneConfig,
) -> Result<Option<Arc<dyn Decider>>, DeciderError> {
    if provided.is_some() || !spec.uses_system_one() {
        return Ok(provided);
    }
    let base_url = config
        .base_url
        .as_deref()
        .ok_or(DeciderError::MissingBaseUrl)?;
    let client = SystemOneClient::new(base_url);
    let client = match &config.api_key {
        Some(key) => client.with_api_key(key.clone()),
        None => client,
    };
    Ok(Some(Arc::new(RetryingDecider::new(
        Arc::new(client),
        RetryPolicy::default(),
        Arc::new(TokioClock),
    ))))
}
