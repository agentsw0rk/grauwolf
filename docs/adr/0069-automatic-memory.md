# ADR-0069: Automatic recall and storage in long-term memory

- **Status:** Accepted (User, 2026-11-08: "start with automatic memory")
- **Date:** 2026-11-08

## Context
Since ADR-0058, an agent stores facts and retrieves them only if the model calls the tools `remember`/`recall`. Small models do this unreliably, and each retrieval costs an additional round. Needed are automatic storage (F07.6), enriching the prompt (F07.7), and clear error handling (F07.9).

## Decision
- `long_term_memory` gets `auto_recall` and `auto_remember` (both default `false`); the tools remain alongside them.
- `auto_recall`: before the run, the `top_k` facts most similar to the input are retrieved and appended as a system message after the existing history ("Known facts: …"). Appended, not inserted: the history stays append-only, and the prompt cache (ADR-0062) and thinking blocks remain valid. Without hits, no message.
- `auto_remember`: after a successful run, a separate structured request asks the agent's model for lasting facts from input and response (`{ facts: [string] }`, empty allowed) and stores them; duplicates remain one-time (ADR-0058). The agent's history remains untouched.
- Port `grauwolf_agents::memory::FactMemory { recall(query), remember(facts) }`, passed as `RunExtras.memory`; the facade implements it with `LongTermMemory` for one user (or the shared scope). CLI (`spec run`, `spec chat`) and service wire it up when the spec requires it.
- Failure policy: errors when retrieving or storing do not abort the run. Events `memory_recalled { facts }`, `memory_remembered { facts }` and `memory_failed { error }` make both visible.
- The generator initially rejects `auto_recall`/`auto_remember` with `Unsupported`, instead of silently ignoring them in the generated code.

## Addendum (User, 2026-11-08: "yes, do both")
The live test showed that Claude still uses the tools offered in addition (an extra round), and that near-duplicates arise as a result ("The user is named Anna." next to "The user's name is Anna.").
- `long_term_memory.tools` (default `true`): with `false`, no path offers the memory tools: neither `external_tools()`, the facade, `spec run --user`, nor the service per user. Without `auto_recall`/`auto_remember`, this is a validation error (`UnusedMemory`).
- Before the fact request, the known facts for the input are retrieved and passed along as "Already known"; the instruction asks only for new facts, not even rephrased ones. If the retrieval fails, `memory_failed` reports it and the request runs without the known facts.

## Consequences
- Memory works even with models that rarely use tools on their own.
- `auto_remember` costs one additional request per run; `auto_recall` costs one embedding.
- Retrieved facts are part of the conversation history when a conversation is stored.
