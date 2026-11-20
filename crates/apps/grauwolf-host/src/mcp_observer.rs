//! The observer of MCP tool lists (ADR-0094): whoever composes an agent (the chat API of the
//! server, a generated service) hands it explicitly to `ProvisionOptions::with_mcp_tools_observer`,
//! `mcp::start_servers_with`, the `AgentLoader` or `ProvisionOptions`; there is no process-wide state.
//! Whether chat API mode is on is part of the configuration
//! ([`crate::config::ProvidersConfig::chat_api_mode`]).

use std::sync::Arc;

use grauwolf_prompt::ToolDescriptor;

/// Receives the tools an MCP server offers (server name, descriptors).
pub type McpToolsObserver = Arc<dyn Fn(&str, &[ToolDescriptor]) + Send + Sync>;
