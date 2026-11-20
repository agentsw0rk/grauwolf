//! Shared helpers of the tests.
#![allow(dead_code)]

use std::sync::Arc;

use grauwolf_atlassian::AtlassianClient;
use grauwolf_tools::DynTool;
use wiremock::MockServer;

/// Basic auth of `agent@example.com:token`.
pub const AUTH: &str = "Basic YWdlbnRAZXhhbXBsZS5jb206dG9rZW4=";

pub fn client(server: &MockServer) -> AtlassianClient {
    AtlassianClient::new(&server.uri(), "agent@example.com", "token").unwrap()
}

pub fn tool(tools: &[Arc<dyn DynTool>], name: &str) -> Arc<dyn DynTool> {
    tools
        .iter()
        .find(|tool| tool.name() == name)
        .cloned()
        .unwrap()
}
