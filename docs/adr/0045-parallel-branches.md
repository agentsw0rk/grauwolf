# ADR-0045: Parallel branches – node kind `parallel` with a merge strategy

- **Status:** Accepted (User, 2026-11-07: "yes, keep going", item 6 of the agent features)
- **Date:** 2026-11-07

## Context
Several branches should run concurrently, each with its own copy of the context, and their results should be merged (selection by criterion or index, or summarizing; F05.11). Typical cases: several perspectives on the same question, several solution paths with subsequent selection.

## Decision
- `AgentContext::fork()` copies the context (history, model, storage, tools, features); `absorb(branch)` takes over history, model, and storage of a branch.
- `grauwolf_agents::parallel::run_parallel(ctx, branches, input, merge)`: each branch (graph plus optional scope) runs on its own fork, all concurrently (`join_all`, no runtime dependency). Merge strategies:
  - `concat` (text output only): results as `## <branch>` sections; the caller's history remains unchanged.
  - `index { index }`: result and context of the branch `index` are taken over.
  - `llm_select { criteria }`: an evaluation without tools selects the best branch (as `llm_request_best_of`); its result and context are taken over.
- Specs: node kind `parallel` (structural, not in the catalog, like `subgraph`/`plan`) with `params { subgraphs: [..], merge, index, criteria }`; all subgraphs have the same signature, which is the node's signature. Interpreter and generator use `run_parallel`.

## Consequences
- Events of the branches appear interleaved; checkpoints lie at the boundary of the `parallel` node (a resume restarts all branches).
- `concat` presupposes text output; for structured outputs, `index` and `llm_select` are available.

## Addendum (2026-11-09): progress within the node
A `parallel` node saves the result and context of each finished branch; a resume restarts only the missing branches, not all of them (ADR-0052).
