//! `lock`: resolves the tool packages and git skills of a spec and its local sub-agents into the
//! one `grauwolf.lock` next to the spec (ADR-0107, ADR-0115).
//!
//! The tree is read by `grauwolf-agent-tree`, the same reader the host uses, and locked with
//! `grauwolf_toolpkg::tree_lock::lock_tree`. This program does not run agents, so it skips the one
//! check that needs the model providers (`ReadOptions::providers`); `grauwolf tools lock` passes a
//! `Locker` over the host's tree, which makes that check too (ADR-0118).

use std::path::{Path, PathBuf};

use grauwolf_agent_tree::{AgentTree, ReadOptions};
use grauwolf_config::{ProcessEnv, RegistryConfig};
use grauwolf_toolpkg::tree_lock::{LockNode, LockOptions, lock_tree};

use crate::errln;
use crate::options::BuildOptions;
use crate::output::Io;

/// What a lock run did, for the one line it prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockSummary {
    /// The lockfile of the tree.
    pub lock_path: PathBuf,
    /// Number of specs in the tree.
    pub nodes: usize,
    /// Number of `tool_packages` entries of all nodes.
    pub packages: usize,
    /// Number of tools those packages resolved to.
    pub tools: usize,
    /// Number of git sources of `skills` of all nodes.
    pub git_skills: usize,
}

impl LockSummary {
    /// The line `lock` prints for the spec `file`; an error if the tree has nothing to lock.
    pub fn describe(&self, file: &Path) -> Result<String, String> {
        if self.packages == 0 && self.git_skills == 0 {
            let subject = if self.nodes == 1 {
                "the spec has"
            } else {
                "the spec and its sub-agents have"
            };
            return Err(format!(
                "{}: {subject} no `tool_packages` and no git `skills`",
                file.display()
            ));
        }
        let mut parts = Vec::new();
        if self.packages > 0 {
            parts.push(format!("{} locked ({} tools)", self.packages, self.tools));
        }
        if self.git_skills > 0 {
            parts.push(format!("{} git skills source(s) locked", self.git_skills));
        }
        Ok(format!(
            "{}: {}",
            self.lock_path.display(),
            parts.join(", ")
        ))
    }
}

/// How `lock` obtains the tree and locks it: reads the spec `file`, resolves, writes the
/// lockfile and returns the summary.
pub type Locker<'a> = &'a dyn Fn(&Io, &Path, &BuildOptions) -> Result<LockSummary, String>;

/// The locker of this program: the agent tree of `file` (every check but the providers), locked
/// with the registry settings of the process environment.
pub fn lock_spec_tree(io: &Io, file: &Path, options: &BuildOptions) -> Result<LockSummary, String> {
    let tree =
        AgentTree::read(file, &ReadOptions::default()).map_err(|errors| errors.to_string())?;
    for warning in tree.warnings() {
        errln!(io, "{warning}");
    }
    let nodes: Vec<LockNode<'_>> = tree
        .nodes()
        .iter()
        .map(|node| LockNode {
            file: node.file(),
            relative: node.relative(),
            spec: node.spec(),
        })
        .collect();
    let builder = options.tool_builder();
    let config = RegistryConfig::from_env(&ProcessEnv);
    let locked = crate::errors::runtime()?.block_on(lock_tree(
        &nodes,
        &LockOptions {
            builder: &builder,
            locked: options.locked,
            config: &config,
            resolve_root: true,
        },
    ))?;
    Ok(LockSummary {
        lock_path: grauwolf_toolpkg::setup::lock_path(file),
        nodes: nodes.len(),
        packages: nodes.iter().map(|n| n.spec.tool_packages.len()).sum(),
        tools: locked.iter().map(|n| n.package_tools.len()).sum(),
        git_skills: nodes
            .iter()
            .map(|n| n.spec.skills.iter().filter(|s| s.git.is_some()).count())
            .sum(),
    })
}
