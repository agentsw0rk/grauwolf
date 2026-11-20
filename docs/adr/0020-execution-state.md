# ADR-0020: ExecutionState as the complete, serializable run state

- **Status:** Accepted (review iteration 1, 2026-11-06)
- **Date:** 2026-11-06

## Context
Checkpoints and resume need more than the node input: a checkpoint must contain message history, model, parameters, tool names, storage, iteration counter and node path. A storage made of `Box<dyn Any>` is not serializable.

## Decision
The engine maintains an `ExecutionState { version, path_stack, node_input: Value, prompt, model, params, tool_names, storage: Map<String, Value>, iterations }`. `path_stack` contains the subgraph scopes with stable path IDs (`sub/node`, ADR-0025).
- `StorageKey<T>` requires `T: Serialize + DeserializeOwned` (storage is thus always persistable).
- Non-serializable handles (clients, connections) live in `Extensions` and are re-injected on resume.
- Checkpoints are only created outside parallel branches.
- Tool side effects between two checkpoints count as at-least-once; rollback tools (F07.16) mitigate this.

## Consequences
Resume correctly restores the session and subgraph context. Storage is limited to serializable values.

## Addendum (2026-11-09): actual shape of the run state
A leaner shape is implemented (ADR-0034, `crates/domain/grauwolf-agents/src/checkpoints.rs`): `ExecutionState { version, checkpoint: Checkpoint { path, input }, context: ContextSnapshot { prompt, model, storage, self_tools } }`. `params`, `tool_names` and `iterations` are not stored; the engine restores tool subsets when entering the subgraphs, self-built tools come via `self_tools` (ADR-0077). Runtime handles are not kept in `Extensions` but are passed as `RunExtras` on resume (`resume_with`, ADR-0077).
