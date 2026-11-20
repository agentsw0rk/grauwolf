# ADR-0034: Subgraphs in spec and engine, checkpoints at node boundaries

- **Status:** Accepted (User, 2026-11-06); refines ADR-0020, ADR-0022, ADR-0025, ADR-0027
- **Date:** 2026-11-06

## Context
Slice 4 requires subgraphs with their own tool subset (F05.6) and resumption after an abort (checkpoints, ADR-0020) – including in the middle of a subgraph. If a subgraph were merely a node function that internally runs a graph, the engine could neither store its position within it nor return there.

## Decision
**Spec**
- Section `subgraphs`: named graphs with `input`, `output` (type references as for the agent), optionally `tools` (a subset of the spec tools), and `graph`. Subgraphs form their own namespace (ADR-0025).
- A node `{ "kind": "subgraph", "params": { "subgraph": "<name>" } }` binds it. `subgraph` is a structural kind like `start`/`finish`, not an entry in the node catalog: its signature follows from the `input`/`output` of the subgraph (ADR-0022).
- Validation checks each subgraph like the main graph (paths `/subgraphs/<name>/graph/...`), plus unknown subgraphs, unknown tools, and cycles in the binding.

**Engine**
- `GraphBuilder::subgraph(id, graph, scope)` attaches a graph as a node. A `Scope<Ctx>` is invoked on entering and on leaving; `grauwolf-agents` uses it to set the tool subset that `AgentContext` enforces on every tool call and when offering tools (ADR-0027).
- The position of a run is a path of node IDs (`["solve", "call_llm"]`, ADR-0025) plus the input of the next node.

**Checkpoints**
- Before each node, the engine reports a `Checkpoint { path, input, iterations }` to an observer. Together with the serializable state of the context (`AgentContext::snapshot`: history, model, storage) this yields the `ExecutionState` from ADR-0020.
- `Graph::resume(ctx, checkpoint)` continues at the saved position; scopes on the path are entered again. What lay before the checkpoint does not run again.
- Storage happens through a port `CheckpointStore`; a file adapter lives in the facade.

## Consequences
- Resumption works at every nesting depth.
- Tool calls between two checkpoints may run again after an abort (at-least-once, ADR-0020).

## Addendum (2026-11-09): checkpoint without iterations, further stores
- `Checkpoint` (`crates/domain/grauwolf-graph/src/engine.rs`) contains only `path` and `input`, no `iterations`. The context state now also includes self-built tools (ADR-0077).
- Besides the file adapter of the facade there is `SqlCheckpointStore` in `grauwolf-store-sql` (ADR-0048) and `MemoryCheckpointStore` in the domain.
- Long nodes (`plan`, `parallel`) additionally save their progress inside the node (ADR-0052).
