# ADR-0041: Strategy templates in the DSL – `strategy` instead of `graph`

- **Status:** Accepted (User, 2026-11-07: order of the agent features, item 2)
- **Date:** 2026-11-07

## Context
Common flows should be available as ready-made strategies (F05.12): single run (tool loop, sequential or parallel), ReAct (think before each tool call), chat agent, and task with verification (F05.13). So far every spec must spell out its graph; the tool loop appears almost identically in nearly every example.

## Decision
- Specs may specify a `strategy { kind, params }` instead of `graph`. `grauwolf-spec` expands it on load into an ordinary `GraphSpec`; validation, interpreter, generator, and builder see only the graph. Specifying both at once, or an unknown kind, is a load error.
- Kinds:
  - `single_run` (`parallel_tools`: bool): tool loop with `llm_request`, `execute_tool`/`execute_multiple_tools`, `llm_send_tool_result`/`llm_send_multiple_tool_results`.
  - `react` (`reasoning_prompt`: text): think first, then act; think again after each tool result.
- Two new node kinds for the catalog:
  - `llm_reason` (`params { value, prompt }`, signature `value → string`, `value` ∈ `string`, `tool_result`, `tool_results`): appends the value (text as a user message, results as tool results), then the reasoning prompt, asks **without tools**, and returns the thought.
  - `llm_continue` (`string → message`): asks with tools, without appending the input (it is already in the history).
- Further kinds (chat agent, task with verification) follow as separate templates.

## Consequences
- Short specs for the common cases; the builder can choose strategies instead of building graphs.
- After `llm_reason`, two assistant messages follow each other (thought, then answer or tool call); the provider adapters must allow this.

## Addendum: several tool calls in one answer (2026-11-11)
- **Bug:** the sequential templates (`single_run` without `parallel_tools`, `chat` with `parallel_tools: false`, `verified`, `react`) routed a tool answer only with `on_tool_call`, which needs exactly one call. When a model (small ones do) sent two or more calls in one answer, no edge matched and the run failed with `no matching edge from node 'call_llm'`.
- **Fix:** every answer is now routed by three edges in this order: `on_tool_call` (exactly one call → `execute_tool`, unchanged), `on_multiple_tool_calls` (→ `execute_tools_in_order`) and `on_assistant_message`. The graph engine takes the first matching edge in declaration order (ADR-0021), and `on_multiple_tool_calls` matches any answer with at least one call, so it only ever sees answers with two or more calls because the single-call edge comes first. `execute_tools_in_order` is `execute_multiple_tools` with the new parameter `sequential: true` (default `false`): it runs the calls of the answer one after another in call order – not concurrently, `parallel_tools` stays the opt-in for that – with tool events, interceptors, approvals and `max_tool_calls` applying per call. Then all results go to the LLM (`send_results`; in `react`, `reflect_all` thinks over `tool_results` first), and the loop continues. `verified` still ends in `verify`.
- The node ids of the single-call path (`call_llm`, `execute_tool`, `send_result`, `think`, `reflect`) are unchanged. The parameter is implemented in the interpreter and in generated crates alike (`nodes::execute_tools_in_order`).
