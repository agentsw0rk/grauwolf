# ADR-0078: Limits per run in the spec

- **Status:** Accepted (User, 2026-11-09: "make adjustable limits per run")
- **Date:** 2026-11-09

## Context
A run ends after at most `DEFAULT_MAX_STEPS` (50) node executions per graph, fixed and not adjustable. Tokens and costs can only be limited via the CLI (`--max-tokens`, `--max-cost`, ADR-0072), not in the spec. There is no limit on tool calls; a model stuck in the tool loop runs until the step limit.

## Decision
- Spec field `limits { max_steps?, max_tool_calls?, max_tokens?, max_cost? }`. Checks: `max_steps` ≥ 1, `max_tokens` ≥ 1, `max_cost` > 0; otherwise `InvalidLimit` at `/limits/<field>`.
- `max_steps`: at most this many node executions per graph run (the main graph and each subgraph run count separately, as the 50 did before). `GraphBuilder::max_steps`, or new `TypedGraphBuilder::max_steps`.
- `max_tool_calls`: at most this many tool calls per run, including forks and subgraphs (shared counter in the `AgentContext`, part of `Budget`). Beyond that, a call is not executed; its result is `Error: limit of N tool calls per run reached – answer now without further tools.` This lets the model finish cleanly.
- `max_tokens`/`max_cost`: as `Budget` in ADR-0072. The interpreter takes the values from the spec; a field set via `Interpreter::with_budget` (CLI) takes precedence. `max_cost` without a price table is an error when loading (`InterpreterError::CostLimitWithoutPrices`).
- Generator: `max_steps`, `max_tool_calls` and `max_tokens` are generated; `max_cost` was `Unsupported` (generated code knew no price table); since ADR-0119 it is generated too – the price table comes with provisioning (`GRAUWOLF_PRICES`), and a spec with `max_cost` and no table fails in `provision`.

## Consequences
- Agents bring their limits with them; the CLI options remain for tightening or loosening in individual cases.
- The tool limit does not abort; it lets the model answer. The step limit remains the hard upper bound.
