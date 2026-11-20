//! Reading a spec file with its file references (ADR-0073, ADR-0102): `tool_files` and
//! `code_file` replaced by their content, warnings as lines.

use std::path::Path;

use grauwolf_spec::{AgentSpec, AgentSpecExt};

/// Message for `file`: with pointer as `<file>:<pointer>: …`, otherwise `<file>: …` (ADR-0080).
fn located(file: &Path, error: impl std::fmt::Display) -> String {
    let text = error.to_string();
    if text.starts_with('/') {
        format!("{}:{text}", file.display())
    } else {
        format!("{}: {text}", file.display())
    }
}

/// Reads a spec from `file` and resolves tool files (ADR-0073) without validating it – e.g.
/// to report all errors of a spec at once.
pub fn read_spec(file: &Path) -> Result<AgentSpec, String> {
    let json = resolved_json(file)?;
    AgentSpec::from_json(&json).map_err(|error| located(file, error))
}

/// The text of the spec `file` with the file references of the tools resolved (ADR-0073):
/// `tool_files` and `code_file` replaced by their content. Text that is no JSON stays as it is,
/// `from_json` reports it with its usual error message.
pub(crate) fn resolved_json(file: &Path) -> Result<String, String> {
    let json = std::fs::read_to_string(file)
        .map_err(|error| format!("{}: not readable: {error}", file.display()))?;
    Ok(match serde_json::from_str::<serde_json::Value>(&json) {
        Ok(mut document) => {
            let base = file.parent().unwrap_or(Path::new("."));
            resolve_tool_files(&mut document, base).map_err(|error| located(file, error))?;
            document.to_string()
        }
        Err(_) => json,
    })
}

/// The warnings of `spec` as lines `warning: <file>:<pointer>: <message>`.
pub fn warning_lines(file: &Path, spec: &AgentSpec) -> Vec<String> {
    spec.warnings()
        .iter()
        .map(|warning| format!("warning: {}:{warning}", file.display()))
        .collect()
}
/// Resolves the file references of a spec document in memory, as if its file were in `base`:
/// `tool_files` are appended to `tools`, every `code_file` (tools, `code` nodes, `on_code`
/// conditions, edge transformations) replaced by the file's content (ADR-0073, ADR-0102).
/// Errors start with the JSON pointer: `/<pointer>: <message>`.
pub fn resolve_file_references(
    document: &mut serde_json::Value,
    base: &Path,
) -> Result<(), String> {
    resolve_tool_files(document, base)
}

/// What a file a spec names is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceKind {
    /// `code_file` of a tool (in `tools` or in a tool file).
    ToolCode,
    /// An entry of `tool_files`: a whole tool definition.
    ToolFile,
    /// `code_file` of a `code` node.
    NodeCode,
    /// `code_file` of an `on_code` condition.
    ConditionCode,
    /// `code_file` of an edge's `transform`.
    TransformCode,
    /// `tools_dir`: tools read before every run.
    ToolsDir,
    /// `agent_tools.<name>.spec`: a local sub-agent.
    SubAgent,
    /// A path the service reads at run time ([`grauwolf_generator::companions`]).
    Companion(grauwolf_generator::CompanionKind),
}

/// A file or directory a spec names, found the way the reader resolves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReference {
    /// JSON pointer of the field; for the `code_file` inside a tool file
    /// `/tool_files/<i>/code_file`.
    pub pointer: String,
    /// The path as written.
    pub value: String,
    /// The path joined to the folder it is relative to (the spec's, or the tool file's).
    pub path: std::path::PathBuf,
    /// What it is for.
    pub kind: ReferenceKind,
}

/// Every file a spec document names, as if its file were in `base`, whether the file exists or
/// not: the code files and tool files the reader resolves (the same walk as
/// [`resolve_file_references`], a `code_file` in a tool file relative to that file), and – if the
/// spec loads once missing code is taken as empty – `tools_dir`, the local sub-agent specs and the
/// companion paths ([`grauwolf_generator::companions`]). URLs are left out.
pub fn file_references(document: &serde_json::Value, base: &Path) -> Vec<FileReference> {
    let mut lister = Lister::default();
    let mut document = document.clone();
    // A malformed field (e.g. `tool_files` not a list) ends the walk; what was found stays.
    let _ = walk_code_files(&mut document, base, &mut lister);
    let mut references = lister.found;
    let Ok(spec) = AgentSpec::from_json(&document.to_string()) else {
        return references;
    };
    let mut add = |pointer: String, value: &str, kind| {
        if !value.contains("://") {
            references.push(FileReference {
                pointer,
                value: value.to_owned(),
                path: base.join(value),
                kind,
            });
        }
    };
    if let Some(dir) = &spec.tools_dir {
        add("/tools_dir".into(), dir, ReferenceKind::ToolsDir);
    }
    for (name, tool) in &spec.agent_tools {
        if let Some(sub) = &tool.spec {
            let pointer = format!(
                "/agent_tools/{}/spec",
                grauwolf_spec::escape_pointer_token(name)
            );
            add(pointer, sub, ReferenceKind::SubAgent);
        }
    }
    for companion in grauwolf_generator::companions(&spec) {
        add(
            companion.pointer,
            &companion.path,
            ReferenceKind::Companion(companion.kind),
        );
    }
    references
}

/// What the walk over the code files does at each place: the reader inlines the files, the
/// lister records them.
trait CodeFiles {
    /// The object `holder` (a tool, the `params` of a `code` node or `on_code` condition, a
    /// `transform`) may name a `code_file` relative to `base`.
    fn code_file(
        &mut self,
        holder: &mut serde_json::Value,
        base: &Path,
        pointer: &str,
        kind: ReferenceKind,
    ) -> Result<(), String>;

    /// The tool file at `path` (entry `pointer` of `tool_files`): its definition, or `None` to
    /// skip it.
    fn tool_file(
        &mut self,
        path: &Path,
        relative: &str,
        pointer: &str,
    ) -> Result<Option<serde_json::Value>, String>;
}

/// The reader: inlines every file, a missing one is an error.
struct Resolver;

impl CodeFiles for Resolver {
    fn code_file(
        &mut self,
        holder: &mut serde_json::Value,
        base: &Path,
        pointer: &str,
        _kind: ReferenceKind,
    ) -> Result<(), String> {
        resolve_code_file(holder, base, pointer)
    }

    fn tool_file(
        &mut self,
        path: &Path,
        _relative: &str,
        pointer: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{pointer}: {}: {error}", path.display()))?;
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| format!("{pointer}: {}: not JSON: {error}", path.display()))
    }
}

/// The lister: records every file and puts empty code in place of each `code_file`, so the
/// document can still be loaded when files are missing.
#[derive(Default)]
struct Lister {
    found: Vec<FileReference>,
}

impl CodeFiles for Lister {
    fn code_file(
        &mut self,
        holder: &mut serde_json::Value,
        base: &Path,
        pointer: &str,
        kind: ReferenceKind,
    ) -> Result<(), String> {
        let Some(object) = holder.as_object_mut() else {
            return Ok(());
        };
        let Some(relative) = object
            .remove("code_file")
            .and_then(|value| value.as_str().map(str::to_owned))
        else {
            return Ok(());
        };
        object
            .entry("code")
            .or_insert_with(|| serde_json::Value::String(String::new()));
        self.found.push(FileReference {
            pointer: format!("{pointer}/code_file"),
            path: base.join(&relative),
            value: relative,
            kind,
        });
        Ok(())
    }

    fn tool_file(
        &mut self,
        path: &Path,
        relative: &str,
        pointer: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        self.found.push(FileReference {
            pointer: pointer.to_owned(),
            value: relative.to_owned(),
            path: path.to_owned(),
            kind: ReferenceKind::ToolFile,
        });
        Ok(std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok()))
    }
}

/// Replaces `code_file` of the tools in `document` with the content of the file (relative to
/// `base`); errors name the JSON pointer (ADR-0073).
fn resolve_tool_files(document: &mut serde_json::Value, base: &Path) -> Result<(), String> {
    walk_code_files(document, base, &mut Resolver)
}

/// The places a spec takes code or tools from files (ADR-0073, ADR-0102): `tool_files` (appended
/// to `tools`, their `code_file` relative to the tool file), the `code_file` of every tool, and
/// the code of the graphs.
fn walk_code_files(
    document: &mut serde_json::Value,
    base: &Path,
    visitor: &mut impl CodeFiles,
) -> Result<(), String> {
    let Some(object) = document.as_object_mut() else {
        return Ok(());
    };
    // `tool_files`: whole tool definitions from separate files, `code_file` relative to them.
    let mut appended = Vec::new();
    if let Some(files) = object.remove("tool_files") {
        let files = files
            .as_array()
            .ok_or("/tool_files: expected a list of paths")?
            .clone();
        for (index, relative) in files.iter().enumerate() {
            let pointer = format!("/tool_files/{index}");
            let relative = relative
                .as_str()
                .ok_or_else(|| format!("{pointer}: expected a path as text"))?;
            let path = base.join(relative);
            let Some(mut tool) = visitor.tool_file(&path, relative, &pointer)? else {
                continue;
            };
            let tool_base = path.parent().unwrap_or(base);
            visitor.code_file(&mut tool, tool_base, &pointer, ReferenceKind::ToolCode)?;
            appended.push(tool);
        }
    }
    if let Some(tools) = object
        .get_mut("tools")
        .and_then(serde_json::Value::as_array_mut)
    {
        for (index, tool) in tools.iter_mut().enumerate() {
            visitor.code_file(
                tool,
                base,
                &format!("/tools/{index}"),
                ReferenceKind::ToolCode,
            )?;
        }
    }
    walk_graph_code_files(object, base, visitor)?;
    if !appended.is_empty() {
        let tools = object
            .entry("tools")
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        if let Some(tools) = tools.as_array_mut() {
            tools.extend(appended);
        }
    }
    Ok(())
}

/// The code of the graphs (ADR-0102): the `params` of `code` nodes and `on_code` conditions and
/// the `transform` of edges, in the main graph and in every subgraph.
fn walk_graph_code_files(
    document: &mut serde_json::Map<String, serde_json::Value>,
    base: &Path,
    visitor: &mut impl CodeFiles,
) -> Result<(), String> {
    if let Some(graph) = document.get_mut("graph") {
        walk_code_files_in_graph(graph, base, "/graph", visitor)?;
    }
    if let Some(subgraphs) = document
        .get_mut("subgraphs")
        .and_then(serde_json::Value::as_object_mut)
    {
        for (name, subgraph) in subgraphs {
            if let Some(graph) = subgraph.get_mut("graph") {
                let pointer = format!(
                    "/subgraphs/{}/graph",
                    grauwolf_spec::escape_pointer_token(name)
                );
                walk_code_files_in_graph(graph, base, &pointer, visitor)?;
            }
        }
    }
    Ok(())
}

fn walk_code_files_in_graph(
    graph: &mut serde_json::Value,
    base: &Path,
    pointer: &str,
    visitor: &mut impl CodeFiles,
) -> Result<(), String> {
    if let Some(nodes) = graph
        .get_mut("nodes")
        .and_then(serde_json::Value::as_array_mut)
    {
        for (index, node) in nodes.iter_mut().enumerate() {
            if node.get("kind").and_then(serde_json::Value::as_str) == Some("code")
                && let Some(params) = node.get_mut("params")
            {
                visitor.code_file(
                    params,
                    base,
                    &format!("{pointer}/nodes/{index}/params"),
                    ReferenceKind::NodeCode,
                )?;
            }
        }
    }
    if let Some(edges) = graph
        .get_mut("edges")
        .and_then(serde_json::Value::as_array_mut)
    {
        for (index, edge) in edges.iter_mut().enumerate() {
            let edge_pointer = format!("{pointer}/edges/{index}");
            if let Some(when) = edge.get_mut("when")
                && when.get("kind").and_then(serde_json::Value::as_str) == Some("on_code")
                && let Some(params) = when.get_mut("params")
            {
                visitor.code_file(
                    params,
                    base,
                    &format!("{edge_pointer}/when/params"),
                    ReferenceKind::ConditionCode,
                )?;
            }
            if let Some(transform) = edge.get_mut("transform") {
                visitor.code_file(
                    transform,
                    base,
                    &format!("{edge_pointer}/transform"),
                    ReferenceKind::TransformCode,
                )?;
            }
        }
    }
    Ok(())
}

/// Reads a tool's `code_file` into `code`; exactly one of the two is allowed. The loader, the tools
/// directory and the agent tree read tool files.
pub fn resolve_code_file(
    tool: &mut serde_json::Value,
    base: &std::path::Path,
    pointer: &str,
) -> Result<(), String> {
    let Some(object) = tool.as_object_mut() else {
        return Ok(());
    };
    let Some(code_file) = object.remove("code_file") else {
        return Ok(());
    };
    if object.contains_key("code") {
        return Err(format!(
            "{pointer}: give exactly one of `code` and `code_file`"
        ));
    }
    let relative = code_file
        .as_str()
        .ok_or_else(|| format!("{pointer}/code_file: expected a path as text"))?;
    let path = base.join(relative);
    let code = std::fs::read_to_string(&path)
        .map_err(|error| format!("{pointer}/code_file: {}: {error}", path.display()))?;
    object.insert("code".into(), serde_json::Value::String(code));
    Ok(())
}
