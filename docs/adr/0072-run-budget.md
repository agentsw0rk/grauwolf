# ADR-0072: Budget per run

- **Status:** Accepted (User, 2026-11-09: "make a cost budget per run")
- **Date:** 2026-11-09

## Context
Since ADR-0067/0071, the tokens and costs of a run have been visible, but only afterwards. An agent that gets stuck in a tool loop, or keeps sending a large history, can consume unlimited tokens up to `max_steps`.

## Decision
- `grauwolf_agents::Budget { max_tokens?, max_cost? }`; `max_tokens` counts input and output tokens together, `max_cost` is in the currency of the price table.
- `AgentContext::with_budget`: before every LLM request (normal, streaming, structured, multiple responses, summary, fact request), the context checks the run's sum so far, including forks. If a limit has been reached, it returns `LlmError::BudgetExceeded` with the limit and the consumption; the executor does not retry it, and the run ends with `agent_failed` (with `usage`/`cost`).
- A call may exceed the limit, because its consumption is unknown beforehand; after that, no further call starts.
- `Interpreter::with_budget`, `AgentLoader::with_budget`; CLI `--max-tokens <n>` and `--max-cost <amount>` for `spec run`, `spec chat` (per line), `spec serve` (per request) and `spec mcp` (per call). `--max-cost` without `--prices` is an error.

## Consequences
- Runs can be bounded from above without changing the graph.
- Sub-agents (`agent_tools`) receive the same budget via the loader, but as their own: their consumption does not count toward the budget of the calling agent.
- Calls without usage information (moderation, multiple response variants) and models without a price do not count toward the budget until these gaps are closed.

## Addendum (2026-11-09): `spec resume`
Like `--prices`, `--max-tokens` and `--max-cost` belong to the shared `RunOptions` of the CLI and also apply to `spec resume`. Since ADR-0078, `Budget` additionally has `max_tool_calls`.
