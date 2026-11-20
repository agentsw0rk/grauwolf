# ADR-0059: Long-term memory separated by user

- **Status:** Accepted (User, 2026-11-08: "keep going with the separation by user")
- **Date:** 2026-11-08

## Context
The long-term memory (ADR-0058) is a single area per file. Under `spec serve` (ADR-0053), one agent serves many callers – each would see the memories of all others. The tools themselves do not know the run (`DynTool::call_json` has no context), and the agent is loaded once.

## Decision
- **Store:** Each fact optionally carries a user (`user`). A memory provides, via `tools(user)`, tools for exactly one area: with a user, only that user's facts; without a user, the shared area (as before; old files remain valid). Duplicates count per area.
- **One instance per file:** The same file is opened only once per process (registry keyed by path), so that loaders, the service, and sub-agents do not overwrite each other.
- **Per run:** `RunExtras.tools` replaces same-named tools of the agent for one run. This way each run gets the memory tools of its user without reloading the agent; subgraph scopes continue to filter by name.
- **Interfaces:** `/run` and `/run/stream` optionally accept `user_id`; the CLI takes `spec run --user <id>`. The service trusts the passed ID – this separates data but does not authenticate (the caller ensures that, e.g., via its own backend behind the token).

## Consequences
- A2A and MCP know no user and use the shared area.
- Sub-agents from `agent_tools` have their own memory without a user.
- Conversation histories (`conversation_id`) are still not bound to users; random IDs are the caller's responsibility.

## Addendum (2026-11-09): conversations and A2A
- The consequence "conversation histories are still not bound to users" is superseded by ADR-0060: with `user_id`, a conversation lies under `conversation_key(user, id)` in the user's area.
- A2A now knows the user for conversations and tasks with user tokens (ADR-0068) (addendum in ADR-0054); the memory there continues to use the shared area.
