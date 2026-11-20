# E09 – Protocols: MCP, A2A, ACP


## Model Context Protocol (MCP)

| ID | Feature | Rust approach | Prio |
|----|---------|-------------|------|
| F09.1 | MCP client: register the tools of an MCP server as grauwolf tools | Official Rust SDK `rmcp`; adapter MCP tool → `DynTool` | M2 |
| F09.2 | Transports stdio (start process) & SSE/Streamable HTTP | `rmcp` transports | M2 |
| F09.3 | MCP metadata (server info, attributes for OTel) | Struct + OTel attributes | M2 |
| F09.4 | MCP server: offer own tools as an MCP server | `rmcp` server with `ToolRegistry` bridge | M3 |

## Agent2Agent (A2A)

| ID | Feature | Rust approach | Prio |
|----|---------|-------------|------|
| F09.5 | A2A data model (AgentCard, Task, Message, Parts, Artifacts, Status) | serde structs following the A2A spec | M3 |
| F09.6 | JSON-RPC transport (HTTP, SSE streaming) | Server: axum; client: reqwest + `eventsource-stream` | M3 |
| F09.7 | A2A server (task store, push notifications, agent executor) | `trait AgentExecutor`, `TaskStore` (in-memory) | M3 |
| F09.8 | A2A client | `A2aClient` | M3 |
| F09.9 | Agent integration: agent as A2A server / A2A nodes in the graph | Feature + nodes | M3 |
| F09.10 | Conformance tests (TCK) | Run the TCK against our server | M3 |

## Agent Client Protocol (ACP)

| ID | Feature | Rust approach | Prio |
|----|---------|-------------|------|
| F09.11 | ACP-compatible agent (communication with editors/clients over stdio JSON-RPC) | Crate `agent-client-protocol` (Rust reference implementation by Zed) + message converter | M3 |
