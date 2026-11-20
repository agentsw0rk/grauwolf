//! `grauwolf spec build <file>`: builds the tool code of the spec and of its sub-agents into WASM
//! artifacts (ADR-0024) and fetches their tool packages and git skills (ADR-0107, ADR-0115).

use std::path::Path;
use std::process::ExitCode;

use grauwolf_host::agent_tree::{AgentTree, ResolveOptions};

use crate::options::BuildOptions;
use crate::output::{Io, errln, outln, report};

/// `grauwolf spec build <file> [--cache <dir>] [--locked]`.
pub fn run(io: &Io, file: &Path, options: &BuildOptions) -> ExitCode {
    report(
        io,
        (|| {
            let tree = resolve_tree(io, file, options, |_| true)?;
            for node in tree.nodes() {
                let Some(resolution) = node.resolution() else {
                    continue;
                };
                for artifact in &resolution.artifacts {
                    outln!(io, "{}: {}", artifact.name, artifact.hash);
                }
                // Tool packages (ADR-0107): fetched into the cache, checked against the lockfile.
                for tool in &resolution.package_tools {
                    outln!(io, "{}: {}", tool.name, tool.artifact.hash);
                }
                // Git skills are fetched into the cache as well (ADR-0103).
                for skills in &resolution.skills {
                    outln!(io, "{}: commit {}", skills.key, skills.commit);
                }
            }
            Ok(())
        })(),
    )
}

/// Reads and resolves the agent tree of `file` (ADR-0115) with the tool cache of `options`;
/// warnings go to stderr. `build_code` decides from the read tree whether the code tools are
/// built too.
pub(crate) fn resolve_tree(
    io: &Io,
    file: &Path,
    options: &BuildOptions,
    build_code: impl FnOnce(&AgentTree) -> bool,
) -> Result<AgentTree, String> {
    let tree = grauwolf_host::agent_tree::read(file).map_err(|errors| errors.to_string())?;
    resolve_read_tree(io, tree, options, build_code)
}

/// Resolves a tree that was read (see [`resolve_tree`]).
pub(crate) fn resolve_read_tree(
    io: &Io,
    mut tree: AgentTree,
    options: &BuildOptions,
    build_code: impl FnOnce(&AgentTree) -> bool,
) -> Result<AgentTree, String> {
    let builder = options.tool_builder();
    let config =
        grauwolf_host::config::RegistryConfig::from_env(&grauwolf_host::config::ProcessEnv);
    let build_code = build_code(&tree);
    crate::errors::runtime()?.block_on(tree.resolve(&ResolveOptions {
        builder: &builder,
        locked: options.locked,
        config: &config,
        build_code,
        resolve_root: true,
    }))?;
    for warning in tree.warnings() {
        errln!(io, "{warning}");
    }
    Ok(tree)
}
