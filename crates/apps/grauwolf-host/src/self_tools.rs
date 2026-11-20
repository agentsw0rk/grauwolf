//! Factory for tools that agents create themselves during the run (ADR-0075): build with the
//! `ToolBuilder` (cache), execution in the WASM sandbox.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_sandbox::SandboxRuntime;
use grauwolf_spec::ToolSpec;
use grauwolf_toolbuild::ToolBuilder;
use grauwolf_tools::{DynTool, NewTool, ToolFactory, ToolRuntime};

/// Builds new tools into the sandbox; errors (also compile errors with line and column in the
/// tool code) come back as text for the model.
pub struct BuildingToolFactory {
    builder: ToolBuilder,
    runtime: Arc<dyn ToolRuntime>,
    /// Tools directory where built tools are placed (`self_tools.save`, ADR-0074).
    save_dir: Option<PathBuf>,
}

impl BuildingToolFactory {
    /// Factory that builds with `builder`.
    pub fn new(builder: ToolBuilder) -> Result<Self, String> {
        let runtime = SandboxRuntime::new().map_err(|error| error.to_string())?;
        Ok(Self {
            builder,
            runtime: Arc::new(runtime),
            save_dir: None,
        })
    }

    /// Stores each built tool as `<name>.json` + `<name>.rs` in `dir` (format of
    /// ADR-0073), so that later runs have it via the tools directory.
    pub fn with_save_dir(mut self, dir: impl AsRef<Path>) -> Self {
        self.save_dir = Some(dir.as_ref().to_owned());
        self
    }
}

/// Writes `tool` into the tools directory `dir`.
fn save(dir: &Path, tool: &ToolSpec) -> Result<(), String> {
    let fail = |error: std::io::Error| format!("{}: {error}", dir.display());
    std::fs::create_dir_all(dir).map_err(fail)?;
    let code_file = format!("{}.rs", tool.name);
    std::fs::write(dir.join(&code_file), &tool.code).map_err(fail)?;
    let definition = serde_json::json!({
        "name": tool.name,
        "description": tool.description,
        "args": tool.args,
        "code_file": code_file,
    });
    let text = serde_json::to_string_pretty(&definition).map_err(|error| error.to_string())?;
    std::fs::write(dir.join(format!("{}.json", tool.name)), text).map_err(fail)
}

impl ToolFactory for BuildingToolFactory {
    fn create(&self, tool: NewTool) -> BoxFuture<'_, Result<Arc<dyn DynTool>, String>> {
        Box::pin(async move {
            let spec = ToolSpec {
                name: tool.name,
                description: tool.description,
                args: tool.args,
                code: tool.code,
            };
            let builder = self.builder.clone();
            let (spec, artifact) = tokio::task::spawn_blocking(move || {
                builder.prepare()?;
                let artifact = builder.build(&spec)?;
                Ok::<_, grauwolf_toolbuild::BuildError>((spec, artifact))
            })
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
            let descriptor = ToolDescriptor {
                name: spec.name.clone(),
                description: spec.description.clone(),
                parameters: spec.args.clone(),
            };
            let instance = self
                .runtime
                .instantiate(&artifact, descriptor)
                .map_err(|error| error.to_string())?;
            if let Some(dir) = &self.save_dir {
                save(dir, &spec)?;
            }
            Ok(instance)
        })
    }
}
