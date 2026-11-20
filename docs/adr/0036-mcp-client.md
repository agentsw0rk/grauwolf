# ADR-0036: Own MCP client, MCP tools as grauwolf tools

- **Status:** Accepted (User, 2026-11-06); deviates from F09.1 (`rmcp`)
- **Date:** 2026-11-06

## Context
Via the Model Context Protocol (MCP), many ready-made servers offer tools (file system, Git, databases, web …). Agents should be able to use them like their own tools (F09.1, F09.2). The feature catalog names the official Rust SDK `rmcp`. For the client side, however, grauwolf needs only a small part of the protocol: JSON-RPC 2.0 with `initialize`, `notifications/initialized`, `tools/list`, and `tools/call`.

## Decision
- New adapter crate `grauwolf-mcp` with its own client, analogous to the LLM providers (ADR-0005): no external crate for the protocol, dependencies only tokio and serde_json.
- The client works on arbitrary `AsyncRead`/`AsyncWrite` (line-by-line JSON-RPC as in the stdio transport); `McpClient::spawn(command, args)` starts a server process and ends it on drop.
- Each tool of a server becomes a `DynTool` (name, description, `inputSchema`) and can be registered normally in a `ToolRegistry`; text contents of the result are joined, `isError` becomes `ToolError::Execution`.
- In specs, a section `mcp_servers` declares servers with command, arguments, and the names of the tools used. The names are declared so that validation can check tool references (e.g., in subgraphs) without starting servers.
- Streamable HTTP transport, resources, prompts, and the server side (F09.4) follow as needed.

## Consequences
- A small, testable client without dependency on a quickly changing SDK API.
- grauwolf must follow protocol changes itself; the protocol version is a constant in the client.
- MCP tools run outside the WASM sandbox – with the rights of the server process. This is deliberate (they do not come from LLM-generated code) and is made visible in the spec.

## Addendum (2026-11-09): follow-up steps implemented
Streamable HTTP is implemented with ADR-0051, the server side (F09.4) with ADR-0056. The client can also query resources and prompts (`McpClient::list_resources`, `read_resource`, `list_prompts`, `get_prompt`); in specs they are not integrated, MCP servers still provide only tools there.
