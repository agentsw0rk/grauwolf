# ADR-0062: Prompt cache for Anthropic

- **Status:** Accepted (self-prioritized; User, 2026-11-08: "decide yourself what is important and keep going"; costs matter to the user)
- **Date:** 2026-11-08

## Context
Agents send the system prompt, the tools and the entire history so far again with every LLM call. Without a prompt cache, every round of a tool loop pays the full input price for everything sent so far. Anthropic reads cached prefixes at about a tenth of the input price (Opus 5.5: a twentieth); writing costs 1.25 times the input price (5-minute lifetime). From two requests with the same prefix onward, caching pays off. Prefixes below the minimum size (512 tokens for current models) are not cached, without an error.

## Decision
- The Anthropic adapter sets `cache_control: { type: "ephemeral" }` at the top level of every request (automatic caching): the breakpoint lies on the last block and moves along with the history.
- In addition, the last system block gets an explicit breakpoint; for this, `system` is sent as a list of text blocks. This way, conversations of different users with the same system prompt and the same tools share one cache entry (API order: tools → system → messages).
- Lifetime 5 minutes (default): agent loops send requests at shorter intervals; each hit extends the entry at no cost.
- No switch: there is no case in the project in which caching hurts, except single requests above 512 tokens without repetition (25% surcharge on that one prefix).

## Consequences
- Multi-round LLM calls and continued conversations (within 5 minutes) become significantly cheaper.
- Anything that changes the prefix (e.g., interceptors that rewrite the prompt for each request, or a changing tool list) prevents cache hits, without an error message.
- Compatible endpoints without automatic caching would reject the top-level field; then a switch would be needed (not needed so far).
- Measurable only through `usage.cache_read_input_tokens` of the API; the domain reports no token usage.

## Addendum (2026-11-09): Token usage is reported
The statement "the domain reports no token usage" is superseded by ADR-0067: adapters report `Usage` with `cache_read_tokens`/`cache_write_tokens`; cache hits appear in `llm_call_completed` and in the OpenTelemetry spans.
