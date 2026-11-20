//! The prepared agent tree (ADR-0115): a spec and the specs of its sub-agents, read and checked
//! once, with one set of rules for every node – so `spec validate`, `build`, `tools lock`, `run`
//! and `generate` agree on what a tree is.
//!
//! **Read phase** ([`AgentTree::read`], local, no network): reads every spec and its code files,
//! finds the children, computes the signatures of the sub-agents, the companion files of every
//! node, and refuses cycles and sub-specs outside the root's directory. A sub-spec that several
//! parents name (a diamond) is one node, read once.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use grauwolf_spec::{AgentSpec, AgentSpecExt};

use crate::spec_file::{read_spec, resolved_json, warning_lines};

/// A spec and its sub-agents, read and checked. Node 0 is the root.
#[derive(Debug)]
pub struct AgentTree {
    root_dir: PathBuf,
    nodes: Vec<AgentNode>,
    warnings: Vec<String>,
}

/// One spec of the tree.
#[derive(Debug)]
pub struct AgentNode {
    file: PathBuf,
    identity: PathBuf,
    relative: PathBuf,
    spec: AgentSpec,
    children: Vec<Child>,
    companions: Vec<PathBuf>,
    resolution: Option<Resolution>,
}

/// What the resolve phase made of a node (ADR-0115).
#[derive(Debug)]
pub struct Resolution {
    /// The spec with its git skill sources replaced by the cache directories they resolved to
    /// (absolute); otherwise the node's spec.
    pub spec: AgentSpec,
    /// The tools of the `tool_packages`.
    pub package_tools: Vec<grauwolf_tools::ResolvedTool>,
    /// The built code of the tools and of the code in the graphs (in the shared tool cache).
    pub artifacts: Vec<grauwolf_tools::ToolArtifact>,
    /// The git skill sources, resolved.
    pub skills: Vec<ResolvedSkill>,
}

/// A git skills source that was resolved into the tool cache.
#[derive(Debug, Clone)]
pub struct ResolvedSkill {
    /// Index in `skills` of the spec.
    pub index: usize,
    /// Lock key.
    pub key: String,
    /// The commit it is at.
    pub commit: String,
    /// Its directory in the tool cache.
    pub dir: PathBuf,
}

/// What the resolve phase needs.
pub struct ResolveOptions<'a> {
    /// Builds the code tools; its cache is the shared tool cache of the tree.
    pub builder: &'a grauwolf_toolbuild::ToolBuilder,
    /// `grauwolf.lock` must be complete: resolving fails instead of changing it.
    pub locked: bool,
    /// Registries, credentials, tool policy file, `GIT_TOKEN`.
    pub config: &'a grauwolf_config::RegistryConfig,
    /// Build the code tools (`false` for `tools lock`, which only locks).
    pub build_code: bool,
    /// Resolve the root node too. `false` where the caller has the root already (the generated
    /// service embeds it): its lock entries stay as they are, nothing of it is built.
    pub resolve_root: bool,
}

/// A sub-agent of a node, by the name in `agent_tools`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Child {
    /// A local sub-agent: the index of its node (the same for every parent that names it).
    Local { name: String, node: usize },
    /// A remote agent over A2A: a leaf with an address, no spec, no files.
    Remote { name: String, url: String },
}

impl Child {
    /// The name in `agent_tools`.
    pub fn name(&self) -> &str {
        match self {
            Self::Local { name, .. } | Self::Remote { name, .. } => name,
        }
    }
}

/// Why a tree could not be read: every error of every node, in the format
/// `<file>:<pointer>: <message>`, one per line when displayed.
#[derive(Debug)]
pub struct ReadErrors {
    /// The errors.
    pub errors: Vec<String>,
    /// The warnings of the nodes that could be read, as `warning: <file>:<pointer>: <message>`.
    pub warnings: Vec<String>,
}

impl std::fmt::Display for ReadErrors {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.errors.join("\n"))
    }
}

impl std::error::Error for ReadErrors {}

impl AgentTree {
    /// Reads the spec `file` and, recursively, the specs of its local sub-agents.
    ///
    /// Fails with every error found: unreadable or invalid specs, unknown providers, invalid
    /// local skill directories, `agent` nodes whose types differ from the sub-agent's, cycles
    /// (`sub-agents form a cycle: a.json → b.json → a.json`) and sub-specs outside the root's
    /// directory.
    pub fn read(file: &Path, options: &ReadOptions) -> Result<Self, ReadErrors> {
        Reader::new(file, options)?.finish(file, None)
    }

    /// Like [`read`](Self::read) for a root that is already in memory: `spec` stands for the
    /// root, its sub-agent specs are read relative to `base_dir`. Nothing is read from the root's
    /// file, which may not exist (the generated service embeds its root spec).
    pub fn read_spec(
        spec: AgentSpec,
        base_dir: &Path,
        options: &ReadOptions,
    ) -> Result<Self, ReadErrors> {
        let file = base_dir.join(format!("{}.json", spec.name));
        Reader::new(&file, options)?.finish(&file, Some(spec))
    }

    /// The directory of the root spec; every spec of the tree is inside it.
    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    /// The root node.
    pub fn root(&self) -> &AgentNode {
        &self.nodes[0]
    }

    /// All nodes, the root first; a spec that several parents name is one node.
    pub fn nodes(&self) -> &[AgentNode] {
        &self.nodes
    }

    /// The node with `index` (see [`Child::Local`]).
    pub fn node(&self, index: usize) -> &AgentNode {
        &self.nodes[index]
    }

    /// The lockfile of the tree: `grauwolf.lock` next to the root spec. Its entries for tool
    /// packages and git skills of the root keep the keys they have in a lockfile of a spec alone;
    /// the entries of every other node carry the node's spec path relative to the root's
    /// directory in front: `<relative spec path>#<key>`, e.g. `agents/writer.json#git+…@v1`.
    pub fn lock_path(&self) -> PathBuf {
        self.nodes[0].base_dir().join(LOCKFILE)
    }

    /// The warnings of all nodes, as `warning: <file>:<pointer>: <message>`.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

impl AgentNode {
    /// The path of the spec file as it is named in messages.
    pub fn file(&self) -> &Path {
        &self.file
    }

    /// The canonical path of the spec file: what makes two references the same node.
    pub fn identity(&self) -> &Path {
        &self.identity
    }

    /// The path of the spec file relative to the root's directory (for the root its file name):
    /// the place of the file in an image's `app/`.
    pub fn relative(&self) -> &Path {
        &self.relative
    }

    /// The directory of the spec file; its files are relative to it.
    pub fn base_dir(&self) -> &Path {
        self.file.parent().unwrap_or(Path::new("."))
    }

    /// The spec, with the files of `code_file` and `tool_files` read.
    pub fn spec(&self) -> &AgentSpec {
        &self.spec
    }

    /// The sub-agents in the order of the names in `agent_tools`.
    pub fn children(&self) -> &[Child] {
        &self.children
    }

    /// The files and directories the node reads at run time (knowledge, memory store, root of
    /// the file tools, skill directories), relative to [`base_dir`](Self::base_dir); only paths
    /// below it, existing or not.
    pub fn companions(&self) -> &[PathBuf] {
        &self.companions
    }

    /// The spec as it is in an image's `app/`: git skill sources replaced by the local
    /// directories `skills-git/<index>` (next to the spec; see [`AgentTree::copy_into`]). Without
    /// a resolve phase the spec as it is.
    pub fn image_spec(&self) -> AgentSpec {
        let mut spec = self.spec.clone();
        for skill in self
            .resolution
            .iter()
            .flat_map(|resolution| &resolution.skills)
        {
            crate::skills::replace_with_dir(
                &mut spec.skills[skill.index],
                image_skills_dir(skill.index),
            );
        }
        spec
    }

    /// The spec file as it is in an image: the document with `tool_files` and `code_file` inlined
    /// and the git skill sources replaced as in [`image_spec`](Self::image_spec); everything else
    /// as the author wrote it.
    fn image_document(&self) -> Result<String, String> {
        let fail = |error: &dyn std::fmt::Display| format!("{}: {error}", self.file.display());
        let mut document: serde_json::Value =
            serde_json::from_str(&resolved_json(&self.file)?).map_err(|error| fail(&error))?;
        for skill in self
            .resolution
            .iter()
            .flat_map(|resolution| &resolution.skills)
        {
            let mut entry = serde_json::json!({ "dir": image_skills_dir(skill.index) });
            if let Some(names) = &self.spec.skills[skill.index].names {
                entry["names"] = serde_json::json!(names);
            }
            document["skills"][skill.index] = entry;
        }
        serde_json::to_string_pretty(&document)
            .map(|text| text + "\n")
            .map_err(|error| fail(&error))
    }

    /// What the resolve phase made of the node; `None` before [`AgentTree::resolve`].
    pub fn resolution(&self) -> Option<&Resolution> {
        self.resolution.as_ref()
    }
}

/// The checks of the read phase that depend on the program (ADR-0118). Everything else is checked
/// always.
#[derive(Debug, Clone)]
pub struct ReadOptions {
    /// The model providers the program knows: a spec naming another one is an error. `None` skips
    /// the check (a program that does not run agents, like `grauwolf-tools lock`, does not know
    /// the providers; `spec validate` and `run` of the host do).
    pub providers: Option<Vec<String>>,
    /// Check the local `skills` directories like loading them does (`SKILL.md`, names, sizes,
    /// symlinks).
    pub skill_dirs: bool,
}

impl Default for ReadOptions {
    /// Local skill directories are checked, providers are not.
    fn default() -> Self {
        Self {
            providers: None,
            skill_dirs: true,
        }
    }
}

/// Reads the nodes depth first; `chain` are the nodes being read above the current one.
struct Reader<'a> {
    options: &'a ReadOptions,
    root_dir: PathBuf,
    nodes: Vec<AgentNode>,
    by_identity: HashMap<PathBuf, usize>,
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl<'a> Reader<'a> {
    fn new(root_file: &Path, options: &'a ReadOptions) -> Result<Self, ReadErrors> {
        let failed = |error: String| ReadErrors {
            errors: vec![error],
            warnings: Vec::new(),
        };
        let dir = root_file.parent().unwrap_or(Path::new("."));
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        let root_dir = dir
            .canonicalize()
            .map_err(|error| failed(format!("{}: {error}", root_file.display())))?;
        Ok(Self {
            options,
            root_dir,
            nodes: Vec::new(),
            by_identity: HashMap::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
        })
    }

    fn finish(mut self, file: &Path, spec: Option<AgentSpec>) -> Result<AgentTree, ReadErrors> {
        let identity = match &spec {
            Some(_) => file.file_name().map(|name| self.root_dir.join(name)),
            None => file.canonicalize().ok(),
        };
        let Some(identity) = identity else {
            // Not canonical: `read_spec` says why in its usual words.
            let error = read_spec(file)
                .err()
                .unwrap_or_else(|| format!("{}: not readable", file.display()));
            return Err(ReadErrors {
                errors: vec![error],
                warnings: Vec::new(),
            });
        };
        self.node(file, identity, spec, &mut Vec::new());
        if self.errors.is_empty() {
            Ok(AgentTree {
                root_dir: self.root_dir,
                nodes: self.nodes,
                warnings: self.warnings,
            })
        } else {
            Err(ReadErrors {
                errors: self.errors,
                warnings: self.warnings,
            })
        }
    }

    /// Reads the node `file` (and below it its children) unless it is read already; its index,
    /// or `None` when it cannot be read.
    fn node(
        &mut self,
        file: &Path,
        identity: PathBuf,
        spec: Option<AgentSpec>,
        chain: &mut Vec<PathBuf>,
    ) -> Option<usize> {
        let spec = match spec {
            Some(spec) => spec,
            None => match read_spec(file) {
                Ok(spec) => spec,
                Err(error) => {
                    self.errors.push(error);
                    return None;
                }
            },
        };
        let base_dir = file.parent().unwrap_or(Path::new("."));
        match spec.validate() {
            Err(invalid) => self.errors.extend(
                invalid
                    .iter()
                    .map(|error| format!("{}:{error}", file.display())),
            ),
            Ok(()) => {
                self.warnings.extend(warning_lines(file, &spec));
                // Local skill directories are checked here, git sources when they are resolved
                // (ADR-0103).
                if self.options.skill_dirs
                    && let Err(error) = crate::skills::check_local(&spec, base_dir)
                {
                    self.errors
                        .push(format!("{}:/skills: {error}", file.display()));
                }
            }
        }
        if let Some(known) = &self.options.providers {
            for (pointer, provider) in spec.provider_uses() {
                if !known.iter().any(|name| name == provider) {
                    self.errors.push(format!(
                        "{}:{pointer}: unknown provider '{provider}' (built-in: {})",
                        file.display(),
                        known.join(", ")
                    ));
                }
            }
        }
        let index = self.nodes.len();
        let relative = identity
            .strip_prefix(&self.root_dir)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| identity.clone());
        let companions = grauwolf_generator::companion_paths(&spec)
            .into_iter()
            .map(PathBuf::from)
            .filter(|path| {
                path.components()
                    .all(|part| matches!(part, Component::Normal(_)))
            })
            .collect();
        self.by_identity.insert(identity.clone(), index);
        self.nodes.push(AgentNode {
            file: file.to_owned(),
            identity: identity.clone(),
            relative,
            spec: spec.clone(),
            children: Vec::new(),
            companions,
            resolution: None,
        });
        chain.push(identity);
        let mut children = Vec::new();
        let mut signatures = std::collections::BTreeMap::new();
        for (name, tool) in &spec.agent_tools {
            match (&tool.spec, &tool.url) {
                (Some(sub), _) => {
                    let pointer = format!(
                        "/agent_tools/{}/spec",
                        grauwolf_spec::escape_pointer_token(name)
                    );
                    let Some(node) = self.child(file, &pointer, sub, chain) else {
                        continue;
                    };
                    let child = &self.nodes[node].spec;
                    signatures.insert(name.clone(), (child.input.clone(), child.output.clone()));
                    children.push(Child::Local {
                        name: name.clone(),
                        node,
                    });
                }
                (None, Some(url)) => children.push(Child::Remote {
                    name: name.clone(),
                    url: url.clone(),
                }),
                (None, None) => {}
            }
        }
        chain.pop();
        // The types that `agent` nodes declare must be those of the sub-agents (ADR-0101).
        self.errors.extend(
            spec.check_sub_agent_types(&signatures)
                .iter()
                .map(|error| format!("{}:{error}", file.display())),
        );
        self.nodes[index].children = children;
        Some(index)
    }

    /// The node of the sub-agent spec `sub` (as written in the spec `parent`, at `pointer`).
    fn child(
        &mut self,
        parent: &Path,
        pointer: &str,
        sub: &str,
        chain: &mut Vec<PathBuf>,
    ) -> Option<usize> {
        let fail = |reader: &mut Self, message: String| {
            reader
                .errors
                .push(format!("{}:{pointer}: {message}", parent.display()));
            None
        };
        let file = parent.parent().unwrap_or(Path::new(".")).join(sub);
        if Path::new(sub).is_absolute() {
            return fail(
                self,
                format!("sub-agent spec outside the agent's directory: {sub}"),
            );
        }
        let Ok(identity) = file.canonicalize() else {
            // The usual words for a file that cannot be read.
            return match read_spec(&file) {
                Err(error) => {
                    self.errors.push(error);
                    None
                }
                Ok(_) => fail(self, format!("{}: not readable", file.display())),
            };
        };
        if !identity.starts_with(&self.root_dir) {
            return fail(
                self,
                format!("sub-agent spec outside the agent's directory: {sub}"),
            );
        }
        if chain.contains(&identity) {
            let names: Vec<String> = chain
                .iter()
                .chain([&identity])
                .map(|path| self.label(path))
                .collect();
            return fail(
                self,
                format!("sub-agents form a cycle: {}", names.join(" → ")),
            );
        }
        if let Some(index) = self.by_identity.get(&identity) {
            return Some(*index);
        }
        self.node(&file, identity, None, chain)
    }

    /// `path` relative to the root's directory, for messages.
    fn label(&self, path: &Path) -> String {
        path.strip_prefix(&self.root_dir)
            .unwrap_or(path)
            .display()
            .to_string()
    }
}

/// The name of the lockfile (the one of `grauwolf_toolpkg`, which is optional here).
const LOCKFILE: &str = "grauwolf.lock";

#[cfg(feature = "packages")]
use grauwolf_toolpkg::tree_lock::{lock_key_prefix, lock_view};

impl AgentTree {
    /// Resolve phase (network, lockfile): localises git skills, resolves tool packages and builds
    /// the code tools of every node, once per node. One lockfile (see
    /// [`lock_path`](Self::lock_path)) and the tool policy of the root cover the whole tree; with
    /// `locked` the lockfile is only checked, never written. A lockfile next to a sub-spec is not
    /// used: it gives a warning. The lockfile is not touched when no node has packages or git
    /// skills.
    pub async fn resolve(&mut self, options: &ResolveOptions<'_>) -> Result<(), String> {
        for node in self.nodes.iter().skip(1) {
            let lock = node.base_dir().join(LOCKFILE);
            if node.base_dir().canonicalize().ok().as_deref() != Some(&self.root_dir)
                && lock.is_file()
            {
                self.warnings.push(format!(
                    "warning: {}: not used: the lockfile of the tree is {}",
                    lock.display(),
                    self.lock_path().display()
                ));
            }
        }
        let mut resolutions = Vec::new();
        for node in &self.nodes {
            resolutions.push(Resolution {
                spec: node.spec.clone(),
                package_tools: Vec::new(),
                artifacts: Vec::new(),
                skills: Vec::new(),
            });
        }
        self.resolve_lock(options, &mut resolutions).await?;
        for (node, resolution) in self.nodes.iter().zip(&mut resolutions) {
            let has_code = !node.spec.code_pieces().is_empty();
            let wanted = options.resolve_root || !std::ptr::eq(node, &self.nodes[0]);
            if wanted && options.build_code && (!node.spec.tools.is_empty() || has_code) {
                resolution.artifacts =
                    crate::build::build_tools(&node.file, &node.spec, options.builder)?;
            }
        }
        for (node, resolution) in self.nodes.iter_mut().zip(resolutions) {
            node.resolution = Some(resolution);
        }
        Ok(())
    }

    #[cfg(feature = "packages")]
    async fn resolve_lock(
        &self,
        options: &ResolveOptions<'_>,
        resolutions: &mut [Resolution],
    ) -> Result<(), String> {
        use grauwolf_toolpkg::tree_lock::{LockNode, LockOptions, lock_tree};
        let nodes: Vec<LockNode<'_>> = self
            .nodes
            .iter()
            .map(|node| LockNode {
                file: &node.file,
                relative: &node.relative,
                spec: &node.spec,
            })
            .collect();
        let locked = lock_tree(
            &nodes,
            &LockOptions {
                builder: options.builder,
                locked: options.locked,
                config: options.config,
                resolve_root: options.resolve_root,
            },
        )
        .await?;
        for (resolution, locked) in resolutions.iter_mut().zip(locked) {
            resolution.package_tools = locked.package_tools;
            for found in locked.skills {
                let dir = found
                    .dir
                    .canonicalize()
                    .map_err(|error| format!("{}: {error}", found.dir.display()))?;
                crate::skills::replace_with_dir(
                    &mut resolution.spec.skills[found.index],
                    dir.to_string_lossy().into_owned(),
                );
                resolution.skills.push(ResolvedSkill {
                    index: found.index,
                    key: found.key,
                    commit: found.commit,
                    dir: found.dir,
                });
            }
        }
        Ok(())
    }

    #[cfg(not(feature = "packages"))]
    async fn resolve_lock(
        &self,
        _options: &ResolveOptions<'_>,
        _resolutions: &mut [Resolution],
    ) -> Result<(), String> {
        for node in &self.nodes {
            if !node.spec.tool_packages.is_empty() {
                return Err(format!(
                    "{}: `tool_packages` need the Cargo feature `packages` of grauwolf",
                    node.file.display()
                ));
            }
            if node.spec.skills.iter().any(|source| source.git.is_some()) {
                return Err(format!(
                    "{}: git `skills` need the Cargo feature `packages` of grauwolf",
                    node.file.display()
                ));
            }
        }
        Ok(())
    }
}

/// Where the tool cache is in an image's `app/`, relative to it: the generated service builds its
/// loader for sub-agents on `<base dir>/.grauwolf/tools` (ADR-0096).
pub const APP_TOOL_CACHE: &str = ".grauwolf/tools";

/// The directory of the resolved git skills source `index` in an image, relative to its spec.
fn image_skills_dir(index: usize) -> String {
    format!("skills-git/{index}")
}

impl AgentTree {
    /// Copies the resolved tree into `app` (the `app/` directory of a generated image, whose
    /// content becomes `/app`), so that the service runs without network, toolchain or git:
    /// - every node's companion files and resolved git skills (as `skills-git/<index>` next to
    ///   the spec) at the node's place, relative to `app`;
    /// - the spec of every node but the root (the generated service embeds the root) as
    ///   the node's document with code files and tool files inlined and git skills replaced;
    /// - the tool-cache entries the sub-agents need in [`APP_TOOL_CACHE`]: their built code
    ///   (`artifacts/`, with the toolchain version of `builder` and the shared `Cargo.lock`) and
    ///   the packages their lock entries name;
    /// - the lockfile of the tree as `grauwolf.lock` without its `skills` section (the git skills
    ///   are copied as directories), when a sub-agent has tool packages.
    ///
    /// The tree must be resolved.
    pub fn copy_into(
        &self,
        app: &Path,
        builder: &grauwolf_toolbuild::ToolBuilder,
    ) -> Result<(), String> {
        std::fs::create_dir_all(app).map_err(|error| format!("{}: {error}", app.display()))?;
        for (index, node) in self.nodes.iter().enumerate() {
            let place = app.join(node.relative.parent().unwrap_or(Path::new("")));
            for path in &node.companions {
                let source = node.base_dir().join(path);
                if source.exists() {
                    copy_recursively(&source, &place.join(path))?;
                }
            }
            for skill in node.resolution.iter().flat_map(|r| &r.skills) {
                copy_recursively(&skill.dir, &place.join(image_skills_dir(skill.index)))?;
            }
            if index > 0 {
                let target = app.join(&node.relative);
                let text = node.image_document()?;
                std::fs::create_dir_all(target.parent().unwrap_or(app))
                    .and_then(|()| std::fs::write(&target, text))
                    .map_err(|error| format!("{}: {error}", target.display()))?;
            }
        }
        self.copy_tool_cache(app, builder)
    }

    fn copy_tool_cache(
        &self,
        app: &Path,
        builder: &grauwolf_toolbuild::ToolBuilder,
    ) -> Result<(), String> {
        let cache = app.join(APP_TOOL_CACHE);
        let sub_nodes = || self.nodes.iter().skip(1);
        let artifacts: Vec<_> = sub_nodes()
            .filter_map(AgentNode::resolution)
            .flat_map(|resolution| &resolution.artifacts)
            .collect();
        if !artifacts.is_empty() {
            let dir = cache.join("artifacts");
            std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
            for artifact in artifacts {
                let target = dir.join(format!("{}.wasm", artifact.hash));
                std::fs::write(&target, &artifact.bytes)
                    .map_err(|error| format!("{}: {error}", target.display()))?;
            }
            builder
                .export_toolchain(&cache)
                .map_err(|error| error.to_string())?;
            // `prepare()` of the loader in the image finds the shared lock and does nothing.
            let lock = builder.cache_dir().join("Cargo.lock");
            if lock.is_file() {
                std::fs::copy(&lock, cache.join("Cargo.lock"))
                    .map_err(|error| format!("{}: {error}", lock.display()))?;
            }
        }
        if sub_nodes().any(|node| !node.spec.tool_packages.is_empty()) {
            self.copy_lockfile(app)?;
            self.copy_packages(&cache, builder.cache_dir())?;
        }
        Ok(())
    }

    /// The lockfile of the tree into `app`, without the `skills` section: the git skills travel
    /// as directories and the copied specs name those, so no node of the copy claims the
    /// entries (they would be stale under `--locked`).
    #[cfg(feature = "packages")]
    fn copy_lockfile(&self, app: &Path) -> Result<(), String> {
        let mut lock = grauwolf_toolpkg::Lockfile::load(&self.lock_path())
            .map_err(|error| error.to_string())?;
        lock.skills.clear();
        lock.save(&app.join(LOCKFILE))
            .map_err(|error| error.to_string())
    }

    #[cfg(not(feature = "packages"))]
    fn copy_lockfile(&self, _app: &Path) -> Result<(), String> {
        Ok(())
    }

    /// The cache entries of the packages that the lock entries of the sub-agents name.
    #[cfg(feature = "packages")]
    fn copy_packages(&self, cache: &Path, from: &Path) -> Result<(), String> {
        let lock = grauwolf_toolpkg::Lockfile::load(&self.lock_path())
            .map_err(|error| error.to_string())?;
        let others: Vec<String> = self
            .nodes
            .iter()
            .skip(1)
            .map(|node| lock_key_prefix(&node.relative))
            .collect();
        let others: Vec<&str> = others.iter().map(String::as_str).collect();
        let mut entries = Vec::new();
        for node in self.nodes.iter().skip(1) {
            let view = lock_view(&lock, Some(&lock_key_prefix(&node.relative)), &others);
            entries.extend(
                view.packages
                    .values()
                    .map(|package| package.digest.replace(':', "-")),
            );
            entries.extend(view.git.values().map(|git| format!("git-{}", git.commit)));
        }
        let packages = from.join("packages");
        let Ok(listing) = std::fs::read_dir(&packages) else {
            return Ok(());
        };
        for entry in listing.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // Git entries are named `git-<commit>` and, for a folder, `git-<commit>-<hash>`.
            if entries
                .iter()
                .any(|wanted| name == *wanted || name.starts_with(&format!("{wanted}-")))
            {
                copy_recursively(&entry.path(), &cache.join("packages").join(&name))?;
            }
        }
        Ok(())
    }

    #[cfg(not(feature = "packages"))]
    fn copy_packages(&self, _cache: &Path, _from: &Path) -> Result<(), String> {
        Ok(())
    }
}

fn copy_recursively(source: &Path, target: &Path) -> Result<(), String> {
    let fail = |error: std::io::Error| format!("{}: {error}", source.display());
    if source.is_dir() {
        std::fs::create_dir_all(target).map_err(fail)?;
        for entry in std::fs::read_dir(source).map_err(fail)? {
            let entry = entry.map_err(fail)?;
            copy_recursively(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else {
        std::fs::create_dir_all(target.parent().unwrap_or(Path::new("."))).map_err(fail)?;
        std::fs::copy(source, target).map_err(fail)?;
    }
    Ok(())
}
