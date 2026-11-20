//! The prepared agent tree (ADR-0115) lives in `grauwolf-agent-tree`, shared with `grauwolf-tools`
//! (ADR-0118); this module re-exports it and reads trees with the checks only the host can make:
//! the built-in model providers and the local skill directories.

pub use grauwolf_agent_tree::{
    APP_TOOL_CACHE, AgentNode, AgentTree, Child, ReadErrors, ReadOptions, Resolution,
    ResolveOptions, ResolvedSkill,
};

/// The read options of the host: every check, with the providers of [`crate::providers::BUILTIN`].
pub fn read_options() -> ReadOptions {
    ReadOptions {
        providers: Some(
            crate::providers::BUILTIN
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
        ),
        skill_dirs: true,
    }
}

/// Reads the spec `file` and its local sub-agents ([`AgentTree::read`] with [`read_options`]).
pub fn read(file: &std::path::Path) -> Result<AgentTree, ReadErrors> {
    AgentTree::read(file, &read_options())
}

/// Like [`read`] for a root in memory ([`AgentTree::read_spec`] with [`read_options`]).
pub fn read_spec(
    spec: grauwolf_spec::AgentSpec,
    base_dir: &std::path::Path,
) -> Result<AgentTree, ReadErrors> {
    AgentTree::read_spec(spec, base_dir, &read_options())
}
