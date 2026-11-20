//! Built-in tools (F03.13, F03.14, ADR-0046): files under a root and user input/output
//! via the [`UserIo`] port.

mod kv;
mod skills;
mod think;
mod todo;

use std::path::{Component, Path, PathBuf};

pub use kv::{KV_TOOLS, KvTools, MAX_KEY_BYTES, MAX_VALUE_BYTES};
pub use skills::{MAX_SKILL_FILE_BYTES, SKILL_FILE, SkillLibrary, SkillRoot};
use std::sync::Arc;
pub use think::{THINK_TOOLS, think_tool};
pub use todo::{TODO_STATE_KEY, TODO_TOOLS, todo_tools};

use futures_util::future::BoxFuture;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

/// Names of the file tools.
pub const FILE_TOOLS: &[&str] = &[
    "read_file",
    "list_directory",
    "write_file",
    "edit_file",
    "search_files",
];

/// Matches of the regex search without `max_results`.
const DEFAULT_SEARCH_RESULTS: usize = 50;
/// At most this many regex search matches.
const MAX_SEARCH_RESULTS: usize = 200;
/// The regex search does not scan larger files.
const MAX_SEARCH_FILE_BYTES: u64 = 1024 * 1024;
/// Length to which the regex search truncates matching lines.
const MAX_SEARCH_LINE: usize = 200;

/// File tools that only read and write below `root`.
#[derive(Debug, Clone)]
pub struct FileTools {
    root: PathBuf,
}

impl FileTools {
    /// File tools under `root`; the directory must exist.
    pub fn new(root: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Self {
            root: root.as_ref().canonicalize()?,
        })
    }

    /// The tools with the names `names`; unknown names are an error.
    pub fn tools<'n>(
        &self,
        names: impl IntoIterator<Item = &'n str>,
    ) -> Result<Vec<Arc<dyn DynTool>>, String> {
        names
            .into_iter()
            .map(|name| {
                let kind = match name {
                    "read_file" => FileToolKind::Read,
                    "list_directory" => FileToolKind::List,
                    "write_file" => FileToolKind::Write,
                    "edit_file" => FileToolKind::Edit,
                    "search_files" => FileToolKind::Search,
                    other => return Err(format!("unknown built-in tool '{other}'")),
                };
                Ok(Arc::new(FileTool {
                    root: self.root.clone(),
                    kind,
                }) as Arc<dyn DynTool>)
            })
            .collect()
    }
}

/// Resolves the nearest existing ancestor of `path` (symlinks) and appends the rest;
/// so targets that do not exist yet cannot silently lead out through a symlink either.
fn canonicalize_existing(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut rest = Vec::new();
    loop {
        if let Ok(resolved) = existing.canonicalize() {
            return rest
                .iter()
                .rev()
                .fold(resolved, |path, part| path.join(part));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => return path.to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum FileToolKind {
    Read,
    List,
    Write,
    Edit,
    Search,
}

struct FileTool {
    root: PathBuf,
    kind: FileToolKind,
}

impl FileTool {
    /// Path under the root; relative paths without `..`; symlinks must not lead outside.
    fn resolve(&self, path: &str) -> Result<PathBuf, ToolError> {
        let relative = Path::new(path);
        if relative.is_absolute() {
            return Err(self.error(format!("path '{path}' must be relative to the root")));
        }
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        {
            return Err(self.error(format!("path '{path}' leads outside the root")));
        }
        let resolved = canonicalize_existing(&self.root.join(relative));
        if !resolved.starts_with(&self.root) {
            return Err(self.error(format!("path '{path}' leads outside the root")));
        }
        Ok(resolved)
    }

    /// Entries of `target`, sorted, directories with `/`.
    async fn list(&self, path: &str, target: &Path) -> Result<String, ToolError> {
        let failed = |error: std::io::Error| self.error(format!("{path}: {error}"));
        let mut entries = tokio::fs::read_dir(target).await.map_err(failed)?;
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(failed)? {
            let mut name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type().await.map_err(failed)?.is_dir() {
                name.push('/');
            }
            names.push(name);
        }
        names.sort();
        Ok(names.join("\n"))
    }

    /// Matches of `regex` in the text files under `target` (or in `target` itself), sorted
    /// by path and line; at most `limit` lines.
    async fn search(
        &self,
        target: &Path,
        regex: &regex::Regex,
        limit: usize,
    ) -> Result<String, ToolError> {
        let mut files = Vec::new();
        let mut pending = vec![target.to_owned()];
        while let Some(path) = pending.pop() {
            // Symlinks are not followed: they could lead out of the root.
            let Ok(metadata) = tokio::fs::symlink_metadata(&path).await else {
                continue;
            };
            if metadata.is_dir() {
                let Ok(mut entries) = tokio::fs::read_dir(&path).await else {
                    continue;
                };
                while let Ok(Some(entry)) = entries.next_entry().await {
                    if !entry.file_name().to_string_lossy().starts_with('.') {
                        pending.push(entry.path());
                    }
                }
            } else if metadata.is_file() && metadata.len() <= MAX_SEARCH_FILE_BYTES {
                files.push(path);
            }
        }
        let mut hits = Vec::new();
        for file in files {
            // Binary files and other encodings are skipped.
            let Ok(content) = tokio::fs::read_to_string(&file).await else {
                continue;
            };
            let relative = file
                .strip_prefix(&self.root)
                .unwrap_or(&file)
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            for (index, line) in content.lines().enumerate() {
                if regex.is_match(line) {
                    let text: String = line.trim().chars().take(MAX_SEARCH_LINE).collect();
                    hits.push((relative.clone(), index + 1, text));
                }
            }
        }
        if hits.is_empty() {
            return Ok("No matches.".to_owned());
        }
        hits.sort();
        let total = hits.len();
        let mut lines: Vec<String> = hits
            .into_iter()
            .take(limit)
            .map(|(path, line, text)| format!("{path}:{line}: {text}"))
            .collect();
        if total > limit {
            lines.push(format!(
                "… {} more matches cut off (raise max_results or search more narrowly)",
                total - limit
            ));
        }
        Ok(lines.join("\n"))
    }

    fn error(&self, message: String) -> ToolError {
        ToolError::Execution(message)
    }

    fn text<'a>(&self, args: &'a Value, field: &str) -> Result<&'a str, ToolError> {
        args[field].as_str().ok_or_else(|| ToolError::InvalidArgs {
            tool: self.name().to_owned(),
            reason: format!("missing field '{field}' (text)"),
        })
    }
}

impl DynTool for FileTool {
    fn name(&self) -> &str {
        match self.kind {
            FileToolKind::Read => "read_file",
            FileToolKind::List => "list_directory",
            FileToolKind::Write => "write_file",
            FileToolKind::Edit => "edit_file",
            FileToolKind::Search => "search_files",
        }
    }

    fn descriptor(&self) -> ToolDescriptor {
        let path =
            json!({ "type": "string", "description": "Path relative to the working directory" });
        let (description, parameters) = match self.kind {
            FileToolKind::Read => (
                "Reads a text file and returns its content.",
                json!({ "type": "object", "properties": { "path": path }, "required": ["path"] }),
            ),
            FileToolKind::List => (
                "Lists a directory, sorted; subdirectories end with `/`. \
                 The working directory itself is `.`.",
                json!({ "type": "object", "properties": { "path": path }, "required": ["path"] }),
            ),
            FileToolKind::Write => (
                "Writes a text file (creates directories, overwrites existing files).",
                json!({
                    "type": "object",
                    "properties": { "path": path, "content": { "type": "string" } },
                    "required": ["path", "content"]
                }),
            ),
            FileToolKind::Edit => (
                "Replaces the passage `old` with `new` in a text file; `old` must occur exactly \
                 once.",
                json!({
                    "type": "object",
                    "properties": {
                        "path": path,
                        "old": { "type": "string" },
                        "new": { "type": "string" }
                    },
                    "required": ["path", "old", "new"]
                }),
            ),
            FileToolKind::Search => (
                "Searches text files recursively for a regular expression (Rust syntax, \
                 `(?i)` ignores case) and returns matches as \
                 `path:line: text`. Hidden directories, symlinks, binary files and files \
                 over 1 MiB are skipped.",
                json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Regular expression" },
                        "path": {
                            "type": "string",
                            "description": "Directory or file relative to the working directory; default `.`"
                        },
                        "max_results": { "type": "integer", "minimum": 1, "maximum": MAX_SEARCH_RESULTS }
                    },
                    "required": ["pattern"]
                }),
            ),
        };
        ToolDescriptor {
            name: self.name().to_owned(),
            description: description.to_owned(),
            parameters,
        }
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let path = match self.kind {
                FileToolKind::Search => args["path"].as_str().unwrap_or("."),
                _ => self.text(&args, "path")?,
            };
            let target = self.resolve(path)?;
            match self.kind {
                FileToolKind::Search => {
                    let pattern = self.text(&args, "pattern")?;
                    let regex =
                        regex::Regex::new(pattern).map_err(|error| ToolError::InvalidArgs {
                            tool: self.name().to_owned(),
                            reason: format!("pattern is not a valid regular expression: {error}"),
                        })?;
                    let limit = args["max_results"]
                        .as_u64()
                        .map_or(DEFAULT_SEARCH_RESULTS, |limit| {
                            usize::try_from(limit).unwrap_or(MAX_SEARCH_RESULTS)
                        })
                        .clamp(1, MAX_SEARCH_RESULTS);
                    self.search(&target, &regex, limit).await
                }
                FileToolKind::Read => tokio::fs::read_to_string(&target)
                    .await
                    .map_err(|error| self.error(format!("{path}: {error}"))),
                FileToolKind::List => self.list(path, &target).await,
                FileToolKind::Write => {
                    let content = self.text(&args, "content")?;
                    let failed = |error: std::io::Error| self.error(format!("{path}: {error}"));
                    if let Some(parent) = target.parent() {
                        tokio::fs::create_dir_all(parent).await.map_err(failed)?;
                    }
                    tokio::fs::write(&target, content).await.map_err(failed)?;
                    Ok(format!("{path} written ({} bytes)", content.len()))
                }
                FileToolKind::Edit => {
                    let (old, new) = (self.text(&args, "old")?, self.text(&args, "new")?);
                    let failed = |error: std::io::Error| self.error(format!("{path}: {error}"));
                    let content = tokio::fs::read_to_string(&target).await.map_err(failed)?;
                    match content.matches(old).count() {
                        1 => {
                            tokio::fs::write(&target, content.replacen(old, new, 1))
                                .await
                                .map_err(failed)?;
                            Ok(format!("{path} changed"))
                        }
                        0 => Err(self.error(format!("{path}: passage not found"))),
                        count => Err(self.error(format!(
                            "{path}: passage occurs {count} times, it must be unique"
                        ))),
                    }
                }
            }
        })
    }
}

/// Names of the user tools.
pub const USER_TOOLS: &[&str] = &["say_to_user", "ask_user"];

/// Port to the user: outputs and questions during a run.
pub trait UserIo: Send + Sync {
    /// Shows `message` to the user.
    fn say(&self, message: &str);

    /// Asks `question` and returns the answer.
    fn ask<'a>(&'a self, question: &'a str) -> BoxFuture<'a, Result<String, String>>;
}

/// Console: outputs and questions go to stderr (stdout stays reserved for the result);
/// answers come from stdin.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConsoleUserIo;

impl UserIo for ConsoleUserIo {
    fn say(&self, message: &str) {
        eprintln!("{message}");
    }

    fn ask<'a>(&'a self, question: &'a str) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            use tokio::io::AsyncBufReadExt;
            eprintln!("{question}");
            let mut line = String::new();
            tokio::io::BufReader::new(tokio::io::stdin())
                .read_line(&mut line)
                .await
                .map_err(|error| error.to_string())?;
            Ok(line.trim_end().to_owned())
        })
    }
}

/// Tools that talk to the user via a [`UserIo`].
#[derive(Clone)]
pub struct UserTools {
    io: Arc<dyn UserIo>,
}

impl UserTools {
    /// User tools over `io`.
    pub fn new(io: Arc<dyn UserIo>) -> Self {
        Self { io }
    }

    /// The tools with the names `names`; unknown names are an error.
    pub fn tools<'n>(
        &self,
        names: impl IntoIterator<Item = &'n str>,
    ) -> Result<Vec<Arc<dyn DynTool>>, String> {
        names
            .into_iter()
            .map(|name| {
                let ask = match name {
                    "say_to_user" => false,
                    "ask_user" => true,
                    other => return Err(format!("unknown built-in tool '{other}'")),
                };
                Ok(Arc::new(UserTool {
                    io: Arc::clone(&self.io),
                    ask,
                }) as Arc<dyn DynTool>)
            })
            .collect()
    }
}

struct UserTool {
    io: Arc<dyn UserIo>,
    ask: bool,
}

impl DynTool for UserTool {
    fn name(&self) -> &str {
        if self.ask { "ask_user" } else { "say_to_user" }
    }

    fn descriptor(&self) -> ToolDescriptor {
        let (description, field) = if self.ask {
            (
                "Asks the user a question and returns the answer.",
                "question",
            )
        } else {
            (
                "Shows the user a message, e.g. an interim status.",
                "message",
            )
        };
        ToolDescriptor {
            name: self.name().to_owned(),
            description: description.to_owned(),
            parameters: json!({
                "type": "object",
                "properties": { field: { "type": "string" } },
                "required": [field]
            }),
        }
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let field = if self.ask { "question" } else { "message" };
            let text = args[field].as_str().ok_or_else(|| ToolError::InvalidArgs {
                tool: self.name().to_owned(),
                reason: format!("missing field '{field}' (text)"),
            })?;
            if self.ask {
                self.io.ask(text).await.map_err(ToolError::Execution)
            } else {
                self.io.say(text);
                Ok("shown".to_owned())
            }
        })
    }
}
