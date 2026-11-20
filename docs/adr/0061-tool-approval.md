# ADR-0061: Asynchronous approval of tool calls

- **Status:** Accepted (self-prioritized; User, 2026-11-08: "decide yourself what is important and keep going")
- **Date:** 2026-11-08

## Context
Interceptors (ADR-0047) run synchronously. The CLI asks for confirmation with `--confirm-tools` by blocking on stdin; `spec serve` offers no approval and runs every tool immediately, including `write_file` or `edit_file`. A service that changes files or spends money needs a person who can agree over the network while the run waits.

## Decision
- `Interceptor` gets the asynchronous hook `approve_tool_call(&ToolCall) -> BoxFuture<ToolDecision>` (default: `Proceed`). `execute_tool` calls it after the synchronous `before_tool_call` hooks; a rejection acts like `Deny` (the text becomes the tool result).
- `Interpreter::with_tool_approval(tools)` makes tools require approval: every run of the agent, including via `into_tool` (MCP, sub-agent), rejects them, unless the run has an approver in `RunExtras.approver` (`ToolApprover`, asynchronous). So "reject" stays the default on every path for which the caller does not explicitly provide a response channel.
- HTTP service with approval-requiring tools (`--confirm-tools a,b` with `spec serve`):
  - `/run/stream` sends the SSE event `approval` with `{ id, tool, args }` before such a call and waits;
  - `POST /approvals/{id}` with `{ approved, reason?, user_id? }` decides; the `user_id` must match that of the run, otherwise `404` (like an unknown ID);
  - without a response within the time limit (default 5 minutes) the call is rejected;
  - paths without a response channel (`/run`, A2A, MCP) reject approval-requiring tools and tell the LLM why.
- Approval IDs are random (128 bits) and are valid only while the run is waiting.

## Consequences
- Existing interceptors remain unchanged; the new hook is optional.
- Clients that want to use approval-requiring tools must use `/run/stream`.
- If the client aborts the stream, the run waits until the time limit and then rejects.
- Sub-agents (`agent_tools`) and checkpoint runs have no approver; they always reject approval-requiring tools.
- Open approvals are held in memory; several service instances behind a load balancer need sticky sessions.

## Addendum (2026-11-09): Checkpoint runs with an approver
The consequence "checkpoint runs have no approver" is superseded by ADR-0077: `run_checkpointed_with` and `resume_with` take `RunExtras`, including `approver`; the CLI passes the console approver to `spec run --run-id` and `spec resume`. Without an approver, checkpoint runs continue to reject approval-requiring tools.
