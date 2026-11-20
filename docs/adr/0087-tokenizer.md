# ADR-0087: Counting tokens before the request

- **Status:** Accepted (User, 2026-11-09: "do 11. tokenizer (F06.13): count tokens before the request, e.g. for compression or limits")
- **Date:** 2026-11-09

## Context
grauwolf so far knows token counts only after a request (ADR-0067). History can be compressed only by the number of messages (`on_history_longer`), and the budget applies only once the tokens are already consumed. How large a request will be is not visible beforehand.

## Decision
- **Port** `Tokenizer` in `grauwolf-prompt`: `count(text) -> u64`; `count_prompt(tokenizer, prompt)` counts all messages (text, tool calls as JSON, tool results) plus a fixed surcharge per message (4) and per attachment (1000).
- **Built in:** `EstimatingTokenizer`: one token per started 4 characters. An estimate without dependencies, precise enough for decisions of the kind "roughly this large"; exact tokenizers (e.g., for OpenAI) can be plugged in via the same port.
- **Context:** `AgentContext::with_tokenizer(..)` (default: estimate) and `prompt_tokens()`.
- **Compression:** condition `on_history_tokens_above { tokens }`: passes the value on when the history has more tokens; in the catalog, in the interpreter and in the generator.
- **Limit:** `limits.max_prompt_tokens` (≥ 1), or `Budget.max_prompt_tokens`: before each LLM request, the history is counted; if it is larger, the run ends with `BudgetExceeded`, without sending the request.

## Consequences
- Compression by size instead of by count becomes possible; long tool results now count as they should.
- The estimate can be off by about a quarter, depending on language and model; therefore set limits with a margin to the real context window.
