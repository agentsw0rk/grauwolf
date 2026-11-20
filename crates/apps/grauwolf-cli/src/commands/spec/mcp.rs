//! `grauwolf spec mcp`: the agent as an MCP tool over stdio (ADR-0056, ADR-0089).

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use grauwolf_host::otel::OtelTracing;

use super::handles::agent_description;
use super::loader::agent_loader;
use crate::options::{RunOptions, Usage};
use crate::output::{Io, errln, report};

/// `grauwolf spec mcp <file>`: the agent as an MCP tool over stdio (ADR-0056).
pub fn run(io: &Io, file: &Path, options: &RunOptions) -> ExitCode {
    if let Err(error) = options.check_effect(Usage::Mcp) {
        return report(io, Err(error));
    }
    if !options.confirm_tools.is_empty() {
        return report(
            io,
            Err(
                "--confirm-tools reads stdin and does not fit the stdio transport of MCP"
                    .to_owned(),
            ),
        );
    }
    let tracing = match options
        .otel
        .then(|| {
            OtelTracing::init_with(
                None,
                &grauwolf_host::config::TelemetryConfig::from_env(
                    &grauwolf_host::config::ProcessEnv,
                ),
            )
        })
        .transpose()
    {
        Ok(tracing) => tracing,
        Err(error) => return report(io, Err(error)),
    };
    let result = (|| {
        let runtime = crate::errors::runtime()?;
        runtime.block_on(async {
            let (spec, agent, provisioned) = agent_loader(io, options, tracing.as_ref())?
                .load_provisioned(file, None)
                .await?;
            // Tools directory and shared memory as with `spec run` (ADR-0089).
            let handles = grauwolf_server::ServiceOptions::provisioned(provisioned);
            let extras: grauwolf_interpreter::ExtrasProvider = Arc::new(move || {
                let handles = handles.clone();
                Box::pin(async move { handles.extras_for(None).await })
            });
            let tool = Arc::new(agent).into_tool_with(&spec.name, agent_description(&spec), extras);
            errln!(io, "grauwolf MCP server for {} on stdio", spec.name);
            let base_dir = file.parent().unwrap_or(Path::new("."));
            grauwolf_host::mcp::agent_server(&spec, base_dir, vec![tool])
                .await?
                .serve(tokio::io::stdin(), tokio::io::stdout())
                .await
                .map_err(|error| error.to_string())
        })
    })();
    let flushed = tracing.map_or(Ok(()), OtelTracing::shutdown);
    report(io, result.and(flushed))
}
