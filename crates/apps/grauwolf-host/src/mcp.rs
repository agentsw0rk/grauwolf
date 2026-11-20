//! Starts the MCP servers of a spec and provides their tools (ADR-0036).

use std::sync::Arc;

pub use grauwolf_mcp::{McpClient, McpPrompt, McpPromptArgument, McpResource, McpServer};
use grauwolf_spec::{AgentSpec, McpHttpTransport, McpServerSpec};
use grauwolf_tools::DynTool;

use crate::config::Env;
use crate::mcp_observer::McpToolsObserver;

/// Starts all MCP servers from `spec.mcp_servers` and returns their declared tools; the servers
/// run as long as the tools live. `${VAR}` in the headers comes from `env`, `observer` (if any)
/// sees the tool list of every server.
pub(crate) async fn start_servers_with(
    spec: &AgentSpec,
    env: &dyn Env,
    observer: Option<&McpToolsObserver>,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    let mut tools = Vec::new();
    for (name, server) in &spec.mcp_servers {
        let client = connect(name, server, env)
            .await
            .map(Arc::new)
            .map_err(|error| format!("MCP server '{name}': {error}"))?;
        let offered = client
            .tools()
            .await
            .map_err(|error| format!("MCP server '{name}': {error}"))?;
        let descriptors: Vec<_> = offered.iter().map(|tool| tool.descriptor()).collect();
        if let Some(observer) = observer {
            observer(name, &descriptors);
        }
        for declared in &server.tools {
            let tool = offered
                .iter()
                .find(|tool| tool.name() == declared)
                .ok_or_else(|| {
                    let names: Vec<&str> = offered.iter().map(|tool| tool.name()).collect();
                    format!(
                        "MCP server '{name}' does not offer tool '{declared}' (available: {})",
                        names.join(", ")
                    )
                })?;
            tools.push(Arc::clone(tool));
        }
        if server.resources {
            tools.extend(client.resource_tools(name));
        }
    }
    Ok(tools)
}

/// The tools the server `name` offers, all of them, whatever the spec declares (grauwolf studio's
/// discovery, ADR-0126 decision 13): it connects like a run does (stdio, Streamable HTTP or SSE,
/// `${VAR}` in the headers from `env`, workload identity) and closes again.
pub async fn list_tools(
    name: &str,
    server: &McpServerSpec,
    env: &dyn Env,
) -> Result<Vec<grauwolf_prompt::ToolDescriptor>, String> {
    let client = connect(name, server, env)
        .await
        .map_err(|error| format!("MCP server '{name}': {error}"))?;
    client
        .list_tools()
        .await
        .map_err(|error| format!("MCP server '{name}': {error}"))
}

/// Connection via stdio (`command`), Streamable HTTP (`url`, ADR-0051) or, with
/// `transport: "sse"`, the older HTTP+SSE transport (ADR-0112).
async fn connect(name: &str, server: &McpServerSpec, env: &dyn Env) -> Result<McpClient, String> {
    match (&server.command, &server.url) {
        (Some(command), _) => {
            // The child gets the agent's identity only on request (ADR-0110).
            let vars = if server.workload_api {
                crate::spiffe::workload_env(env, &format!("MCP server '{name}'"))?
            } else {
                Vec::new()
            };
            McpClient::spawn_with_env(command, &server.args, &vars)
                .await
                .map_err(|error| format!("{command}: {error}"))
        }
        (None, Some(url)) => {
            let headers = server
                .headers
                .iter()
                .map(|(name, value)| Ok((name.clone(), crate::env::expand_with(value, env)?)))
                .collect::<Result<Vec<_>, String>>()?;
            let auth = match &server.auth {
                Some(auth) => Some(
                    crate::spiffe::client_auth(env, &format!("MCP server '{name}'"), auth).await?,
                ),
                None => None,
            };
            let timeout = grauwolf_mcp::DEFAULT_READ_TIMEOUT;
            match server.http_transport() {
                McpHttpTransport::StreamableHttp => {
                    McpClient::connect_http_with_auth(url, &headers, timeout, auth).await
                }
                McpHttpTransport::Sse => {
                    McpClient::connect_sse_with_auth(url, &headers, timeout, auth).await
                }
            }
            .map_err(|error| format!("{url}: {error}"))
        }
        (None, None) => Err("neither `command` nor `url` given".into()),
    }
}

/// MCP server for an agent (ADR-0089): `tools` (usually the agent itself), as resources
/// the spec (`grauwolf://<name>/spec`) and the documents from `knowledge.sources` relative to
/// `base_dir` (`grauwolf://<name>/knowledge/<path>`, UTF-8 text only), as prompt `<name>` with
/// argument `request` the system prompt of the agent together with the request.
pub async fn agent_server(
    spec: &AgentSpec,
    base_dir: &std::path::Path,
    tools: Vec<Arc<dyn DynTool>>,
) -> Result<McpServer, String> {
    let name = &spec.name;
    let mut resources = vec![McpResource {
        uri: format!("grauwolf://{name}/spec"),
        name: "spec".into(),
        description: format!("The spec of the agent {name}"),
        mime_type: "application/json".into(),
        text: spec.to_json(),
    }];
    for source in spec
        .knowledge
        .iter()
        .flat_map(|knowledge| &knowledge.sources)
    {
        for file in crate::knowledge::files(&base_dir.join(source)).await? {
            // Binary files are not part of the knowledge (ADR-0038); they are missing here too.
            let Ok(text) = tokio::fs::read_to_string(&file).await else {
                continue;
            };
            let relative = file
                .strip_prefix(base_dir)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            let markdown = file.extension().is_some_and(|extension| extension == "md");
            resources.push(McpResource {
                uri: format!("grauwolf://{name}/knowledge/{relative}"),
                name: relative.clone(),
                description: format!("Knowledge document {relative}"),
                mime_type: if markdown {
                    "text/markdown"
                } else {
                    "text/plain"
                }
                .into(),
                text,
            });
        }
    }
    let template = match &spec.system_prompt {
        Some(system) => format!("{system}\n\n{{request}}"),
        None => "{request}".to_owned(),
    };
    let prompt = McpPrompt {
        name: name.clone(),
        description: format!("Request in the style of the agent {name} (its system prompt)"),
        arguments: vec![McpPromptArgument {
            name: "request".into(),
            description: "Request".into(),
            required: true,
        }],
        template,
    };
    Ok(McpServer::new(name.clone(), tools)
        .with_resources(resources)
        .with_prompts(vec![prompt]))
}
