# ADR-0039: Planner – plan loop as a crate, LLM planner with subgraph steps, node kind `plan`

- **Status:** Accepted (User, 2026-11-07: "Planner, all the main features that have to do with agents have priority")
- **Date:** 2026-11-07

## Context
Planning agents (E10) work in a loop: create plan → execute step → finished? → re-plan if necessary. They come as an LLM planner (optionally with a critic) and as GOAP (A* search over actions with preconditions, effects, and costs). A separate agent type alongside graph agents, whose steps run without tools, would be too limited.

## Decision
- New domain crate `grauwolf-planner` (depends on grauwolf-agents and grauwolf-graph).
- Trait `Planner` with `initial_state`, `build_plan`, `execute_step`, `is_completed`, `output`; a generic loop `run_planner` with an iteration limit and planner events (F10.7).
- `LlmPlanner`: plan as a structured response (`Plan { goal, steps }`), optionally with a critic (a second call that returns "continue" or "re-plan" with a justification). **Every step runs through a graph** (`Graph<AgentContext>`, text → text), thus also with the tool loop; the result of the last step is the output.
- GOAP generic over a state `S: Clone + Eq + Hash` with its own A* and cost functions; replan when the actual state deviates from the assumption (F10.4–F10.6).
- In specs: node kind `plan` with `params { subgraph, critic, max_iterations }`, signature `string → string`; the named subgraph (signature `string → string`) executes each step, and its tool subset applies. Interpreter and generator use the same `LlmPlanner` implementation.

## Consequences
- Planners fit into graphs (e.g., pre- and post-processing around a `plan` node) instead of being a separate agent type.
- Checkpoints for now lie at the boundary of the `plan` node: resuming restarts the planning.
- GOAP remains a code API; the DSL gets it only once actions can sensibly be described as data.

## Addendum (2026-11-09): checkpoints in the plan and GOAP in the DSL
- A `plan` node saves its progress after each step; resuming continues with the next step instead of re-planning (ADR-0052).
- GOAP is part of the DSL as the node kind `goap` (ADR-0070).
