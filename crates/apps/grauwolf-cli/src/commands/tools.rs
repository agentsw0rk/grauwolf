//! `grauwolf tools pull|inspect|keygen|pack|push|lock|build|validate`: the commands live in
//! `grauwolf-tools-app` and are the same as those of the program `grauwolf-tools` (ADR-0118). Only
//! `lock` differs: here it reads the fully checked agent tree of the host (ADR-0115).

use std::path::Path;
use std::process::ExitCode;

use grauwolf_tools_app::LockSummary;
pub use grauwolf_tools_app::ToolsCommand;

use crate::options::BuildOptions;
use crate::output::Io;

pub fn run(io: &Io, command: ToolsCommand) -> ExitCode {
    grauwolf_tools_app::run_tools(io, command, &lock)
}

/// Resolves the agent tree of `file` (read and checked, no code build) and writes the lockfile.
fn lock(io: &Io, file: &Path, build: &BuildOptions) -> Result<LockSummary, String> {
    let tree = super::spec::build::resolve_tree(io, file, build, |_| false)?;
    let nodes = tree.nodes();
    Ok(LockSummary {
        lock_path: tree.lock_path(),
        nodes: nodes.len(),
        packages: nodes.iter().map(|n| n.spec().tool_packages.len()).sum(),
        tools: nodes
            .iter()
            .filter_map(|n| n.resolution())
            .map(|r| r.package_tools.len())
            .sum(),
        git_skills: nodes
            .iter()
            .map(|n| n.spec().skills.iter().filter(|s| s.git.is_some()).count())
            .sum(),
    })
}
