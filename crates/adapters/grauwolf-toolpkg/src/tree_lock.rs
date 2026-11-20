//! Locking a whole agent tree (ADR-0115, ADR-0118): one `grauwolf.lock` next to the root spec
//! covers the tool packages and the git skills sources of every node. The keys of the root are
//! plain, the keys of every other node are prefixed with its spec path relative to the root's
//! directory (`agents/writer.json#<key>`). The host's agent tree and the `grauwolf-tools lock`
//! command both lock through [`lock_tree`].

use std::path::Path;

use grauwolf_config::RegistryConfig;
use grauwolf_spec::AgentSpec;
use grauwolf_toolbuild::ToolBuilder;
use grauwolf_tools::ResolvedTool;

use crate::setup::{lock_path, package_resolver, skills_resolver};
use crate::{Lockfile, ResolvedSkills};

/// One spec of the tree. The first node of a slice is the root.
#[derive(Debug, Clone, Copy)]
pub struct LockNode<'a> {
    /// The spec file (its directory holds the lockfile of the root).
    pub file: &'a Path,
    /// The spec path relative to the root's directory (the key prefix of non-root nodes).
    pub relative: &'a Path,
    /// The spec.
    pub spec: &'a AgentSpec,
}

/// What locking needs.
pub struct LockOptions<'a> {
    /// Compiles git packages; its cache is the tool cache.
    pub builder: &'a ToolBuilder,
    /// The lockfile must be complete: fails instead of changing it.
    pub locked: bool,
    /// Registries, credentials, tool policy file, `GIT_TOKEN`.
    pub config: &'a RegistryConfig,
    /// Resolve the root too; `false` keeps its entries as they are (the generated service).
    pub resolve_root: bool,
}

/// What locking resolved for one node.
#[derive(Debug, Default)]
pub struct LockedNode {
    /// The tools of the `tool_packages`.
    pub package_tools: Vec<ResolvedTool>,
    /// The git skills sources, resolved into the cache.
    pub skills: Vec<ResolvedSkills>,
}

/// The key of an entry of the node with spec path `relative` in the lockfile of the tree.
pub fn lock_key_prefix(relative: &Path) -> String {
    format!("{}#", relative.display())
}

/// Resolves the tool packages and git skills of every node against the one lockfile of the tree
/// (next to the root spec), writes it when it changed and returns what each node resolved to, in
/// the order of `nodes`. Entries that no node claims are dropped; with `locked` they are an error
/// naming the entry. The lockfile is not touched when no node has packages or git skills.
pub async fn lock_tree(
    nodes: &[LockNode<'_>],
    options: &LockOptions<'_>,
) -> Result<Vec<LockedNode>, String> {
    let has_git = |spec: &AgentSpec| spec.skills.iter().any(|source| source.git.is_some());
    let mut locked_nodes: Vec<LockedNode> = nodes.iter().map(|_| LockedNode::default()).collect();
    let Some(root) = nodes.first() else {
        return Ok(locked_nodes);
    };
    let root_file = root.file;
    let lock_path = lock_path(root_file);
    let old = Lockfile::load(&lock_path).map_err(|error| error.to_string())?;
    if !nodes
        .iter()
        .any(|node| !node.spec.tool_packages.is_empty() || has_git(node.spec))
    {
        // Nothing claims an entry: whatever the lockfile holds is stale.
        drop_unclaimed(options.locked, &old, &Lockfile::default(), &lock_path)?;
        return Ok(locked_nodes);
    }
    let cache = options.builder.cache_dir();
    let packages = package_resolver(
        root_file,
        cache,
        options.locked,
        options.config,
        options.builder,
    )?;
    let skills = skills_resolver(root_file, cache, options.locked, options.config)?;
    let prefixes: Vec<Option<String>> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (index > 0).then(|| lock_key_prefix(node.relative)))
        .collect();
    let others: Vec<&str> = prefixes.iter().flatten().map(String::as_str).collect();
    let mut merged = Lockfile::default();
    for ((node, locked_node), prefix) in nodes.iter().zip(&mut locked_nodes).zip(&prefixes) {
        let fail = |error: &dyn std::fmt::Display| format!("{}: {error}", node.file.display());
        let mut view = lock_view(&old, prefix.as_deref(), &others);
        let skip = prefix.is_none() && !options.resolve_root;
        if !skip && !node.spec.tool_packages.is_empty() {
            let (tools, new) = packages
                .resolve_in(node.spec, &view, &lock_path)
                .await
                .map_err(|error| fail(&error))?;
            locked_node.package_tools = tools;
            view = new;
        }
        if !skip && has_git(node.spec) {
            let (resolved, new) = skills
                .resolve_skills_in(node.spec, &view, &lock_path)
                .await
                .map_err(|error| fail(&error))?;
            view = new;
            locked_node.skills = resolved;
        }
        if !skip {
            // Entries that this node does not claim (a section its spec does not use)
            // are stale; `--locked` names them, otherwise they are dropped (ADR-0115).
            let mut unclaimed = Lockfile::default();
            if node.spec.tool_packages.is_empty() {
                unclaimed.packages = std::mem::take(&mut view.packages);
                unclaimed.git = std::mem::take(&mut view.git);
            }
            if !has_git(node.spec) {
                unclaimed.skills = std::mem::take(&mut view.skills);
            }
            let mut stale = Lockfile::default();
            lock_merge(&mut stale, unclaimed, prefix.as_deref());
            if let Some(key) = first_entry(&stale) {
                if options.locked {
                    return Err(stale_error(&lock_path, key));
                }
            }
        }
        lock_merge(&mut merged, view, prefix.as_deref());
    }
    drop_unclaimed(options.locked, &old, &merged, &lock_path)?;
    Ok(locked_nodes)
}

/// Ends the lockfile handling: `merged` is the lockfile as the tree claims it. With
/// `--locked`, any entry of `old` that it lacks is an error naming the entry; without, the
/// lockfile is rewritten when it differs.
fn drop_unclaimed(
    locked: bool,
    old: &crate::Lockfile,
    merged: &crate::Lockfile,
    lock_path: &Path,
) -> Result<(), String> {
    if merged == old {
        return Ok(());
    }
    if locked {
        let mut stale = old.clone();
        stale
            .packages
            .retain(|key, _| !merged.packages.contains_key(key));
        stale.git.retain(|key, _| !merged.git.contains_key(key));
        stale
            .skills
            .retain(|key, _| !merged.skills.contains_key(key));
        return Err(match first_entry(&stale) {
            Some(key) => stale_error(lock_path, key),
            None => format!(
                "{} is out of date: it would change; run `grauwolf tools lock <spec>` and \
                 commit the lockfile",
                lock_path.display()
            ),
        });
    }
    merged.save(lock_path).map_err(|error| error.to_string())
}

/// The first entry key of `lock` in any section.
fn first_entry(lock: &crate::Lockfile) -> Option<&String> {
    lock.packages
        .keys()
        .chain(lock.git.keys())
        .chain(lock.skills.keys())
        .next()
}

/// The `--locked` error for the entry `key` that no node of the tree claims.
fn stale_error(lock_path: &Path, key: &str) -> String {
    crate::lock_change(
        lock_path,
        format!("`{key}` is locked but no node of the agent tree uses it"),
    )
    .to_string()
}

/// The entries of `lock` that belong to the node with key prefix `prefix` (root: `None`, the
/// entries that no other node's prefix claims), keys without the prefix.
pub fn lock_view(lock: &crate::Lockfile, prefix: Option<&str>, others: &[&str]) -> crate::Lockfile {
    use std::collections::BTreeMap;
    fn pick<T: Clone>(
        entries: &BTreeMap<String, T>,
        prefix: Option<&str>,
        others: &[&str],
    ) -> BTreeMap<String, T> {
        entries
            .iter()
            .filter_map(|(key, value)| match prefix {
                Some(prefix) => key
                    .strip_prefix(prefix)
                    .map(|rest| (rest.to_owned(), value.clone())),
                None => (!others.iter().any(|other| key.starts_with(other)))
                    .then(|| (key.clone(), value.clone())),
            })
            .collect()
    }
    crate::Lockfile {
        lock_version: lock.lock_version,
        packages: pick(&lock.packages, prefix, others),
        git: pick(&lock.git, prefix, others),
        skills: pick(&lock.skills, prefix, others),
    }
}

/// Adds the entries of a node's `view` to `merged` with the node's key prefix.
pub fn lock_merge(merged: &mut crate::Lockfile, view: crate::Lockfile, prefix: Option<&str>) {
    let key = |key: String| format!("{}{key}", prefix.unwrap_or(""));
    merged
        .packages
        .extend(view.packages.into_iter().map(|(k, v)| (key(k), v)));
    merged
        .git
        .extend(view.git.into_iter().map(|(k, v)| (key(k), v)));
    merged
        .skills
        .extend(view.skills.into_iter().map(|(k, v)| (key(k), v)));
}
