# ADR-0070: GOAP in the DSL

- **Status:** Accepted (User, 2026-11-09: "first do GOAP in the DSL (E10)")
- **Date:** 2026-11-09

## Context
GOAP (F10.4–F10.6) has existed since ADR-0039 only as a Rust API over an arbitrary state `S`, with closures for precondition, expected effect, cost and execution. ADR-0039 postponed the DSL "until actions can sensibly be described as data". The classic GOAP model describes the world as named true/false facts; preconditions, effects and goals are then pure data, and the execution of an action is a subgraph.

## Decision
- New special kind `goap` (like `plan`, ADR-0039), signature `string → string`, parameters:
  - `state`: initial facts `{ "<fact>": bool }`; they also define which facts exist;
  - `actions`: `[{ name, subgraph, pre?, effects, cost? (default 1), description? }]`;
  - `goals`: `[{ name, facts }]`;
  - `max_iterations` (default as for the `plan` node).
- Checked: facts in `pre`, `effects` and `goals` must exist in `state`; action names unique; subgraphs exist and have input `string`; at least one action and one goal; `cost` ≥ 0.
- Execution: before each step, A* searches from the actual state for the cheapest path to any goal (existing `GoapPlanner`). The subgraph of the next action receives `"<task>\n\nAction: <name>"` (plus `description`). After success, the `effects` apply; if the subgraph yields an object, its `bool` fields overwrite facts of the same name (if the observed state deviates, replanning occurs). The result is the output of the last action as text; if a goal is already reached, the empty text.
- `grauwolf_planner::goap::fact_planner(params, step_graphs)` builds the planner from the parameters; the interpreter and generated code call it the same way. `grauwolf_planner::planner_node` turns every planner with text input/output into a node with progress under `grauwolf.plan.<id>` (ADR-0052); `plan_node` uses it.

## Addendum: Enumerations (User, 2026-11-09: "do enumerations")
- A fact is true/false or a text from a fixed list: `values: { "<fact>": ["value", …] }`, `state: { "<fact>": "value" }`. Preconditions, effects and goals compare or set exactly one value; the state space stays finite.
- Checked: text facts have a list of values, all texts are in it, true/false and text facts are not mixed, and `values` names only text facts from `state`.
- Observed state: a text field of the object output overwrites the fact only with an allowed value; other values are ignored.
- Free numbers and free texts deliberately remain excluded (infinite state space, or nothing to plan over); bounded integers follow only if needed.

## Addendum: Numbers, operators, priorities and executors (User, 2026-11-13: F14.15, GOAP like AgentWeaver's planner)
AgentWeaver's planner has numeric state, comparison operators, goal priorities, actions executed by sub-agents with an input template, success checks and failure effects. Every existing `goap` spec keeps its meaning; everything below is an extension.

### Facts, conditions and effects
- **Fact types** follow the initial value in `state`: `true`/`false` (bool), a JSON integer (**integer**, `0`), a JSON float (**number**, `0.0` – write the decimal point), a text with a list under `values` (**enumeration**, as before), and a text without a list (**free text**). Free texts are allowed now; they can only be compared with `eq`/`ne` and set, never ordered (typically the input and output fields, see below).
- **Conditions** (`pre`, goal `facts`): a plain value means equality, as before. An operator object `{ "eq" | "ne" | "gt" | "lt" | "ge" | "le": value }` (exactly one operator per fact) compares otherwise; the ordering operators need an integer or number fact, `ne` and `eq` work for every type. The value must fit the fact type (an integer literal is fine for a number fact, not the other way round).
- **Effects:** a plain value sets the fact; `{ "add": n }` adds to an integer or number fact (an integer `n` for integers). The planner applies `add` in the expected effect, too. AgentWeaver allows `increment` only after the execution; grauwolf allows it in the expected effect because the search is bounded (see below). Preconditions such as `{ "lt": 3 }` are how loops over counters stay bounded.
- **Observed state:** a field of an object output overwrites the fact of the same name if the value fits the fact (integer, number, text for free-text facts; enumerations only with an allowed value).

### Actions: executors, input, success check, failure
- An action has exactly one **executor**: `subgraph` (as before) or `agent` (a name in `agent_tools`; the sub-agent runs as a step like the `agent` node of ADR-0101: shared budget and counters, its own history).
- `input` is an optional template with `{{fact}}` placeholders (the value of the fact as text) and `{{task}}` (the input of the planner; always the planner input, even if a fact has that name). Unknown placeholders are errors. Without `input` the executor gets `"<task>\n\nAction: <name>"` (+ `description`), as before; with a template the description is not added.
- `output_fact` names a free-text fact that receives the answer of the executor as text (also if the success check fails, so a reviewer's remarks reach the next action). An object answer is stored as its JSON text.
- `success_contains` (text): the action succeeded iff the answer contains it; without it, iff the executor did not fail. An executor error is a failure, too.
- Three effect lists: `effects` is the expected effect the planner searches with; `on_success` (default: `effects`) and `on_failure` (default: none) apply after the run. On a failure with `on_failure`, it applies and planning continues from the actual state; **without `on_failure` the node fails** (as AgentWeaver's executor abort). Observed state from object outputs applies after either list. An **exhausted run budget** (ADR-0072: `max_tokens`, `max_cost`, `max_prompt_tokens`) is never an ordinary failure: when the executor fails and `AgentContext::budget_exhausted()` reports a reason, the node fails with it even if `on_failure` is set (2026-11-13; before, `on_failure` hid it and only `max_iterations` ended the run). Other executor errors, including a step limit of the executor's graph, stay failures.

### Goals and bounded search
- A goal has a `priority` (integer ≥ 0, default 0). The planner finds the cheapest plan per reachable goal and pursues the goal with the smallest `cost / (priority + 1)` – the same rule as AgentWeaver's export, so exports behave alike; ties go to the goal declared first. A run still ends as soon as any goal holds.
- Numbers make the state space infinite. The A* search is bounded by `max_search_nodes` (states examined per search, default 10 000) and `max_plan_depth` (actions per plan, default 50), both integers ≥ 1 parameters of the node. A goal not reached within them has no plan; if no goal has one, the run fails with `no plan found: … within the search limits`. Guards such as `{ "lt": 3 }` in preconditions remain the way to bound a loop on purpose.

### Planner input and output
- `input_fact` (a free-text fact) is set from the input of the node before planning; `output_fact` (a free-text fact) delivers the result of the node. Without it, the result stays the output of the last action. The signature stays `string → string`.

### Checks
`spec validate` reports, with pointers as before (`InvalidParams { kind: "goap" }` at the node's `params`): unknown facts, an operator that does not fit the fact type (ordering on bool, enumeration or text; two operators in one entry; unknown operators), `add` on a fact that is not an integer or number, values that do not fit the type (an integer for a number fact is fine, a fraction for an integer fact is not), both or none of `subgraph`/`agent`, an unknown placeholder or an unclosed `{{` in `input`, `input_fact`/`output_fact` that are not free-text facts, a negative or fractional `priority`, `max_search_nodes`/`max_plan_depth`/`max_iterations` below 1. An `agent` that is not in `agent_tools` is `UnknownAgent`. A text fact without a list is a free text now, no longer an error.

### Interpreter and generated code
Both build the planner with `grauwolf_planner::goap::fact_planner(params, steps)` as before; `steps` holds graphs only for the subgraph actions. An `agent` action runs the sub-agent from `agent_tools` as a step (`AgentContext::run_agent_step`, ADR-0101), so generated crates get their sub-agents from the loader like for `agent` nodes. Example: `examples/specs/review_loop.json` (with `draft_writer.json` and `draft_reviewer.json`) mirrors AgentWeaver's review loop: write, review (`success_contains: "APPROVED"`, `on_failure` counts an attempt), fix while `review_attempts < 3`, save; the goal `saved` has priority 1, the goal `give_up` (three rejected reviews) ends the run with the last draft. Tests: interpreter (`tests/goap_review_loop.rs`), generator snapshot and a slow equivalence case. The sub-agent receives the rendered input as text, so it must take text; the spec check does not compare the types of `agent_tools` entries here (only the `agent` node declares them).

### Mapping from AgentWeaver
`initialState` → `state` (texts become free-text facts; AgentWeaver has no enumerations), `operator` `equals`/`notEquals`/`greaterThan`/`lessThan` → `eq`/`ne`/`gt`/`lt` (`ge`/`le` are extra), `op: increment` → `{ "add": value }`, `executor` → `agent`, `inputTemplate` → `input`, action `outputKey` → `output_fact`, `successContains` → `success_contains`, `onSuccess`/`onFailure` → `on_success`/`on_failure`, goal `priority` → `priority`, planner `inputKey`/`outputKey` → `input_fact`/`output_fact`, `maxIterations` → `max_iterations`.

### Deviations from AgentWeaver
- `add` is allowed in the expected effect (`effects`) as well; the search is bounded by `max_search_nodes` and `max_plan_depth`, which AgentWeaver does not need.
- `subgraph` executors exist next to `agent` executors; a text fact is an enumeration if it has a list under `values` and a free text otherwise (before, a text fact without a list was an error).
- Numbers keep their JSON type (`0` integer, `0.0` number), because the type of a fact is read from its initial value.
- A run ends as soon as any goal holds (as before); `priority` only decides which goal is planned for.
- The sub-agent of an `agent` action receives text; the spec check does not compare its declared types.

### Live check (2026-11-13)
Ollama `qwen3.5:9b` (`OLLAMA_THINK=false`), `grauwolf spec run docs/user-guide/examples/06-goap-review.json "autumn rain" --events` (writer and reviewer as sub-agents, `save` as a subgraph):

- First run: plans `write, review, save`; the reviewer answered `APPROVED`, `save` ran, and the result of the node (`output_fact: draft`) was the written text.
- Second run with a stricter reviewer (it must find a lantern in the text): after `write` and a `review` that answered `REJECTED: The text lacks the required mention of a lantern.`, `on_failure` set `reviewed` and counted `review_attempts`; the plan before the next step changed from `save` to `fix, review, save` (`plan_built` iteration 3). The writer revised the text with the remarks (`{{feedback}}`) and the new text mentioned a lantern, the second `review` answered `APPROVED`, `save` ran, and the node delivered the revised text, not the first draft.
- Unit tests cover the other paths (give up after three rejections, executor errors, no `on_failure`).

## Consequences
- GOAP agents can be written without Rust; every action can be an LLM subgraph with tools.
- States are true/false and enumeration facts; numbers remain reserved for the Rust API.
