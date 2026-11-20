# ADR-0055: A2A client – remote agents as sub-agents

- **Status:** Accepted (User, 2026-11-08: "yes, do that")
- **Date:** 2026-11-08

## Context
With `agent_tools` (ADR-0040) an agent calls other grauwolf specs as tools; since ADR-0054 a grauwolf agent can itself be reachable via A2A. Agents of other frameworks that speak A2A cannot be given tasks so far.

## Decision
- New adapter crate `grauwolf-a2a` with `A2aClient::connect(url, headers)`: loads the agent card (`url` is the card itself or the base URL, then `/.well-known/agent-card.json`), selects the interface with `protocolBinding: "JSONRPC"` and sends messages with `SendMessage` (header `A2A-Version: 1.0`).
- Answers: text and `data` parts of the artifacts of a completed task or of a direct message; `TASK_STATE_INPUT_REQUIRED` returns the agent's follow-up question as the result (the LLM can respond to it); `FAILED`/`REJECTED`/`CANCELED` become a tool error.
- `A2aClient::tool(name, description)` is a `DynTool` with the argument `{ request }`. Every call is a separate message without a remembered `contextId` – like local sub-agents with a fresh history; this way conversations of different users do not mix within a service.
- Specs: an entry in `agent_tools` has **either** `spec` **or** `url` (+ optional `headers` with `${VAR}` from the environment, as for MCP); `description` is optional with `url` (otherwise taken from the agent card) and required with `spec`.

## Consequences
- grauwolf agents can give tasks to agents of other frameworks and vice versa (ADR-0054).
- Streaming and multi-turn conversations with remote agents are left out.
