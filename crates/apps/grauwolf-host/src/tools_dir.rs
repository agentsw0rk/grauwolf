//! Tools directory that is loaded at runtime (ADR-0074).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use grauwolf_prompt::ToolDescriptor;
use grauwolf_sandbox::SandboxRuntime;
use grauwolf_spec::{AgentSpec, ToolSpec};
use grauwolf_toolbuild::ToolBuilder;
use grauwolf_tools::{DynTool, ToolRuntime};

/// A directory with tool files; each [`ToolsDir::load`] reads it anew.
pub struct ToolsDir {
    dir: PathBuf,
    builder: ToolBuilder,
    runtime: Arc<dyn ToolRuntime>,
    /// Names that the tools of the directory must not override.
    taken: Vec<String>,
}

/// Result of a load.
pub struct LoadedTools {
    /// Usable tools in the order of the file names.
    pub tools: Vec<Arc<dyn DynTool>>,
    /// Skipped files with reason, e.g. `upper.json: compile error …`.
    pub skipped: Vec<String>,
}

/// The names that the tools of a directory must not override: every tool of the spec's namespace
/// (ADR-0117) – own, external, package and self-build tools.
pub(crate) fn taken_names(spec: &AgentSpec) -> Vec<String> {
    spec.tool_namespace().names().map(str::to_owned).collect()
}

impl ToolsDir {
    /// Tools directory `dir`, built with `builder`; `taken` are the names of existing tools.
    pub fn new(
        dir: impl AsRef<Path>,
        builder: ToolBuilder,
        taken: Vec<String>,
    ) -> Result<Self, String> {
        let runtime = SandboxRuntime::new().map_err(|error| error.to_string())?;
        Ok(Self {
            dir: dir.as_ref().to_owned(),
            builder,
            runtime: Arc::new(runtime),
            taken,
        })
    }

    /// Reads all `*.json` of the directory, builds new and changed tools (cache via the
    /// checksum) and returns the usable ones; errors end nothing, they are listed in `skipped`.
    pub async fn load(&self) -> LoadedTools {
        let (dir, builder, taken) = (self.dir.clone(), self.builder.clone(), self.taken.clone());
        let built = tokio::task::spawn_blocking(move || build_all(&dir, &builder, &taken)).await;
        let (specs, mut skipped) = match built {
            Ok(result) => result,
            Err(error) => (Vec::new(), vec![format!("tool folder: {error}")]),
        };
        let mut tools = Vec::new();
        for (file, tool, artifact) in specs {
            let descriptor = ToolDescriptor {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.args.clone(),
            };
            match self.runtime.instantiate(&artifact, descriptor) {
                Ok(instance) => tools.push(instance),
                Err(error) => skipped.push(format!("{file}: {error}")),
            }
        }
        LoadedTools { tools, skipped }
    }
}

/// Built tools per file and skipped files with reason.
type Built = (
    Vec<(String, ToolSpec, grauwolf_tools::ToolArtifact)>,
    Vec<String>,
);

fn build_all(dir: &Path, builder: &ToolBuilder, taken: &[String]) -> Built {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect(),
        // No directory yet (e.g. before the first `self_tools.save`) means: no tools.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), Vec::new());
        }
        Err(error) => return (Vec::new(), vec![format!("{}: {error}", dir.display())]),
    };
    files.sort();
    let (mut built, mut skipped) = (Vec::new(), Vec::new());
    let mut names: Vec<String> = Vec::new();
    if let Err(error) = builder.prepare() {
        return (Vec::new(), vec![format!("tool dependencies: {error}")]);
    }
    for path in files {
        let file = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        match build_one(&path, builder) {
            Ok((tool, artifact)) => {
                if taken.contains(&tool.name) || names.contains(&tool.name) {
                    skipped.push(format!(
                        "{file}: tool '{}' already exists and is not overridden",
                        tool.name
                    ));
                    continue;
                }
                names.push(tool.name.clone());
                built.push((file, tool, artifact));
            }
            Err(reason) => skipped.push(format!("{file}: {reason}")),
        }
    }
    (built, skipped)
}

/// Reads, validates and builds the tool file `path`.
fn build_one(
    path: &Path,
    builder: &ToolBuilder,
) -> Result<(ToolSpec, grauwolf_tools::ToolArtifact), String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut document: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("not JSON: {error}"))?;
    let base = path.parent().unwrap_or(Path::new("."));
    grauwolf_agent_tree::resolve_code_file(&mut document, base, "")?;
    let tool: ToolSpec =
        serde_json::from_value(document).map_err(|error| format!("no tool definition: {error}"))?;
    let valid_name = tool
        .name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && tool
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !valid_name {
        return Err(format!(
            "tool name '{}' must be snake_case (a-z, 0-9, _)",
            tool.name
        ));
    }
    let artifact = builder.build(&tool).map_err(|error| error.to_string())?;
    Ok((tool, artifact))
}

#[cfg(test)]
mod tests {
    use grauwolf_spec::AgentSpecExt as _;

    use super::taken_names;

    #[test]
    fn directory_tools_must_not_override_any_tool_of_the_namespace() {
        let spec = grauwolf_spec::AgentSpec::from_json(
            &serde_json::json!({
                "spec_version": 1, "name": "agent",
                "model": { "provider": "ollama", "id": "m" },
                "tools": [{ "name": "own", "description": "d",
                    "args": { "type": "object" }, "code": "Ok(String::new())" }],
                "builtin_tools": { "tools": ["think"] },
                "tool_packages": [{ "oci": "ghcr.io/a/b:1", "tools": ["pkg"] }],
                "self_tools": {},
                "strategy": { "kind": "single_run" }
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(taken_names(&spec), ["own", "think", "pkg", "create_tool"]);
    }
}
