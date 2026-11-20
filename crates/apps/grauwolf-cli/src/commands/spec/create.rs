//! `grauwolf spec create <requirement>`: a valid spec from a requirement, with the builder agent
//! (F13.12).

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use grauwolf_builder_agent::Builder;
use grauwolf_host::builder::ToolBuildChecker;
use grauwolf_host::providers::executor_from_env;
use grauwolf_prompt::LlModel;

use crate::options::BuildOptions;
use crate::output::{Io, outln, report};

/// `grauwolf spec create <requirement> --model <provider>:<model> --out <file>`.
pub fn run(
    io: &Io,
    requirement: &str,
    model: &str,
    out: &Path,
    max_repairs: u32,
    options: &BuildOptions,
) -> ExitCode {
    report(
        io,
        (|| {
            let (provider, id) = model.split_once(':').ok_or_else(|| {
                format!("--model expects <provider>:<model>, e.g. ollama:qwen3.5:9b, got: {model}")
            })?;
            let executor = executor_from_env(provider).map_err(|error| error.to_string())?;
            // The generated agent runs with the model it was generated with.
            let builder = Builder::new(Arc::new(executor), LlModel::new(provider, id))
                .with_agent_model(LlModel::new(provider, id))
                .with_max_repairs(max_repairs)
                .with_checker(Arc::new(ToolBuildChecker::from_builder(
                    options.tool_builder(),
                )));
            let runtime = crate::errors::runtime()?;
            let built = runtime
                .block_on(builder.build(requirement))
                .map_err(|error| error.to_string())?;
            if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("{}: {error}", parent.display()))?;
            }
            std::fs::write(out, format!("{}\n", built.json))
                .map_err(|error| format!("{}: {error}", out.display()))?;
            let repairs = match built.repairs {
                1 => "1 repair".to_owned(),
                count => format!("{count} repairs"),
            };
            outln!(
                io,
                "{}: spec '{}' created ({repairs})",
                out.display(),
                built.spec.name
            );
            Ok(())
        })(),
    )
}
