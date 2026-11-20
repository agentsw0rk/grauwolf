//! `grauwolf tools build <crate-dir>` / `grauwolf-tools build <crate-dir>`: builds the WASM modules of a crate that was generated with
//! `--tool-sources` (ADR-0095). Every tool is built from its guest crate `tools/<name>/` with the
//! isolated tool build (ADR-0030) after its hash was checked against `tools/tools.json`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use grauwolf_toolbuild::{BuildError, ToolBuilder, source_hash};
use grauwolf_tools::{TOOL_SOURCES_FILE, ToolSourceCrate, ToolSourceEntry, ToolSourcesManifest};
use serde_json::{Value, json};

use crate::output::Io;
use crate::{errln, outln};

/// How `tools build` reports.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    /// One line per tool; errors with their position in the tool code.
    Text,
    /// `{ "tools": [ { "name", "kind", "ok", "skipped", "errors": [ { "message", "line"?, "column"? } ] } ] }`
    /// on standard output; `kind` is `tool`, `code`, `condition` or `transform` (ADR-0102, the
    /// name of a code piece is derived from its node or edge, e.g. `code_main_score`); `line` and
    /// `column` (from 1) refer to the code, the body of the function, as in the spec.
    Json,
}

/// One compiler or build error of a tool.
struct ToolError {
    message: String,
    line: Option<usize>,
    column: Option<usize>,
}

impl ToolError {
    fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            line: None,
            column: None,
        }
    }
}

enum Outcome {
    Built,
    Skipped,
    Failed(Vec<ToolError>),
}

/// Stamp next to a module: the source hash it was built from. Without it (or with another hash)
/// the module is rebuilt.
fn stamp_path(module: &Path) -> PathBuf {
    let mut name = module.as_os_str().to_owned();
    name.push(".src-sha256");
    PathBuf::from(name)
}

pub fn run(
    io: &Io,
    dir: &Path,
    format: Format,
    force: bool,
    builder: &ToolBuilder,
) -> Result<ExitCode, String> {
    let manifest_path = dir.join(TOOL_SOURCES_FILE);
    let json = std::fs::read_to_string(&manifest_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!(
                "{}: not found – `tools build` needs a crate generated with \
                 `grauwolf spec generate --tool-sources`",
                manifest_path.display()
            )
        } else {
            format!("{}: {error}", manifest_path.display())
        }
    })?;
    let manifest = ToolSourcesManifest::from_json(&json)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;

    let mut prepared = false;
    let mut results = Vec::new();
    for entry in &manifest.tools {
        let outcome = match read_and_check(dir, entry, force) {
            Err(errors) => Outcome::Failed(errors),
            Ok(None) => Outcome::Skipped,
            Ok(Some(source)) => {
                if !prepared {
                    builder
                        .prepare()
                        .map_err(|error| format!("tool dependencies not prepared: {error}"))?;
                    prepared = true;
                }
                build(dir, entry, &source, builder)
            }
        };
        results.push((entry, outcome));
    }

    let failed = results
        .iter()
        .any(|(_, outcome)| matches!(outcome, Outcome::Failed(_)));
    match format {
        Format::Json => outln!(io, "{}", render_json(&results)),
        Format::Text => render_text(io, &results),
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// The sources of `entry`, checked against the manifest; `None` if the module is up to date.
fn read_and_check(
    dir: &Path,
    entry: &ToolSourceEntry,
    force: bool,
) -> Result<Option<ToolSourceCrate>, Vec<ToolError>> {
    let name = &entry.name;
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        || entry.module != format!("tools/{name}.wasm")
    {
        return Err(vec![ToolError::plain(format!(
            "{TOOL_SOURCES_FILE}: invalid entry for tool '{name}'"
        ))]);
    }
    let read = |relative: &str| {
        let path = dir.join("tools").join(name).join(relative);
        std::fs::read_to_string(&path)
            .map_err(|error| vec![ToolError::plain(format!("{}: {error}", path.display()))])
    };
    let manifest = read("Cargo.toml")?;
    let source = read("src/lib.rs")?;
    let hash = source_hash(&manifest, &source);
    if hash != entry.sha256 {
        return Err(vec![ToolError::plain(format!(
            "the sources of tool '{name}' changed since the crate was generated (hash {hash}, \
             {TOOL_SOURCES_FILE} says {}); generate the crate again instead of editing the tool \
             sources",
            entry.sha256
        ))]);
    }
    let module = dir.join(&entry.module);
    let up_to_date = module.is_file()
        && std::fs::read_to_string(stamp_path(&module)).is_ok_and(|stamp| stamp.trim() == hash);
    if up_to_date && !force {
        return Ok(None);
    }
    Ok(Some(ToolSourceCrate {
        name: name.clone(),
        kind: entry.kind,
        manifest,
        source,
        hash,
    }))
}

fn build(
    dir: &Path,
    entry: &ToolSourceEntry,
    source: &ToolSourceCrate,
    builder: &ToolBuilder,
) -> Outcome {
    let artifact = match builder.build_sources(source) {
        Ok(artifact) => artifact,
        Err(BuildError::Compile {
            diagnostics,
            output,
            ..
        }) => {
            let mut errors: Vec<ToolError> = diagnostics
                .into_iter()
                .map(|diagnostic| ToolError {
                    message: diagnostic.message,
                    line: diagnostic.line,
                    column: diagnostic.column,
                })
                .collect();
            if errors.is_empty() {
                errors.push(ToolError::plain(output.trim().to_owned()));
            }
            return Outcome::Failed(errors);
        }
        Err(error) => return Outcome::Failed(vec![ToolError::plain(error.to_string())]),
    };
    let module = dir.join(&entry.module);
    // Via a temporary file and `rename`: a module is never half written.
    let write = || -> std::io::Result<()> {
        let partial = module.with_extension("wasm.partial");
        std::fs::write(&partial, &artifact.bytes)?;
        std::fs::rename(&partial, &module)?;
        std::fs::write(stamp_path(&module), &source.hash)
    };
    match write() {
        Ok(()) => Outcome::Built,
        Err(error) => Outcome::Failed(vec![ToolError::plain(format!(
            "{}: {error}",
            module.display()
        ))]),
    }
}

fn render_text(io: &Io, results: &[(&ToolSourceEntry, Outcome)]) {
    for (entry, outcome) in results {
        match outcome {
            Outcome::Built => outln!(io, "{}: built {}", entry.name, entry.module),
            Outcome::Skipped => outln!(
                io,
                "{}: {} is up to date (skipped; --force rebuilds)",
                entry.name,
                entry.module
            ),
            Outcome::Failed(errors) => {
                for error in errors {
                    match (error.line, error.column) {
                        (Some(line), Some(column)) => errln!(
                            io,
                            "{}: line {line}, column {column}: {}",
                            entry.name,
                            error.message
                        ),
                        _ => errln!(io, "{}: {}", entry.name, error.message),
                    }
                }
            }
        }
    }
}

fn render_json(results: &[(&ToolSourceEntry, Outcome)]) -> String {
    let tools: Vec<Value> = results
        .iter()
        .map(|(entry, outcome)| {
            let errors: Vec<Value> = match outcome {
                Outcome::Failed(errors) => errors
                    .iter()
                    .map(|error| {
                        let mut value = json!({ "message": error.message });
                        if let (Some(line), Some(column)) = (error.line, error.column) {
                            value["line"] = json!(line);
                            value["column"] = json!(column);
                        }
                        value
                    })
                    .collect(),
                _ => Vec::new(),
            };
            json!({
                "name": entry.name,
                "kind": entry.kind,
                "ok": !matches!(outcome, Outcome::Failed(_)),
                "skipped": matches!(outcome, Outcome::Skipped),
                "errors": errors,
            })
        })
        .collect();
    serde_json::to_string_pretty(&json!({ "tools": tools })).expect("JSON value")
}
