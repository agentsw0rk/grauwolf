//! Tool packages of a spec (ADR-0107): resolved against the lockfile next to the spec, pulled
//! into the tool cache, ready for the interpreter and the generator. The setup shared with the
//! `grauwolf-tools` program (resolvers, policy, keys, pack and push) lives in
//! `grauwolf_toolpkg::setup` and is re-exported here under the same paths (ADR-0118).

use std::path::Path;

use grauwolf_spec::AgentSpec;
use grauwolf_toolbuild::ToolBuilder;
use grauwolf_tools::ResolvedTool;

use crate::config::{ProcessEnv, RegistryConfig};

pub use grauwolf_toolpkg::setup::*;

/// The tools of the `tool_packages` of `spec` (read from `file`); empty without packages, and
/// then without any access to the network or the cache. Git sources are built with a
/// [`ToolBuilder`] on `tool_cache`.
pub async fn resolve_tools(
    file: &Path,
    spec: &AgentSpec,
    tool_cache: &Path,
    locked: bool,
) -> Result<Vec<ResolvedTool>, String> {
    resolve_tools_with(
        file,
        spec,
        tool_cache,
        locked,
        &RegistryConfig::from_env(&ProcessEnv),
        &ToolBuilder::new(tool_cache),
    )
    .await
}

/// Like [`resolve_tools`] with the settings in `config` (read only when the spec has packages)
/// and the `builder` that compiles the packages from git repositories (ADR-0107 stage 3).
pub async fn resolve_tools_with(
    file: &Path,
    spec: &AgentSpec,
    tool_cache: &Path,
    locked: bool,
    config: &RegistryConfig,
    builder: &ToolBuilder,
) -> Result<Vec<ResolvedTool>, String> {
    if spec.tool_packages.is_empty() {
        return Ok(Vec::new());
    }
    package_resolver(file, tool_cache, locked, config, builder)?
        .resolve(spec, &lock_path(file))
        .await
        .map_err(|error| format!("{}: {error}", file.display()))
}
