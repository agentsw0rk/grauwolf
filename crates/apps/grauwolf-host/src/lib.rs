//! grauwolf-host – the composition root: assembles a runnable agent from a spec (loader, providers,
//! tool provisioning, memory, knowledge, packages, MCP, telemetry). The facade `grauwolf` re-exports
//! every module under the same path (ADR-0108, stage 4a).
//! Cargo features: `ollama`, `openai`, `anthropic`, `gemini`, `bedrock`, `sandbox`, `packages` (implies
//! `sandbox`), `mcp`, `builtin`, `git`, `fetch`, `atlassian`, `sql`, `pgvector`, `otel`, `a2a`, `systemone`;
//! `loader` needs `mcp`, `builtin`, `sandbox` and `a2a` (tool packages in it need `packages`).

#[cfg(all(
    feature = "mcp",
    feature = "builtin",
    feature = "sandbox",
    feature = "a2a"
))]
pub mod agent_tree;
pub mod app;
pub mod attachments;
pub mod builder;
#[cfg(feature = "builtin")]
pub mod builtin;
pub mod cache;

pub mod checkpoints;
pub mod config;
pub mod credentials;
pub mod env;
pub mod features;
pub mod identity;
pub(crate) mod knowledge;
#[cfg(all(
    feature = "mcp",
    feature = "builtin",
    feature = "sandbox",
    feature = "a2a"
))]
pub mod loader;
pub(crate) mod long_term_memory;
#[cfg(feature = "mcp")]
pub mod mcp;
pub mod mcp_observer;
pub mod memory;
#[cfg(feature = "otel")]
pub mod otel;
#[cfg(feature = "packages")]
pub mod packages;
#[cfg(feature = "pgvector")]
pub mod pgvector;
pub mod prices;
pub mod project;
pub mod providers;
pub mod provisioning;
pub mod runtime;
#[cfg(feature = "sandbox")]
pub mod self_tools;
#[cfg(feature = "builtin")]
pub mod skills;
pub mod spiffe;
#[cfg(feature = "systemone")]
pub mod systemone;
mod tool_files;
pub mod urls;
pub use tool_files::spec_schema;
#[cfg(feature = "sandbox")]
pub mod tools_dir;

/// The tests that need crate-private building blocks (the stores behind provisioning).
#[cfg(test)]
mod tests {
    #[cfg(feature = "fetch")]
    mod knowledge;
    mod long_term_memory;
    #[cfg(all(feature = "pgvector", feature = "fetch"))]
    mod pgvector_stores;
    #[cfg(feature = "systemone")]
    mod systemone;
}
