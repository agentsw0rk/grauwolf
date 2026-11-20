# ADR-0119: Generated code runs like the interpreter – prices, caller budget, checkpoints, conversations, attachments, approvals

- **Status:** Accepted (User, 2026-11-13: "close the gaps in generated code: prices, caller budget,
  checkpoints, conversations, attachments, approvals")
- **Date:** 2026-11-13
- **Related:** ADR-0116 (run lifecycle; its open points), ADR-0114 (provisioning, `RunOptions`),
  ADR-0093 (generated service), ADR-0084 (generated apps), ADR-0071 (prices), ADR-0072/0078 (budget,
  limits, `max_cost`), ADR-0020/0052 (checkpoints, resume), ADR-0060 (conversations), ADR-0081
  (attachments), ADR-0061 (approvals), ADR-0099 (shell mode needs approval), ADR-0023 (frame sink).

## Context
Since ADR-0116 the interpreter and generated code share one run lifecycle, but generated code uses
only part of it. Generated crates and services lack: prices (so no cost and `limits.max_cost` is
refused at generation time), a caller-supplied budget, checkpoints and resume, conversations (a
generated service accepts `conversation_id` but ignores it), attachments, interceptors and approvals
(so `spec generate` refuses shell-mode `run_command` with `approve_each_call`, ADR-0099), and text
deltas on `/run/stream`. The server already hands every served agent the same `RunExtras`
(conversation, attachments, approver, frame sink, features); the generated agent drops most of them.

## Decision
1. **One path for run options.** `RunOptions` of generated apps carries what a run can have:
   user, features, caller budget, conversation (store + id), attachments, approver, frame sink,
   checkpoints (store + run id) and resume. The generated `run_provisioned` passes them to the run
   lifecycle exactly as the interpreter does; `GeneratedAgent` maps the server's `RunExtras` onto them,
   so a generated service honours `conversation_id`, attachments, `/approvals` and streams text
   deltas like `spec serve`.
2. **Prices** come from the same place as for the interpreter (provisioning reads the price table
   from the same configuration the CLI uses); generated code installs them, events carry `cost`, and
   `limits.max_cost` is no longer refused by the generator.
3. **Approvals in the domain.** The approval gate (the interceptor that asks an approver for tools that
   need approval, ADR-0061) moves from the interpreter into `grauwolf-agents`, so the interpreter and
   generated code use the same one. The tools needing approval are derived from the spec the same way
   (`self_tools.confirm`, shell-mode `run_command` with `approve_each_call`, ADR-0099); without an
   approver such a call is refused with the same text. `spec generate` no longer refuses shell mode
   with `approve_each_call`.
4. **Checkpoints and resume** become functions of the generated app (`run_provisioned` with a
   checkpoint store and run id, and a resume entry), matching the interpreter's `run_checkpointed` /
   `resume`. The generated service exposes them only as far as `spec serve` does.
5. **Equivalence.** Each gap gets a test that the generated code behaves like the interpreter: fast
   tests where the lifecycle or the gate decide, the existing slow equivalence suite extended only
   where a generated crate must actually be built (one case per feature at most).

## Consequences
- What runs under `spec run`/`spec serve` runs the same way as a generated crate or service; the open
  points of ADR-0116 are closed.
- The generator refuses fewer specs (`limits.max_cost`, shell mode with approvals).
- `RunOptions` and the generated function signatures grow; documented in user guide chapter 11.

## Implementation notes (2026-11-13)
- **Approval gate.** `ApprovalGate` and `ToolApprover` moved to `grauwolf_agents::approval` (same
  texts); `AgentSpec::approval_tools()` (spec-model) derives the gated tools (`self_tools.tool` with
  `confirm`, `run_command` in shell mode with `approve_each_call`) for the interpreter and the
  generator.
- **One value for the caller's handles.** Instead of growing every generated function, generated
  code takes `grauwolf_agents::run_handles::RunHandles { prices, budget, frame_sink, approver,
  conversation, attachments, checkpoints, resume }`. Its methods are the interpreter's `equip` and
  lifecycle options for typed graphs: `resume_state()`, `context(..)` (new or restored context with
  prices, `merged_budget` – the caller's fields win over the spec's `limits`, as in the interpreter –,
  frame sink, run handle and `ApprovalGate`), `start(..)` (conversation, attachments, checkpoints,
  `resumed()`) and `run_graph(..)`. `limits.max_cost` without prices is `StartError::CostLimitWithoutPrices`.
- **Generated code.** The core is `run_handled(executor, Option<input>, [external,] [memory,]
  [decider,] features, handles)`; `run_observed` calls it with `RunHandles::default()`. With the feature
  `app`, `run_provisioned` and the new `resume_provisioned(&provisioned, store, run_id, options)` build
  the handles with `RunOptions::handles(provisioned.prices())`. `RunOptions` gained `budget,
  conversation, attachments, approver, frame_sink, checkpoints, resume` (and `resuming(store, id)`).
- **Prices.** `ProvisionOptions::with_prices` or, without it, the file named by `GRAUWOLF_PRICES`
  (relative to the base directory; new section `PricesConfig`, same format as `--prices`; the CLI reads
  it through the same `prices::load`). `limits.max_cost` without a table fails in provisioning, before
  the first run. The generator no longer refuses `limits.max_cost` nor shell mode with
  `approve_each_call`.
- **Server.** `GeneratedAgent` maps conversation, attachments, approver and frame sink of `RunExtras`
  onto `RunOptions`. `/run`, A2A and MCP have no approver, so such calls are refused there, as with
  `spec serve`. By default the generated service keeps conversations in memory; `GRAUWOLF_CONVERSATIONS_DIR` (relative to `GRAUWOLF_DATA_DIR`, `FileChatHistoryStore`) or `GRAUWOLF_CONVERSATIONS_DB` (`sql_chat_history_store`, secret URL, feature `sql` of the server and the facade) make them persistent, like `spec serve --conversations-dir/--conversations-db`; both set is a start-up error (`ServiceConfig::from_lookup`). The store is shared by the main port and the additional A2A port. Open point closed. With `--service` alone the crate's facade has no `sql`; the database needs `--full-facade`.
- **Deviations.** Checkpoints/resume are functions of the generated app only; the service does not
  expose them (as `spec serve` does not). The slow equivalence suite got one new case
  (`generated_apps_price_their_runs_and_keep_conversations`: `GRAUWOLF_PRICES` and a conversation of two
  requests against a mocked Ollama); approvals, attachments, frame sink, budget and resume are decided
  in `grauwolf-agents` (`tests/run_handles.rs`, 13 tests, fake LLM) and the server mapping in
  `grauwolf-server` (`tests/service.rs`, 2 new tests with a generated-agent stand-in).
- **Tests.** Fast: 1 (approval gate), 13 (run handles), 1 (`approval_tools`), 2 + 2 + 2 + 2 (host: `RunOptions`,
  provisioning prices, `prices::load`), generator cost-limit and shell-gate cases, server 2.
  Slow, run one at a time with `generated-equivalence` deleted afterwards: the new case,
  `generated_crates_behave_like_the_interpreter`, `generated_apps_provide_providers_tools_and_memory_themselves`,
  `generated_services_answer_health_and_run`, `generated_apps_install_the_agent_features_in_their_sub_agents`,
  `generated_flows_run_agent_and_for_each_nodes_like_the_interpreter`,
  `generated_goap_runs_the_review_loop_with_agent_and_subgraph_actions`,
  `generated_crates_route_by_the_decider_they_are_given` – all pass.
- **Live check** (generated service built in a debug build of its own, Ollama `qwen3.5:9b`,
  `OLLAMA_THINK=false`): from `07-conversation.json`, two requests with `conversation_id` `c1` – the
  second ("What is my name and what do I like?") answered "Your name is Ada, and you like tea.", a
  request without the ID did not know the name. `/run/stream` of a service from `streaming_echo.json`
  (model changed to `qwen3.5:9b`) sent `event: text` deltas ("Hello", ",", " how", …); a spec with
  plain `llm_request` nodes sends none, as with `spec serve`.
