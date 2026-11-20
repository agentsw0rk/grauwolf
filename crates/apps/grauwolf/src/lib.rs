//! grauwolf – facade: re-exports the sub-crates and the composition root `grauwolf-host` under the
//! same module paths (ADR-0108, stage 4a), and `chat_api`, `observations` and `service` of `grauwolf-server` (stage 4c).
//! Cargo features (all on by default): `macros`, `ollama`, `openai`, `anthropic`, `gemini`, `bedrock`,
//! `sandbox`, `packages` (tool packages from OCI registries, ADR-0107; implies `sandbox`), `mcp`, `builtin` (light built-in tools: files, key-value, user I/O, think, to-do), `git` (git tools), `fetch` (the `fetch` tool), `atlassian` (Jira and Confluence tools; ADR-0108), `sql`, `pgvector` (PostgreSQL with pgvector for knowledge and long-term memory, ADR-0106), `otel`, `a2a`, `systemone`; `service` (generated crates as HTTP services; implies `mcp`); `loader` needs `mcp`, `builtin`, `sandbox` and `a2a` (tool packages in it need `packages`).

pub use grauwolf_agents as agents;
pub use grauwolf_builder_agent as builder_agent;
pub use grauwolf_graph as graph;
pub use grauwolf_interpreter as interpreter;
pub use grauwolf_llm as llm;
/// Tools from functions (`#[grauwolf::tool]`) and from `impl` blocks (`#[grauwolf::toolset]`);
/// the generated code only needs this crate (ADR-0003 addendum).
#[cfg(feature = "macros")]
pub use grauwolf_macros::{facade_tool as tool, facade_toolset as toolset};
pub use grauwolf_prompt as prompt;
pub use grauwolf_rag as rag;
pub use grauwolf_spec as spec;
pub use grauwolf_toolbuild as toolbuild;
pub use grauwolf_tools as tools;

#[cfg(feature = "anthropic")]
pub use grauwolf_anthropic as anthropic;
#[cfg(feature = "bedrock")]
pub use grauwolf_bedrock as bedrock;
#[cfg(feature = "gemini")]
pub use grauwolf_gemini as gemini;
#[cfg(feature = "ollama")]
pub use grauwolf_ollama as ollama;
#[cfg(feature = "openai")]
pub use grauwolf_openai as openai;
#[cfg(feature = "sandbox")]
pub use grauwolf_sandbox as sandbox;
#[cfg(feature = "packages")]
pub use grauwolf_toolpkg as toolpkg;

#[cfg(feature = "builtin")]
pub use grauwolf_host::builtin;
#[cfg(all(
    feature = "mcp",
    feature = "builtin",
    feature = "sandbox",
    feature = "a2a"
))]
pub use grauwolf_host::loader;
#[cfg(feature = "mcp")]
pub use grauwolf_host::mcp;
#[cfg(feature = "otel")]
pub use grauwolf_host::otel;
#[cfg(feature = "packages")]
pub use grauwolf_host::packages;
#[cfg(feature = "pgvector")]
pub use grauwolf_host::pgvector;
#[cfg(feature = "systemone")]
pub use grauwolf_host::systemone;
pub use grauwolf_host::{app, attachments, builder, cache, checkpoints, config, env, features};
pub use grauwolf_host::{memory, providers, provisioning, runtime, spec_schema};
#[cfg(feature = "sandbox")]
pub use grauwolf_host::{self_tools, tools_dir};

#[cfg(feature = "service")]
pub use grauwolf_server::service;
pub use grauwolf_server::{chat_api, observations};

/// Everyday imports in one line: `use grauwolf::prelude::*;` – the spec with its extension trait
/// `AgentSpecExt` (`from_json`, `validate`, `warnings`; ADR-0108 stage 6), the interpreter, the loader,
/// providers, configuration, prompts, tools and the tool macros.
pub mod prelude {
    pub use grauwolf_agents::{AgentContext, Budget};
    pub use grauwolf_host::config::{Config, Env, MapEnv, ProcessEnv};
    #[cfg(all(
        feature = "mcp",
        feature = "builtin",
        feature = "sandbox",
        feature = "a2a"
    ))]
    pub use grauwolf_host::loader::{AgentLoader, load_spec};
    pub use grauwolf_host::providers::executor_from_env;
    pub use grauwolf_interpreter::Interpreter;
    pub use grauwolf_llm::PromptExecutor;
    pub use grauwolf_prompt::{LlModel, Message, Prices, Prompt, ToolDescriptor, Usage};
    pub use grauwolf_spec::{AgentSpec, AgentSpecExt};
    pub use grauwolf_tools::{DynTool, ToolError, ToolRegistry};

    #[cfg(feature = "macros")]
    pub use crate::{tool, toolset};
}
