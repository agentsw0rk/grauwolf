# ADR-0032: Assistant messages made of parts (text, tool calls, reasoning)

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
So far a message carried either text (`Message::Assistant`) or exactly one tool call (`Message::ToolCall`). Two requirements do not fit this:
- Models request several tools in one response at the same time (parallel tool calls); the adapters took over only the first one.
- Anthropic models with thinking blocks expect these back unchanged in the history; whoever omits them alters the history afterwards, which current models may reject (`400`). There was no place for such provider-specific data.

It is common to model an assistant message as a list of parts (text, tool call, reasoning).

## Decision
- `Message::Assistant(AssistantMessage { parts: Vec<AssistantPart> })` replaces `Message::Assistant { content }` and `Message::ToolCall`. One LLM response is exactly one assistant message.
- `AssistantPart` is `Text { text }`, `ToolCall(ToolCall)`, or `Reasoning(Reasoning)`.
- `Reasoning { provider, data }` is opaque to the core (`data` as JSON). Only the adapter of the provider `provider` sends it back; other adapters omit it. It remains serializable (checkpoints, ADR-0010).
- Tool results remain individual `Message::ToolResult`s; adapters combine consecutive results the way their provider requires.
- The constructors `Message::assistant(text)` and `Message::tool_call(id, tool, args)` remain and create messages with one part.
- Conditions operate on the assistant message: `on_assistant_message` matches a response without tool calls (yielding its text), `on_tool_call` matches exactly one tool call; separate conditions and nodes for multiple calls follow (F05.9).

## Consequences
- Parallel tool calls and reasoning can be mapped without further changes to the core.
- All adapters, conditions, the testkit, and structured responses must be switched to parts (a one-time rework).
- The serialization format of `Message` changes; there is no stored data yet that would need to be migrated.
