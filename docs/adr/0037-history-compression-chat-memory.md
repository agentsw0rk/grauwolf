# ADR-0037: History compression as a node, chat memory via a store port

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
Long tool loops and conversations across several runs exceed the context window (F05.14, F07.1, F07.2). The obvious approach is a node that shortens the history and passes its input value through unchanged, plus a store for chat histories keyed by conversation ID.

## Decision
**Compression**
- `AgentContext::compress_history(HistoryCompression)` with the strategies `Summarize` (the LLM summarizes the history; the summary replaces it) and `KeepLast(n)` (only the last n messages remain).
- System messages at the start always remain. If the history ends with a response with tool calls, that response remains so that the following tool result is valid; `KeepLast` never starts with an orphaned tool result.
- Node kind `compress_history` with `params { value, strategy, keep }`: signature `value → value` (the type is given by name, e.g. `tool_result`); condition `on_history_longer` with `params { messages }`: passes the value through if the history has more messages. Both are in the catalogs; the generator implements them via generic functions with const parameters.

**Chat memory**
- Port `ChatHistoryStore` (load and save the messages of a conversation ID) in `grauwolf-agents`, `MemoryChatHistoryStore` there, `FileChatHistoryStore` in the facade.
- The interpreter continues conversations: loaded messages follow the system prompt; after the run, the history is saved without the system prompt. CLI: `spec run --conversation <id>`.

## Consequences
- Compression is visible in the graph and therefore controllable in specs, including by the builder agent.
- `Summarize` costs an additional LLM call; `KeepLast` loses information.
