//! What the integration tests use to get the tools of a spec: through provisioning (ADR-0114).
#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;

use grauwolf_host::config::{Env, MapEnv};
use grauwolf_host::provisioning::{ProvisionOptions, ProvisionedAgent};
use grauwolf_spec::{AgentSpec, AgentSpecExt, BuiltinToolsSpec};
use grauwolf_tools::DynTool;

/// The tools of `spec` for the shared area, with an environment that has no variables.
pub async fn tools(spec: &AgentSpec, base: &Path) -> Result<Vec<Arc<dyn DynTool>>, String> {
    tools_in(spec, base, MapEnv::new([]), None).await
}

/// The tools of `spec` for a run of `user`.
pub async fn tools_for(
    spec: &AgentSpec,
    base: &Path,
    user: Option<&str>,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    tools_in(spec, base, MapEnv::new([]), user).await
}

/// The tools of `spec` for a run of `user`, with the environment `env`.
pub async fn tools_in(
    spec: &AgentSpec,
    base: &Path,
    env: impl Env + 'static,
    user: Option<&str>,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    // The providers of the spec are not under test here (and may not be compiled in).
    let options = ProvisionOptions::new(Arc::new(env))
        .with_executor(Arc::new(grauwolf_llm::PromptExecutor::builder().build()));
    let agent = ProvisionedAgent::new(spec, base, options).await?;
    agent.tools_for(user)
}

/// The tools of the built-in section `builtin` with the environment `env`.
pub fn tools_with_env(
    builtin: &BuiltinToolsSpec,
    base: &Path,
    env: impl Env + 'static,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    let spec = AgentSpec::from_json(
        &serde_json::json!({
            "spec_version": 1, "name": "builtin-test",
            "model": { "provider": "ollama", "id": "m" },
            "builtin_tools": builtin,
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .map_err(|error| error.to_string())?;
    futures_executor::block_on(tools_in(&spec, base, env, None))
}
