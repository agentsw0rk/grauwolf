//! The agent tree of `agent deploy --spec` as a ConfigMap (ADR-0121 decision 2): every file of the
//! tree under a flat key, mounted back to its place with one `subPath` mount per file.
//!
//! `subPath` mounts, not a volume with `items`: Kubernetes projects the files of a ConfigMap volume
//! as symlinks into a hidden directory of the volume, and the skill check refuses a symlink that
//! leads out of its skill folder. A `subPath` mount is a plain file.
//!
//! The tree is read-only, but an agent writes into some of its folders (the root of the file tools,
//! the `tools_dir`). Such a relative folder becomes a directory of the data volume mounted at its
//! place (`subPath: files/<path>`), see [`AgentFiles::writable`].

use std::collections::BTreeMap;
use std::path::Path;

use grauwolf_agent_tree::{AgentNode, AgentTree, ReadOptions};
use grauwolf_spec::AgentSpecExt;
use serde_json::{Value, json};

use crate::agent::{MANAGED_BY, MANAGER};

/// The ConfigMap with the agent's files.
pub const AGENT_MAP: &str = "grauwolf-agent";
/// Where the tree is mounted (read-only) in the container.
pub const SPEC_DIR: &str = "/app/spec";
/// What a ConfigMap may hold: the API server's limit is 1 MiB for the whole object.
pub const MAX_BYTES: usize = 1_000_000;

const BUILD_HINT: &str = "build an image with `grauwolf spec generate --service --dockerfile` and deploy that without --spec";

/// Where the data volume is mounted.
pub const DATA_DIR: &str = "/data";
/// The directory of the data volume that holds the writable folders of the tree.
const DATA_FILES: &str = "files";

/// The files of an agent tree, relative to the root spec's directory, and the root spec's path.
pub struct AgentFiles {
    pub root: String,
    pub files: BTreeMap<String, Vec<u8>>,
    /// The folders of the tree the agent writes: relative to the root spec's directory, with what
    /// asks for them. Each is a directory of the data volume at its place under [`SPEC_DIR`].
    pub writable: Vec<Writable>,
    /// What deserves a warning (the deploy prints them).
    pub warnings: Vec<String>,
}

/// A writable folder of the tree.
pub struct Writable {
    pub path: String,
    /// `builtin_tools.root of team/helper.json`, comma separated if several ask for it.
    pub reason: String,
}

impl Writable {
    /// Where the agent sees it.
    pub fn mount_path(&self) -> String {
        format!("{SPEC_DIR}/{}", self.path)
    }

    /// Its place in the data volume.
    pub fn sub_path(&self) -> String {
        format!("{DATA_FILES}/{}", self.path)
    }
}

/// Where a spec names a directory.
enum Place {
    /// The spec's own folder.
    Own,
    /// A folder of the tree, relative to the root spec's directory.
    Inside(String),
    /// An absolute path: not part of the tree.
    Absolute,
}

impl AgentFiles {
    /// Reads the tree of `file`; refuses what cannot run from a ConfigMap (tool code to build,
    /// packages and git skills to fetch, files the agent writes into its own folder).
    pub fn read(file: &Path) -> Result<Self, String> {
        let tree = AgentTree::read(file, &ReadOptions::default()).map_err(|errors| {
            let mut lines = errors.errors;
            lines.extend(errors.warnings);
            lines.join("\n")
        })?;
        let mut files = BTreeMap::new();
        for node in tree.nodes() {
            refuse(node)?;
            let place = node.relative().parent().unwrap_or(Path::new(""));
            let spec_bytes = std::fs::read(node.file())
                .map_err(|error| format!("{}: {error}", node.file().display()))?;
            files.insert(slash(node.relative()), spec_bytes);
            for companion in node.companions() {
                collect(
                    &node.base_dir().join(companion),
                    &place.join(companion),
                    &mut files,
                )?;
            }
        }
        let root = slash(tree.root().relative());
        let (writable, warnings) = writable_places(&tree, &mut files)?;
        Ok(Self {
            root,
            files,
            writable,
            warnings,
        })
    }

    /// Where the root spec is in the container.
    pub fn root_path(&self) -> String {
        format!("{SPEC_DIR}/{}", self.root)
    }

    /// The ConfigMap, the `subPath` mounts and the size of what is stored; refuses a tree that
    /// does not fit.
    pub fn config_map(&self, namespace: &str) -> Result<(Value, Vec<Value>, usize), String> {
        let mut data = serde_json::Map::new();
        let mut binary = serde_json::Map::new();
        let mut mounts = Vec::new();
        let mut size = 0;
        for (index, (path, content)) in self.files.iter().enumerate() {
            let key = format!("{index:03}-{}", sanitize(path));
            match std::str::from_utf8(content) {
                Ok(text) => {
                    size += text.len();
                    data.insert(key.clone(), json!(text));
                }
                Err(_) => {
                    let encoded = base64(content);
                    size += encoded.len();
                    binary.insert(key.clone(), json!(encoded));
                }
            }
            mounts.push(
                json!({"name": "agent", "mountPath": format!("{SPEC_DIR}/{path}"),
                "subPath": key, "readOnly": true}),
            );
        }
        if size > MAX_BYTES {
            return Err(format!(
                "the agent tree is {} KiB: a ConfigMap holds about 1 MiB at most; {BUILD_HINT}",
                size.div_ceil(1024)
            ));
        }
        let mut object = json!({"apiVersion": "v1", "kind": "ConfigMap",
            "metadata": {"name": AGENT_MAP, "namespace": namespace, "labels": {MANAGED_BY: MANAGER}},
            "data": data});
        if !binary.is_empty() {
            object["binaryData"] = Value::Object(binary);
        }
        Ok((object, mounts, size))
    }
}

fn refuse(node: &AgentNode) -> Result<(), String> {
    let spec = node.spec();
    let file = node.file().display();
    let build = if !spec.tools.is_empty() {
        Some("tools with code")
    } else if !spec.code_pieces().is_empty() {
        Some("code in the graph")
    } else if !spec.tool_packages.is_empty() {
        Some("tool packages")
    } else if spec.skills.iter().any(|skill| skill.git.is_some()) {
        Some("skills from git")
    } else {
        None
    };
    if let Some(what) = build {
        return Err(format!(
            "{file}: {what} must be built or fetched, which a pod that only has the grauwolf image cannot do; {BUILD_HINT}"
        ));
    }
    Ok(())
}

/// The writable folders of the tree and the warnings about the paths that stay as they are.
///
/// A relative folder the agent writes becomes a writable place, unless the spec's own files lie in
/// it: the mount would hide them. The folders are the `builtin_tools.root` (one with files stays a
/// read-only folder of the spec, with a warning), the `tools_dir` (one with files is refused, as it
/// always was: its tools have to be built) and the folder of the `long_term_memory.store` and of the
/// `knowledge.index` (a file in a folder with spec files, or in the spec's own folder, is refused with
/// a hint). A copy of the store or the index that a local run left in the spec folder is not shipped.
fn writable_places(
    tree: &AgentTree,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(Vec<Writable>, Vec<String>), String> {
    let mut places: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut warnings = Vec::new();
    // The files the agent writes itself come first: removing them from `files` decides what else
    // lies in their folders.
    let mut state = Vec::new();
    for node in tree.nodes() {
        let file = slash(node.relative());
        let spec = node.spec();
        let wanted = [
            (
                "long_term_memory.store",
                spec.long_term_memory
                    .as_ref()
                    .and_then(|memory| memory.store.as_deref()),
            ),
            (
                "knowledge.index",
                spec.knowledge
                    .as_ref()
                    .and_then(|knowledge| knowledge.index.as_deref()),
            ),
        ];
        for (option, value) in wanted {
            let Some(value) = value else { continue };
            let place = locate(node, &file, option, value)?;
            if let Place::Inside(path) = &place {
                files.remove(path);
            }
            state.push((
                format!("{option} '{value}' ({file})"),
                format!("{option} of {file}"),
                value,
                node,
                place,
            ));
        }
    }
    for (what, reason, value, node, place) in state {
        match place {
            Place::Absolute => warn_if_outside_data(&mut warnings, &what, value, "a file"),
            Place::Own => {}
            Place::Inside(path) => {
                let name = path.rsplit('/').next().unwrap_or(&path);
                let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
                let hint =
                    format!("put the file in a sub folder of its own, for example 'state/{name}'");
                if folder == own_folder(node) {
                    return Err(format!(
                        "{what}: the file is in the folder of the spec, which cannot be a writable folder of the data volume; {hint}"
                    ));
                }
                let below = format!("{folder}/");
                if let Some(other) = files.keys().find(|key| key.starts_with(&below)) {
                    return Err(format!(
                        "{what}: the folder '{folder}' becomes a writable folder of the data volume, which would hide the file '{other}' of the spec; {hint}"
                    ));
                }
                places.entry(folder.to_owned()).or_default().push(reason);
            }
        }
    }
    for node in tree.nodes() {
        let file = slash(node.relative());
        let spec = node.spec();
        let wanted = [
            (
                "builtin_tools.root",
                spec.builtin_tools
                    .as_ref()
                    .and_then(|builtin| builtin.root.as_deref()),
            ),
            ("tools_dir", spec.tools_dir.as_deref()),
        ];
        for (option, value) in wanted {
            let Some(value) = value else { continue };
            match locate(node, &file, option, value)? {
                Place::Own => {}
                Place::Absolute => warn_if_outside_data(
                    &mut warnings,
                    &format!("{option} '{value}' ({file})"),
                    value,
                    "a folder",
                ),
                Place::Inside(path) => {
                    if option == "tools_dir"
                        && let Some(name) = first_entry(&node.base_dir().join(value))
                    {
                        return Err(format!(
                            "{file}: `tools_dir` '{value}' holds tool files ({}/{name}) that must be built, which a pod that only has the grauwolf image cannot do; {BUILD_HINT}",
                            value.trim_matches('/')
                        ));
                    }
                    let below = format!("{path}/");
                    match files.keys().find(|key| key.starts_with(&below)) {
                        Some(name) => warnings.push(format!(
                            "warning: {option} '{value}' ({file}) holds files of the spec ({name}), so it stays a read-only folder of the spec and the agent cannot write there; keep it empty to make it a writable folder of the data volume"
                        )),
                        None => places
                            .entry(path)
                            .or_default()
                            .push(format!("{option} of {file}")),
                    }
                }
            }
        }
    }
    let writable = places
        .into_iter()
        .map(|(path, reasons)| Writable {
            path,
            reason: reasons.join(", "),
        })
        .collect();
    Ok((writable, warnings))
}

/// An absolute path is used as it is: it must exist in the image or lie under the data volume.
fn warn_if_outside_data(warnings: &mut Vec<String>, what: &str, value: &str, kind: &str) {
    if value != DATA_DIR && !value.starts_with(&format!("{DATA_DIR}/")) {
        warnings.push(format!(
            "warning: {what} is an absolute path: it is used as it is, so {kind} must exist in the image or lie under {DATA_DIR}, the data volume"
        ));
    }
}

/// The folder of the spec of `node`, relative to the root spec's folder.
fn own_folder(node: &AgentNode) -> String {
    slash(node.relative().parent().unwrap_or(Path::new("")))
}

/// The name of one entry of the directory `dir`, if it has any.
fn first_entry(dir: &Path) -> Option<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()))
        .collect();
    names.sort();
    names.into_iter().next()
}

/// Where `value`, a path of the spec of `node`, points: normalized without looking at the disk,
/// and refused if it leaves the root spec's folder.
fn locate(node: &AgentNode, file: &str, option: &str, value: &str) -> Result<Place, String> {
    if value.starts_with('/') {
        return Ok(Place::Absolute);
    }
    let own: Vec<String> = own_folder(node)
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    let mut parts = own.clone();
    for part in value.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(format!(
                        "{file}: {option} '{value}' leaves the spec folder, which the agent cannot reach in the pod; use a relative path inside it, or an absolute path under {DATA_DIR}"
                    ));
                }
            }
            part => parts.push(part.to_owned()),
        }
    }
    Ok(if parts == own {
        Place::Own
    } else {
        Place::Inside(parts.join("/"))
    })
}

/// The file or, recursively, the files of the directory `source` at `target`; a missing path is
/// skipped (the spec may name files the agent creates).
fn collect(
    source: &Path,
    target: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    let fail = |error: std::io::Error| format!("{}: {error}", source.display());
    if source.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(source)
            .map_err(fail)?
            .collect::<Result<_, _>>()
            .map_err(fail)?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            collect(&entry.path(), &target.join(entry.file_name()), files)?;
        }
    } else if source.is_file() {
        files.insert(slash(target), std::fs::read(source).map_err(fail)?);
    }
    Ok(())
}

fn slash(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// A valid ConfigMap key from a path: letters, digits, `-`, `_` and `.`, at most 100 characters
/// (the index in front makes it unique).
fn sanitize(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .take(100)
        .collect()
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, byte)| n | u32::from(*byte) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}
