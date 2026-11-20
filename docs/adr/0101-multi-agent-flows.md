# ADR-0101: Multi-agent flows – agents as steps, `for_each` and decisions

- **Status:** Accepted (ADR-0092, F14.13)
- **Date:** 2026-11-10

## Context
AgentWeaver draws *flows* between agents on the main canvas: `next` edges chain agents (and graph
agents), decision nodes branch, a `forEach` edge runs the following part once per element of a JSON
array produced by an agent, and a `collect` edge gathers all results into the next step. Rules:
exactly one entry, no cycles, at most one outgoing flow edge per agent (branching only through
decisions), the per-item part ends on `collect` edges that all target the same step.

grauwolf has sub-agents only as *tools* the model may call (`agent_tools`, ADR-0040), subgraphs
with their own model and tools (ADR-0034, ADR-0043) and `parallel` with a fixed set of branches
(ADR-0045). A deterministic chain of agents and a loop over a list are missing.

## Decision
- **Node kind `agent`** (special kind like `plan`): runs a sub-agent from `agent_tools` as a step,
  without the model deciding. Parameters `agent` (name in `agent_tools`), input/output types from
  the sub-agent's spec (text or typed). The run shares budget, events, user and approvals with the
  parent; each step has its own history. Checkpoints resume after the last finished step.
- **Node kind `for_each`**: input `array<T>`, parameter `body` (a subgraph `T → U` or an `agent`),
  `max_parallel` (default 1, up to 16), `on_error` (`fail` | `skip`); output `array<U>` in input
  order. Progress per element is saved under `grauwolf.for_each.<id>` (ADR-0052), so a resumed run
  only processes the missing elements. A text input is parsed as a JSON array; otherwise the node
  fails with a clear error.
- **Decisions** use the `decide` node (ADR-0100).
- **Mapping from AgentWeaver:** a flow becomes the top-level graph of one spec: each agent step is
  an `agent` node (its own spec in `agent_tools`), a decision is `decide` + `on_decision` edges (ADR-0100), a
  `forEach … collect` section becomes a `for_each` node whose `body` subgraph contains the per-item
  steps, and its output goes to the `collect` target. The export keeps AgentWeaver's rules
  (validated there); grauwolf validates the resulting graph as usual (types, reachability).
- Generator support with the same semantics (shared runtime functions, as for `plan`/`goap`).

## Consequences
- Flows from AgentWeaver run in grauwolf without a separate workflow runner; one spec describes the
  whole flow, sub-agents stay separate specs.
- `for_each` is useful beyond AgentWeaver (batch processing of lists).
- A flow cannot loop (as in AgentWeaver); loops remain the domain of graph edges.

## Implementation notes
Implemented as F14.13; where the implementation differs from the sketch above, the text below
is authoritative.

- **`agent`** is a catalog kind (`params { agent, input?, output? }`, `input → output`, text by
  default). A single spec cannot read the specs of its sub-agents, so it *declares* the types;
  `spec validate` and the loader compare them with the sub-agent's own spec
  (`AgentSpecExt::check_sub_agent_types`, error `SubAgentTypeMismatch`). The spec check verifies
  that `agent` names an entry of `agent_tools` (`UnknownAgent`) and that named types exist.
- **Running a sub-agent** is a new optional method of the tool port,
  `DynTool::call_step(input, parent)`, used by `AgentContext::run_agent_step(name, input)`. The
  default sends text as `{ "request": text }` (so remote A2A agents work unchanged); the
  interpreter's agent tool overrides it: the input stays a JSON value (typed values need no text
  round trip) and the run is `InterpretedAgent::run_as_step`. The calling run is handed over as
  `grauwolf_agents::flows::ParentRun`: the sub-agent's context **adopts** its usage, cost and
  tool-call counters and its limits (`AgentContext::adopt_run`), so a flow cannot exceed its
  budget; history, tools, model and storage stay the sub-agent's own. Approvals: the interpreter
  puts its `ToolApprover` into the context as an opaque *run handle*, and the sub-agent uses it
  unless it has its own. Events of the sub-agent go to the features that its own loader gave it
  (as for agent tools today), those of the nodes to the parent's.
- **`for_each`** is a structural kind like `parallel` (it needs the spec for subgraph bodies), not
  a catalog kind. Parameters (`ForEachParams`, shared by validation, interpreter and generator):
  `body` (`{ agent }` or `{ subgraph }`), `input`, `output`, `max_parallel` (1–16, default 1),
  `on_error` (`fail`, `skip`).
- **Lists as types:** the catalog got `DataType::Array`, written `array<T>`. `input` is `array<T>` or
  `string` (text is parsed as a JSON array; a Markdown code fence around it is ignored; else the
  node fails with a clear error); `output` is `array<U>` or `string` (the list as JSON text, e.g.
  for an agent that takes text). `array<T>` is allowed in node parameters and in subgraph
  signatures, not as the agent's own input or output; the `types` section still has only objects.
  A subgraph body must take `T` and return `U` of the declared lists (with text in or out there is
  nothing to compare).
- **Runtime** (`grauwolf_agents::for_each`): every element runs on `ctx.fork()` (copy of the history
  at the start of the node, shared counters), `max_parallel` at a time with `buffer_unordered`
  within one task (no tokio); results in input order. `fail` stops at the first error: elements
  that have not started do not start, the error is `for_each '<id>', element <i>: <error>`.
  `skip` leaves the element out of the output and reports it with the event
  `for_each_item_failed`. Events `for_each_item_starting/completed/failed { node, index (, error) }`
  (OpenTelemetry: span events). Progress: every finished element (output, or skipped) is saved
  under `grauwolf.for_each.<id>` with `save_progress()` (ADR-0052); a resumed run runs only the
  missing elements and removes the entry at the end.
- **Decisions:** nothing new (`decide` + `on_decision`, ADR-0100); the `for_each` body may contain
  `agent`, `decide` and `for_each` nodes; a body may not include itself (`SubgraphCycle`).
- **Generator:** `nodes::agent_step::<I, O>("name")` and `for_each::for_each::<I, O>(ForEach::new(..), id)`;
  sub-agents come from the loader (`agent_tools`) as for agent tools. Snapshot test for
  `examples/specs/topic_report.json`; slow equivalence test with a mocked Ollama.
- **Example:** `examples/specs/topic_report.json` (researcher → `for_each` writer → summarizer →
  `decide` → retry) and the guide example `docs/user-guide/examples/04-flow.json`.
- Not done: the export from AgentWeaver itself (the mapping above is for the exporter); `collect`
  edges that end at different steps have no counterpart, as the body is one subgraph or agent.

## Live check (2026-11-12)
Ollama `qwen3.5:9b` (`OLLAMA_THINK=false`), `grauwolf spec run examples/specs/topic_report.json
"Three topics about deep sea animals" --events`:

- The researcher answered `["biolumescence", "giant_squid", "abyss_trenches"]`; the `for_each` node
  (`max_parallel: 2`) started elements 0 and 1 together, element 1 finished before element 0
  (`for_each_item_completed` index 1, then 0), element 2 started when a slot was free; the output
  list was in input order, the summarizer wrote one paragraph, `check` answered *yes* with
  probability 1.0. The usage reported by each sub-agent step grew along the whole run (56/20 →
  93/74 → … → 520/226 tokens), i.e. the counters are shared.
- Interrupt and resume: with `limits.max_tokens: 200` (sequential elements) the budget ran out in
  element 2 (`run budget exhausted: 253 of at most 200 tokens used`, event
  `for_each_item_failed`, run failed). The checkpoint held the finished elements under
  `grauwolf.for_each.write_all`; after raising the limit, `spec resume` started only element 2
  (`for_each_item_starting` index 2 only) and finished the flow.
- With the budget at 300 the summarizer step failed on the same shared counter (326 of 300), as
  intended: a flow cannot exceed the budget of the run.
