# ADR-0116: The run lifecycle – one sequence for every agent run

- **Status:** Accepted (User, 2026-11-13: architecture review, candidate "run lifecycle for the
  interpreter and generated code"; decisions Q1–Q11 of the review session)
- **Date:** 2026-11-13
- **Related:** ADR-0084 (generated code like `spec run`, event order), ADR-0069/0079 (automatic
  memory), ADR-0113 (knowledge as context), ADR-0020/0052 (checkpoints, resume), ADR-0060
  (conversations), ADR-0114 (provisioning), ADR-0115 (agent tree). Term: `GLOSSARY.md` (agent run).

## Context
An agent run follows one sequence: `agent_starting` → automatic memory recall → knowledge injection
→ graph run → automatic remember → `agent_completed` / `agent_failed` with usage and cost. Today it
exists four times (facts gathered 2026-11-13):
- in the interpreter three times – `run_as_step` (normal runs, conversations, sub-agent steps),
  `run_checkpointed_with` (stores the input for a later remember) and `resume_with` (no recall, no
  knowledge, no attachments; starts with the checkpoint's input);
- in the generator once more, as Rust source strings (`run_body`), with its own differences: the
  knowledge source is the first external tool that has one (the interpreter looks it up by name at
  load time), and `serde_json::to_value(output)?` can fail before any completion event.
Further findings: the interpreter saves a conversation after `agent_completed`, so a failed save
returns an error after "completed" was reported. Only the slow equivalence tests (minutes, cargo
builds of generated crates) keep the four copies in line; knowledge as context (ADR-0113) had to be
added twice with its own slow test.

## Decision
1. **New module `grauwolf_agents::run_lifecycle`** with `AgentRun`, in the domain (no tokio, no
   stores – memory, knowledge, conversation store and checkpoint store come in as the existing
   ports). It owns the **order of the steps and the events** of a run; the **graph run is passed in**
   as a function, so the untyped interpreter graph and the typed generated graph both use it.
2. **Variants are options, not copies.** Plain run, run with checkpoints (input stored for the
   remember step after a resume) and resume are options of the same lifecycle. A resume does **not**
   recall memory or inject knowledge again – both are already in the checkpoint's history.
3. **Guarantees.**
   - Every run ends with exactly one of `agent_completed` / `agent_failed`; an error converting the
     output (generated code) becomes `agent_failed`.
   - A conversation is saved **before** the completion event; a failed save is `agent_failed`.
4. **Knowledge found the same way.** The knowledge context comes from the provisioned agent, looked
   up by the name `knowledge.tool`, with one error text, in the interpreter and in generated code.
5. **Generated code calls the lifecycle** instead of emitting the sequence: the generator emits the
   graph, the types and one call.
6. **Scope: only the sequence.** Generated code still lacks prices (cost), a caller budget,
   checkpoints/resume, conversations, attachments, interceptors and approvals; the lifecycle offers
   them as options, generated code adopts them in a later step (open points below).
7. **Tests.** Order and events are verified by fast unit tests in `grauwolf-agents` with a fake LLM,
   fake memory and fake knowledge. The slow equivalence tests are reduced to "the generated graph
   behaves like the interpreted one" (node events and output); the memory and knowledge equivalence
   cases are replaced by the unit tests.
8. **Order of work**, every commit green: the module with its tests → the interpreter's three
   variants on it → generated code on it → slim the equivalence tests, docs.

## Consequences
- One place decides what an agent run is; the next run-level feature is one change, tested fast.
- Behaviour changes (deliberate): a failed conversation save reports `agent_failed`; a generated run
  whose output cannot be converted reports `agent_failed`; generated code finds the knowledge source
  by name.
- Open points: prices, caller budget, checkpoints/resume, conversations, attachments, interceptors
  and approvals in generated code – closed by ADR-0119.

## Implementation notes (2026-11-13)
- **Interface.** `grauwolf_agents::run_lifecycle::AgentRun::new(input)` with the options
  `with_memory(memory, recall, remember)`, `with_knowledge(context)`, `with_conversation(store, id)`,
  `with_attachments(..)`, `with_checkpoints(store, run_id)` and `resumed()`;
  `run(&mut ctx, |ctx, observer| Box::pin(graph_run))` returns the output of the graph run
  (`O: Serialize`) or a `RunError { Graph, Output, ChatHistory }`. The lifecycle loads the
  conversation history itself, so the caller only builds and equips the context (including compressed
  memory and `adopt_run`, which stay outside).
- **Order.** `agent_starting` (after the history was loaded and attachments were attached; a failing
  load returns before any event, as before) → recall → knowledge → graph run → output as JSON → remember
  → conversation save → `agent_completed`. Any error after the graph run, including the output
  conversion and the save, is `agent_failed`.
- **Graph run as a function.** The closure must return a `BoxFuture<'a, ..>` for every borrow of the
  context, so it cannot borrow the interpreter's graph; `InterpretedAgent` now holds an `Arc<Graph>`
  and the closure owns a clone. Generated code builds `graph()?` before the lifecycle call, so a failing
  graph construction returns before `agent_starting` (it used to return after it, without a
  completion event).
- **Knowledge by name.** `grauwolf_tools::knowledge_context_of(tools, name)` is the single lookup;
  generated code uses it on the provided external tools with the interpreter's text ("tool '<name>' was
  not provided (MCP server, knowledge, sub-agent or built-in tool)").
- **Small differences.** A checkpoint run keeps its input for the remember step only when a memory is
  present (before: whenever `auto_remember` was set).
- **Tests.** Fast unit tests in `run_lifecycle` (12): plain run, failing graph, failing output
  conversion, recall/knowledge/remember order, memory only as asked, failed graph remembers nothing,
  conversation saved before completion, failed save is `agent_failed`, history follows the system
  prompt, checkpoints saved, resumed run skips recall and knowledge and remembers the stored input,
  checkpoint run keeps its input; plus 2 for `knowledge_context_of`. `grauwolf-agents` lib tests 71 → 83,
  `grauwolf-tools` +2 tests. Slow equivalence tests 10 → 8 (the memory and knowledge cases are removed
  together with `tests/support/fact_log.rs` and `knowledge_log.rs`; the `fact_keeper` case that
  covered durable facts from compression is gone with them). Durable facts from compression are
  guaranteed by construction instead: `AgentRun::remember_compressed` is the one place that hands
  the memory to the context (`set_fact_memory`), the interpreter and generated code both only call
  it; fast tests: two lifecycle unit tests (a fake LLM answers `durable`, a logging `FactMemory`
  receives it – or not, without the option) and a generator test that the emitted code calls
  `run.remember_compressed()` exactly when the spec asks for it; the interpreter's
  `remembers_durable_facts_of_a_chat_compression` still covers the spec wiring). Snapshots of the generated libraries were updated (the run body is one call).
- **Slow tests run (one at a time):** `generated_crates_behave_like_the_interpreter` (31 cases),
  `generated_apps_provide_providers_tools_and_memory_themselves`,
  `generated_crates_route_by_the_decider_they_are_given`,
  `generated_apps_install_the_agent_features_in_their_sub_agents`,
  `generated_flows_run_agent_and_for_each_nodes_like_the_interpreter`,
  `generated_goap_runs_the_review_loop_with_agent_and_subgraph_actions`,
  `generated_services_answer_health_and_run` – all green; before the removal also the memory and
  knowledge cases. Not run: `generated_crates_with_tool_sources_build_after_their_modules_are_built`
  (does not touch the run body). Smoke test: `spec run` of `07-memory-auto.json` against Ollama
  (remembered a fact in the first run, recalled it in the second).
