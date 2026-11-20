# ADR-0054: A2A server – making grauwolf agents reachable for other agents

- **Status:** Accepted (User, 2026-11-08: "yes, do that" on token protection and A2A)
- **Date:** 2026-11-08

## Context
The Agent2Agent protocol (A2A, version 1.0, a2a-protocol.org) lets agents of different frameworks work together: an agent describes itself in an agent card and accepts messages via JSON-RPC, gRPC or HTTP+JSON; work runs as a task with status and artifacts (E09). grauwolf has had an HTTP service since ADR-0053.

## Decision
- `grauwolf-server` gets the A2A binding **JSON-RPC** (protocol version `1.0`):
  - `GET /.well-known/agent-card.json`: `name`, `description` (system prompt or default text), `supportedInterfaces` (`url` = public address + `/a2a`, `protocolBinding: "JSONRPC"`, `protocolVersion: "1.0"`), `version`, `capabilities.streaming = true`, `defaultInputModes`/`defaultOutputModes` (`text/plain`, `application/json` for named types), one skill with the name of the spec.
  - `POST /a2a` with the methods `SendMessage` (result `{ task }` with state `TASK_STATE_COMPLETED` or `TASK_STATE_FAILED`, answer as an artifact and in `history`), `SendStreamingMessage` (SSE with JSON-RPC responses: first `task` in state `TASK_STATE_WORKING`, then `artifactUpdate` per text delta with `append`, finally `statusUpdate` with the final state), `GetTask`, `CancelTask` (finished tasks: `TaskNotCancelableError -32002`); unknown tasks `-32001`, unknown methods `-32601`, unsupported `A2A-Version` `-32009`.
  - Input: text parts are joined; for named input types the first `data` part or the text counts as JSON. The `contextId` becomes the conversation (ADR-0037); without a `contextId` the service assigns one.
  - Tasks live in the memory of the service.
- The agent card stays public even with a token; `/a2a` is protected like `/run`.
- The opposite direction (remote A2A agents as tools, like MCP) follows as a separate decision.

## Consequences
- With `spec serve`, every spec is an A2A agent without further code.
- Canceling running tasks and push notifications are not supported; after a restart of the service, tasks are forgotten.

## Addendum: users and tasks with user tokens (2026-11-09, User: "first close the A2A gap")
During the documentation review it turned out that `/a2a` ignored the user of a user token (ADR-0068): all token holders shared the contexts, `GetTask`/`CancelTask` returned other users' tasks, IDs were predictable (time + counter) and tasks were never removed.
- With a user token, a request belongs to the token's user: `contextId` becomes that user's conversation via `conversation_key(Some(user), id)` (like `conversation_id` for `/run`, ADR-0060). A shared token and an unprotected service remain in the shared area.
- Every task remembers its owner; `GetTask` and `CancelTask` find only the user's own tasks, other users' tasks are reported like unknown ones with `-32001` (no hint of their existence).
- Task, context and message IDs carry 128 bits from `getrandom` (like approval IDs, ADR-0061).
- At most `MAX_STORED_TASKS` (1000) tasks remain stored; the oldest is dropped first.
