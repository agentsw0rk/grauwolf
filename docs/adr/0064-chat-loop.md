# ADR-0064: Chat as a dialog loop instead of a strategy

- **Status:** Accepted (self-prioritized; User, 2026-11-08: "decide yourself what is important and keep going")
- **Date:** 2026-11-08

## Context
The feature catalog lists a "chat strategy" (F05.12). Conceivable would be a graph in which the model talks to the user only via tools: text replies are rejected with "call a tool", and the conversation ends via an exit tool. But normal text replies work better. We deliberately did not adopt an exit tool in ADR-0046, and conversations across several LLM calls already exist (ADR-0037: `--conversation`, `conversation_id`).

## Decision
- No dedicated graph strategy. Chatting is a loop around ordinary runs: `grauwolf spec chat <file>` reads lines from stdin and executes each one as a run in the same conversation (`run_with` with a conversation). The response goes to stdout, the prompt `> ` to stderr.
- Ends at EOF or `/exit`; empty lines are skipped. A failed run reports the error on stderr and does not end the chat.
- Without `--conversation`, the conversation lives only in memory; with `--conversation <id>` (and `--conversations-dir`/`--conversations-db`) it is stored and can be continued later, also via `spec run` or the HTTP service.
- The options of `spec run` apply (`--events`, `--confirm-tools`, conversation storage, tool builds), except `--stream` and `--otel`, which are rejected for now; tool questions (`ask_user`, `--confirm-tools`) read from the same stdin.

## Consequences
- Every spec and every strategy can be chatted with, without the graph changing.
- A chat that is ended by the model itself does not exist; the user determines the end.
