# ADR-0079: Facts from history compression into long-term memory

- **Status:** Accepted (User, 2026-11-09: "put facts from conversations into long-term memory")
- **Date:** 2026-11-09

## Context
Compression to facts (ADR-0076) condenses older parts of a conversation; these facts apply only within the conversation. Long-term memory (ADR-0058, ADR-0069) lives across conversations, but receives only what `remember` or `auto_remember` stores after a run. Precisely in long conversations, things get lost that the agent should still know later.

## Decision
- Spec: `long_term_memory.remember_compressed` (default `false`). Like `auto_recall`/`auto_remember`, it counts as use of the memory (also with `tools: false`), and it ensures that CLI and service pass the run's memory on (`RunExtras.memory`).
- `AgentContext::with_fact_memory(memory)`: if a memory is set, the compression additionally asks in the same structured request with `facts` for `durable`, the subset of lasting facts about the user (name, preferences, plans, relationships; no open questions, no daily events). `durable` goes into the memory via `FactMemory::remember` (the same text per scope only once), reported as `memory_remembered`; errors report `memory_failed`, and the compression and the run continue. Forks share the memory.
- The interpreter sets the memory from `RunExtras.memory` when the spec requires `remember_compressed`; the same scope applies as for automatic memory (the user from `--user` or the service).
- Generator: `remember_compressed` → `Unsupported`, like `auto_recall`/`auto_remember`.

## Consequences
- No additional LLM request: the distinction between lasting and temporary is made by the model in the compression request.
- Takes effect only where compression happens (`compress_history` with `facts`, strategy `chat`).
