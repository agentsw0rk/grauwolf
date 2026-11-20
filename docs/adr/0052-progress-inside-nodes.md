# ADR-0052: Saving progress inside long nodes (`plan`, `parallel`)

- **Status:** Accepted (User, 2026-11-08: "continue according to the list", item 1)
- **Date:** 2026-11-08

## Context
Checkpoints are created before every node (ADR-0020, ADR-0034). `plan` (ADR-0039) and `parallel` (ADR-0045), however, are single nodes that contain many LLM and tool calls; after an abort, all of their work starts over.

## Decision
- `AgentContext::save_progress()` writes the current context (including agent storage) as the new state **of the current checkpoint**: same node path, same input. For this, the interpreter connects the context to the checkpoint observer (`AgentObserver::progress_saver`); without checkpoints `save_progress` does nothing. Forks of parallel branches get no save hook.
- Long nodes keep their progress in agent storage and read it at startup:
  - `plan`: after every step, state, plan and iteration under `grauwolf.plan.<node-id>`; on resume it continues with the next step; at the end the entry is removed.
  - `parallel`: after every finished branch, its result and context under `grauwolf.parallel.<node-id>`; on resume only the missing branches run.
- Generated code uses the same functions (`plan_node`/`parallel` with a key); resuming generated agents remains the application's job.

## Consequences
- After an abort, `plan` repeats at most the interrupted step, `parallel` only unfinished branches.
- During the run, agent storage contains internal entries with the prefix `grauwolf.`.
