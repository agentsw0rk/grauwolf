# ADR-0051: MCP over Streamable HTTP

- **Status:** Accepted (decision by Claude under "decide on your own", 2026-11-08)
- **Date:** 2026-11-08

## Context
The MCP client (ADR-0036) speaks only stdio: it starts servers as local processes. Many MCP servers, however, run as a service on the network and speak the "Streamable HTTP" transport (MCP 2025-06-18): every JSON-RPC message goes to an endpoint via `POST`, the response comes as JSON or as Server-Sent Events; a session is kept through the header `Mcp-Session-Id`.

## Decision
- `McpClient` separates protocol and transport: `Transport::Stdio` (as before) and `Transport::Http`. `McpClient::connect_http(url, headers)` sends every request via `POST` with `Accept: application/json, text/event-stream`, takes `Mcp-Session-Id` from the response to `initialize` and from then on sends `Mcp-Session-Id` and `MCP-Protocol-Version`. Responses as `application/json` or `text/event-stream` (the message with the matching ID wins, others are skipped); notifications expect `202`.
- HTTP via `reqwest` (like the LLM adapters).
- Specs: an entry in `mcp_servers` has **either** `command` (+ `args`) **or** `url` (+ optional `headers`); both or neither is a validation error. Values in `headers` may contain `${VAR}`, which is replaced from the environment at startup (credentials do not belong in the spec).

## Consequences
- Remote MCP servers can be used without local processes and can be fully tested against a fake server.
- The older HTTP+SSE transport (separate GET/POST endpoints) is not part of this decision; it is added by [ADR-0112](0112-mcp-over-http-sse.md) as an explicit `transport: "sse"`.
