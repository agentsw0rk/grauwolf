# ADR-0053: Agents as an HTTP service (`spec serve`)

- **Status:** Accepted (decision by Claude under "go on", 2026-11-08)
- **Date:** 2026-11-08

## Context
Agents have so far run only through the CLI or embedded in Rust code. Other programs (web front ends, scripts, other services) should be able to call them, with conversations across several requests and with live output. (E11)

Rejected: a source map for compile errors in the generated `lib.rs` (F13.8). The generator already rejects unmappable types and names with a JSON pointer, keywords become raw identifiers; errors in tool code are already mapped to `/tools/<i>/code` (F13.9). For valid specs hardly any compile errors remain that would need a mapping.

## Decision
- `InterpretedAgent::run_with(input, RunExtras { feature, frame_sink, conversation })`: additional runtime handles **per run**; `run_json` and `run_conversation` use it.
- New apps crate `grauwolf-server` (axum): `router(agent, history)` with
  - `GET /health` → `ok`,
  - `POST /run` with `{ "input": …, "conversation_id"?: "…" }` → `{ "output": … }` or `{ "error": "…" }` with status 500,
  - `POST /run/stream` (same body) → Server-Sent Events: `event: agent` with every `AgentEvent`, `event: text` with text deltas, finally `event: output` or `event: error`.
- The CLI starts it with `grauwolf spec serve <file> [--addr 127.0.0.1:8080]`; agent, tools, MCP servers and knowledge are loaded once at startup, conversations use `--conversations-dir`/`--conversations-db`.
- Access (addition 2026-11-08): if a token is set, `/run` and `/run/stream` require `Authorization: Bearer <token>` (constant-time comparison, otherwise `401`); `/health` stays open. The CLI reads the token from `GRAUWOLF_SERVER_TOKEN` (not as an argument, so that it does not end up in the shell history and process list) and starts on a non-local address only with a token. Without a token it binds only to `127.0.0.1` by default; TLS belongs in a reverse proxy in front of it.

## Consequences
- Specs become services without Rust code; several requests run concurrently on the same loaded agent.
- Foundation for later protocols (A2A) on the same server.

## Addendum (2026-11-09): further runtime handles and protocols
- `RunExtras` has meanwhile additionally gained `tools` (tools only for this run, ADR-0059, ADR-0074), `approver` (ADR-0061) and `memory` (automatic memory, ADR-0069). Checkpointed runs take them as well (ADR-0077).
- Besides `router(agent, history)` there is `router_with(agent, history, ServiceOptions)` for per-user tools, the tools directory and further settings.
- A2A (ADR-0054) and MCP via `/mcp` (ADR-0056) run on the same server.
