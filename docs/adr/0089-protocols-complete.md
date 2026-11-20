# ADR-0089: A2A and MCP like `/run` – runtime tools, resources, prompts, streaming

- **Status:** Accepted (User, 2026-11-09: "do protocols and service: 7. A2A streaming in the client, 8. MCP server with resources and prompts, 9. user tools and tools directory also via A2A and MCP")
- **Date:** 2026-11-09

## Context
- `/run` gives a run the tools of the user (ADR-0059), the tools directory (ADR-0074) and the automatic memory (ADR-0069). A2A (ADR-0054) and MCP (ADR-0056) run without these handles; `spec mcp` over stdio likewise.
- The MCP server offers only tools, no resources and no prompts.
- The A2A client sends `SendMessage` and waits for the end of the entire run; long tasks fail on the read timeout (ADR-0065), and nobody sees intermediate results.

## Decision
- **Same handles everywhere:** `ServiceOptions::extras_for(user)` provides the user tools, runtime tools and memory of a run; `/run`, A2A (`a2a::router_with(.., options)`, user = owner of the task) and MCP over HTTP (`mcp_router_with(agent, name, description, options)`, user from the token) use them. `spec mcp` passes the tools directory and the shared memory. To this end, an agent used as a tool can receive handles per call: `InterpretedAgent::into_tool_with(name, description, extras)`.
- **MCP resources and prompts:** `McpServer::with_resources(..)` (`resources/list`, `resources/read`; text resources with URI, name, description, MIME type) and `with_prompts(..)` (`prompts/list`, `prompts/get`; templates with arguments `{name}`). For an agent, the CLI offers: the spec itself (`grauwolf://<name>/spec`, JSON) and the documents from `knowledge.sources` (`grauwolf://<name>/knowledge/<path>`), plus a prompt `<name>` with argument `request`, which supplies the agent's system prompt and the task as user message.
- **A2A streaming in the client:** `A2aClient::send_streaming(text, on_text)` uses `SendStreamingMessage` and reports each text piece; the read timeout then applies between two events instead of for the entire run. The tool of a remote agent streams if the agent card reports `capabilities.streaming`; otherwise it stays with `SendMessage`.

## Consequences
- A user sees the same tools and the same memory via A2A and MCP as via `/run`.
- MCP applications can read the spec and the knowledge documents of an agent and use its prompt.
- Long tasks for remote grauwolf agents no longer need a raised read timeout.
