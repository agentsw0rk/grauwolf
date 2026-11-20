# ADR-0077: Checkpoint runs with runtime handles and self-built tools

- **Status:** Accepted (User, 2026-11-09: "next, do the checkpoints")
- **Date:** 2026-11-09

## Context
`run_checkpointed` and `resume` (ADR-0020, ADR-0034) take no `RunExtras`. With `--run-id` or `spec resume`, this silently leaves out: runtime tools from the tools directory (ADR-0074) and tools per user (ADR-0059), automatic memory (ADR-0069), the approver (ADR-0061; checkpoint runs always rejected approval-requiring tools), features and frame sinks of the run. Self-built tools (ADR-0075) are not in the checkpoint and are missing after resuming.

## Decision
- `InterpretedAgent::run_checkpointed_with(input, store, run_id, extras)` and `resume_with(store, run_id, extras)` take `RunExtras` like `run_with`: `tools`, `approver`, `memory`, `feature`, `frame_sink`. `run_checkpointed`/`resume` are short forms without extras. A conversation (`conversation`) combined with checkpoints remains an error (`InterpreterError::CheckpointConversation`), as in the CLI.
- Automatic memory: `auto_recall` at the start (the retrieved facts then stand in the history, and thus in the checkpoint); for `auto_remember`, the run stores its input in the agent storage under `grauwolf.run_input`, so that a resumed run can also store facts after success.
- Self-built tools: `ToolShelf` remembers the definition (`NewTool`, serializable) of each built tool; `ContextSnapshot` gets `self_tools: Vec<NewTool>` (missing in older checkpoints). `resume_with` rebuilds them via the factory (build cache) and puts them on the shelf before resuming; an error there aborts the resume.
- CLI: `spec run --run-id` and `spec resume` pass the tools directory, memory (the `--user` scope or the shared scope) and the console approver.

## Consequences
- Checkpoint runs behave like ordinary runs; ADR-0061's "checkpoint runs never" is superseded as soon as an approver is passed.
- A resumed run sees the tools of the directory as they are at the time of resuming, not those at the start.

## Addendum: Usage across resumes (User, 2026-11-09: "make usage carry over resumes")
After `resume`, the token sum, costs and the number of tool calls started again at zero. Limits (ADR-0072, ADR-0078) thus applied per segment instead of per run, and `agent_completed` reported only the remainder. `ContextSnapshot` therefore stores `usage`, `cost` and `tool_calls` (missing in older checkpoints and then counted as zero); `AgentContext::restore` sets them again, and forks share them as before. Usage of a node that aborts before its next checkpoint is still lost.

## Addendum: `--run-id` with `--user` (User, 2026-11-09: "combine --run-id with --user")
`spec run --run-id` accepts `--user` and gives the run the memory of this user (tools and automatic memory); `spec resume` has its own `--user` option for this. The user is not part of the checkpoint: when resuming, it must be given again; without `--user`, the shared scope applies. For the local CLI this is justifiable, because there the same person manages all memory files; a service with checkpoints would have to record the owner in the checkpoint.
