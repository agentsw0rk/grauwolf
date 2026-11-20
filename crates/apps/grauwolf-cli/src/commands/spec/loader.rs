//! Loaders configured from the command line options: tool builds, events, tracing, prices,
//! budgets, answer cache and approval prompts.

use std::sync::Arc;

use grauwolf_agents::events::{Interceptor, ToolDecision};
use grauwolf_host::features::JsonlEventLog;
use grauwolf_host::loader::AgentLoader;
use grauwolf_host::otel::OtelTracing;
use grauwolf_prompt::ToolCall;
use grauwolf_spec::AgentSpec;

use crate::options::RunOptions;
use crate::output::{Io, errln};

/// Loader with the options of the command line: tool builds, events, tracing, approval prompts.
pub fn agent_loader(
    io: &Io,
    options: &RunOptions,
    tracing: Option<&OtelTracing>,
) -> Result<AgentLoader, String> {
    let loader = observed_loader(io, options, tracing, None)?;
    if options.confirm_tools.is_empty() {
        return Ok(loader);
    }
    Ok(loader.with_interceptor(Arc::new(ConfirmTools(options.confirm_tools.clone()))))
}

/// Loader for `spec serve`: `--confirm-tools` makes the tools require approval (ADR-0061).
pub fn service_loader(
    io: &Io,
    options: &RunOptions,
    tracing: Option<&OtelTracing>,
    observations: Option<&Arc<grauwolf_server::observations::Observations>>,
) -> Result<AgentLoader, String> {
    Ok(observed_loader(io, options, tracing, observations)?
        .with_tool_approval(options.confirm_tools.clone()))
}

/// Loader with tool builds, `--events`, `--otel`, `--prices` and the observations of the chat API.
pub fn observed_loader(
    io: &Io,
    options: &RunOptions,
    tracing: Option<&OtelTracing>,
    observations: Option<&Arc<grauwolf_server::observations::Observations>>,
) -> Result<AgentLoader, String> {
    // The process environment enters here, at the outermost entry point (ADR-0114).
    let skipped = io.clone();
    let mut loader = AgentLoader::new(options.build.tool_builder())
        .with_locked(options.build.locked)
        .with_env(Arc::new(grauwolf_host::config::ProcessEnv))
        .with_skipped_tool_report(Arc::new(move |skipped_tool: &str| {
            errln!(skipped, "tool folder: {skipped_tool}");
        }));
    if let Some(dir) = &options.llm_cache {
        loader = loader.with_llm_cache(Arc::new(grauwolf_host::cache::FilePromptCache::new(dir)));
    }
    if let Some(path) = &options.prices {
        let prices = grauwolf_host::prices::load(path)?;
        loader = loader.with_prices(Arc::new(prices));
    }
    if options.max_cost.is_some() && options.prices.is_none() {
        return Err("--max-cost needs --prices: without a price table the cost is unknown".into());
    }
    if options.max_tokens.is_some() || options.max_cost.is_some() {
        loader = loader.with_budget(grauwolf_agents::Budget {
            max_tokens: options.max_tokens,
            max_cost: options.max_cost,
            max_tool_calls: None,
            max_prompt_tokens: None,
        });
    }
    if let Some(observations) = observations {
        loader = loader.with_mcp_tools_observer(Some(observations.mcp_tools_observer()));
    }
    if observations.is_some() {
        // Chat API mode (ADR-0094): `OLLAMA_BASE_URL` stands in for a missing `OLLAMA_HOST`.
        loader = loader.with_chat_api_mode(true);
    }
    if options.events {
        loader = loader.with_feature(Arc::new(JsonlEventLog::new(io.err_writer())));
    }
    if tracing.is_some() || observations.is_some() {
        // Each agent and sub-agent reports its own `invoke_agent` span (ADR-0049) and its own
        // models (ADR-0094).
        let (tracing, content) = (tracing.cloned(), options.otel_content);
        let observations = observations.cloned();
        loader = loader.with_agent_features(Arc::new(move |spec: &AgentSpec| {
            let mut features = Vec::new();
            if let Some(tracing) = &tracing {
                features.push(tracing.feature(&spec.name, content));
            }
            if let Some(observations) = &observations {
                features.push(observations.feature_for(&spec.name));
            }
            features
        }));
    }
    Ok(loader)
}

/// Asks on stderr before calls of the named tools and reads `j`/`ja` from stdin;
/// anything else refuses the call (ADR-0047).
pub struct ConfirmTools(Vec<String>);

impl Interceptor for ConfirmTools {
    fn before_tool_call(&self, call: &mut ToolCall) -> ToolDecision {
        if !self.0.contains(&call.tool) {
            return ToolDecision::Proceed;
        }
        eprint!("Run tool {} with {}? [y/N] ", call.tool, call.args);
        let mut answer = String::new();
        let confirmed = std::io::stdin().read_line(&mut answer).is_ok()
            && matches!(
                answer.trim().to_lowercase().as_str(),
                "j" | "ja" | "y" | "yes"
            );
        if confirmed {
            ToolDecision::Proceed
        } else {
            ToolDecision::Deny(format!("The user refused the call of {}.", call.tool))
        }
    }
}
