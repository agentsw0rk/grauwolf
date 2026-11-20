# ADR-0040: Agent as tool – specs call other specs as tools

- **Status:** Accepted (User, 2026-11-07: "yes, do that" on the order of the agent features)
- **Date:** 2026-11-07

## Context
An agent should be able to serve as a tool for another agent (F03.12): a coordinator delegates subtasks to specialized agents that work with their own history. In grauwolf, each spec describes exactly one agent; collaboration of several agents has been missing so far.

## Decision
- `InterpretedAgent::into_tool(name, description)` turns a loaded agent into a `DynTool`. Each call runs like `run_json` in a fresh context (own history, same executor and same features).
- Arguments: for an input type `string`, an object `{ "request": "<text>" }`; for a named type, its schema (the arguments are the input). Result: text unchanged, structured outputs as JSON text. Errors of the sub-agent become a tool error that the LLM sees.
- In specs, `agent_tools` names named sub-agents: `{ "<name>": { "spec": "<path relative to the spec>", "description": "…" } }`. The names share the tool namespace. The interpreter requires them like MCP and search tools as external tools; the CLI loads the referenced specs (recursively, cycles are an error) along with their tools, MCP servers, and knowledge.
- The generator rejects `agent_tools` for now (interpreter only).

## Consequences
- Multi-agent setups (coordinator and specialists) arise from several small specs that remain individually testable.
- Sub-agents share the executor; each spec determines its own model and system prompt.

## Addendum (2026-11-09): generator and loading of sub-agents
- The generator no longer rejects `agent_tools`; sub-agents are external tools of the generated code (`EXTERNAL_TOOLS`, `run_with_tools`, ADR-0050).
- The recursive loading of sub-agents no longer lives in the CLI but in the facade (`grauwolf::loader::AgentLoader`), which the CLI and applications share.
