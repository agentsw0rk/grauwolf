# ADR-0056: MCP server – offering grauwolf agents as MCP tools

- **Status:** Accepted (decision by Claude, User 2026-11-08: "then go on")
- **Date:** 2026-11-08

## Context
grauwolf uses MCP servers as a tool source (ADR-0036, ADR-0051). Conversely, MCP-capable applications (Claude Desktop, IDEs, other agent frameworks) should be able to call grauwolf agents (F09.4).

## Decision
- `grauwolf-mcp` gets `McpServer::new(name, tools)`: offers arbitrary `DynTool`s. `handle(message)` answers `initialize` (the client's protocol version if supported, otherwise `2025-06-18`; `capabilities.tools`, `serverInfo`), `ping`, `tools/list` (`name`, `description`, `inputSchema`), `tools/call` (`content` with text, tool errors as `isError: true`); notifications remain unanswered, unknown methods `-32601`.
  - Transport stdio: `serve(reader, writer)` – line-based JSON-RPC as in the client.
  - Transport Streamable HTTP: `grauwolf-server` offers `POST /mcp` (JSON response, notifications `202`); stateless, without `Mcp-Session-Id`, protected like `/run`.
- What is offered is the agent itself as one tool with the name of the spec (`InterpretedAgent::into_tool`, argument `{ request }` or the schema of the input type), description from the system prompt.
- CLI: `grauwolf spec mcp <file>` starts the stdio server (output only on stderr); `spec serve` includes `/mcp`.

## Consequences
- With one line of configuration, a spec is a tool in Claude Desktop or any other MCP application.
- For now the server offers no resources, prompts or progress notifications.
