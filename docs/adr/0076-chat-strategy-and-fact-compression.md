# ADR-0076: `chat` strategy and history compression with facts

- **Status:** Accepted (User, 2026-11-09: "can we do the chat strategy and history compression with facts")
- **Date:** 2026-11-09

## Context
Long conversations (`spec chat`, `--conversation`, the service) grow with every run until the model's context window overflows. `compress_history` so far knows `summarize` (the entire history becomes one running text) and `keep_last` (older messages are dropped). Summarizing loses details, truncating loses everything older. The feature catalog foresees F05.15 for this (keeping facts during compression). ADR-0064 defined chatting as a loop around ordinary runs; a strategy that carries such conversations permanently is missing.

## Decision
- **Compression with facts:** `HistoryCompression::Facts(keep)`, or `compress_history` with `"strategy": "facts"` and `keep`. The messages before the last `keep` (boundary as with `keep_last`: no orphaned tool result; an open tool call remains) go into a structured request `{ facts: [text] }` to the active model: all facts, results, decisions, the user's wishes and open questions needed for the further conversation, as standalone sentences. They replace the older messages with exactly one user message that begins with `FACTS_HEADER` ("Facts from the conversation so far:") and contains one line `- …` per fact. Such a message from an earlier compression is passed along and is merged into the new result; there is thus always at most one. System messages remain. Usage and budget count as for `summarize`. If nothing is older than `keep`, nothing happens.
- **Strategy `chat`:** `strategy: { kind: "chat", params: { max_messages?, keep?, parallel_tools? } }` (defaults 30, 10, `true`; `keep` < `max_messages`). Graph: `start` → `compress` (`compress_history` with `facts`, edge `on_history_longer { messages: max_messages }`) → `call_llm`, otherwise directly `start` → `call_llm`; afterwards the tool loop of `single_run`. Compression thus happens before each response of a run, not in the middle of the tool loop.
- Generator: node `compress_history_facts::<T, KEEP>`; interpreter and generated code behave the same.

## Consequences
- Conversations can run arbitrarily long; older parts remain as a list of facts, the last messages verbatim.
- Every compression costs one additional LLM request; with a gap of `max_messages` − `keep`, it happens rarely.
- Stored conversations contain the list of facts instead of the old messages after a compression; the compression is final.
- ADR-0064 still applies: the user determines the end of the conversation.
