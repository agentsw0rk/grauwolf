# ADR-0071: Costs per LLM call and run

- **Status:** Accepted (User, 2026-11-09: "then on with costs in money"; planned since "record the costs in money")
- **Date:** 2026-11-09

## Context
Since ADR-0067, every call reports its token usage and the run reports their sum. What that costs had to be calculated by hand. Prices differ per model and for cached tokens, and they change over time; built-in prices would go stale unnoticed.

## Decision
- Price table as configuration, not in code: JSON `{ "currency": "USD", "models": { "<provider>:<id>": { "input", "output", "cache_read"?, "cache_write"? } } }`, prices per million tokens; if a cache price is missing, the input price applies.
- `grauwolf_prompt::Prices::cost(model, usage)`: uncached input (`input_tokens − cache_read − cache_write`), cache reads, cache writes and output, each at its price. Without a price for the model or without usage: no amount (not estimated).
- `AgentContext::with_prices` / `Interpreter::with_prices`: `llm_call_completed.cost` per call, `AgentContext::cost()` as sum including forks, `agent_completed.cost`/`agent_failed.cost`; fact requests of automatic memory count too. The currency appears only in the table.
- OpenTelemetry: `grauwolf.cost` on the chat and agent span (there is no GenAI convention for it).
- CLI: `--prices <file>` for `spec run`, `spec chat` and `spec serve`.

## Consequences
- Costs are visible without recalculation, as long as the table is correct; maintaining it remains the operator's task.
- Models without an entry appear without an amount, so that no false sense of security arises.
- Moderation and multiple response variants do not yet report usage, and therefore no costs either.

## Addendum (2026-11-09): `--prices` for all run commands
`--prices` belongs to the shared `RunOptions` of the CLI and therefore also applies to `spec mcp` and `spec resume`, not only to `spec run`, `spec chat` and `spec serve`.
