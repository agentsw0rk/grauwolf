# ADR-0112: MCP over the HTTP+SSE transport

- **Status:** Accepted (F14.20; User, 2026-11-13: "continue with F14.20 MCP over SSE")
- **Date:** 2026-11-13

## Context
The MCP client speaks stdio (ADR-0036) and Streamable HTTP (ADR-0051, protocol 2025-06-18).
Streamable HTTP replaced the older **HTTP+SSE transport of protocol version 2024-11-05**, but
servers that only speak the old transport are still in use, and AgentWeaver's `mcp-server` can be
configured with the transport `sse`. To attach such servers (F14.20), grauwolf needs a client for
the old transport; a server side is not needed.

## Decision
- **Spec:** `mcp_servers.<name>.transport` is optional: `"streamable_http"` (default when `url` is
  set) or `"sse"`. It only works together with `url`; with `command` it is a validation error
  (`McpTransportWithoutUrl`, pointer `/mcp_servers/<name>/transport`, next to `McpTransport`).
  There is **no automatic fallback** between the two transports: one explicit choice is
  predictable, and AgentWeaver knows the type of its server. AgentWeaver's `mcp-server` with
  transport `sse` maps to `transport: "sse"`.
- **Protocol (2024-11-05):**
  - `GET <url>` with `Accept: text/event-stream` and the configured `headers` opens the stream.
  - The first event `endpoint` carries the URI for client messages, resolved relative to `url`. It
    must have the **same scheme, host and port as `url`**; otherwise the client refuses and
    sends nothing (headers such as tokens must not go to another origin, the same rule as for OCI
    uploads, ADR-0107). Waiting for it is limited (`CONNECT_TIMEOUT`).
  - Client messages go by `POST <endpoint>` with `Content-Type: application/json` and the same
    headers; any 2xx is accepted (servers answer `202`), other statuses are an error that names
    the status but not the response body.
  - Server messages arrive as `message` events: responses are matched to pending requests by
    `id` (unknown ids are ignored); notifications are ignored; requests of the server get a
    JSON-RPC reply (`ping` an empty result, everything else "method not found" -32601), so a
    server never waits for us.
  - SSE parsing: `event:`, `data:` (several lines are joined with `\n`), `id:`, comments (`:`),
    a blank line ends an event, `\r\n` is tolerated, `retry:` is ignored.
  - Handshake: `initialize` with `PROTOCOL_VERSION`, then `notifications/initialized`; the version
    the server answers is not checked (as for Streamable HTTP; `2024-11-05` works).
  - **Stream loss:** the old transport cannot resume. If the stream ends or fails, pending
    requests fail with an error that says the stream was lost (a call that was already sent may
    have been executed); the **next call reconnects once** (new `GET`, new endpoint, new
    handshake) and fails with that error if the reconnect does not work. Dropping the client closes
    the stream.
  - Timeouts as for Streamable HTTP (ADR-0065): connection `CONNECT_TIMEOUT`, per request
    `DEFAULT_READ_TIMEOUT` (`connect_sse_with_read_timeout`). The idle event stream itself is not
    subject to the read timeout.
- **Code:** in `grauwolf-mcp` next to the Streamable HTTP client as a third `Transport`
  (`McpClient::connect_sse(url, headers)`); handshake and all MCP methods are shared, plain
  `reqwest` without a new dependency. Host, interpreter, generated code, `mcp_tools_observer` and
  `resources: true` work unchanged through the client API.
- **Tests without network:** a test server for the 2024-11-05 transport
  (`grauwolf_mcp::testing::SseTestServer`, feature `test-sse-server`, axum) and the example
  `serve_sse_mcp` that serves a small SSE MCP server (tool `reverse`) for live checks.

## Considered
- **Automatic fallback** as in the MCP specification's backwards-compatibility advice (`POST`
  `initialize`; on `400`/`404`/`405` open the stream with `GET`): rejected. It costs a failed
  request per start, hides a wrong configuration behind a second transport, and makes errors
  harder to read; the author of the spec knows which server it uses.
- **Following redirects of the endpoint to other origins:** rejected (credential leak).

## Consequences
- Servers that only speak HTTP+SSE can be used; the old transport is a deprecated one, so no
  server side is offered.
- The reconnect after a lost stream is lazy: the failure of the call during which the stream broke
  is visible, only the following call recovers.
- Server-initiated requests (sampling, roots) stay unsupported on all transports.

## Live check (2026-11-13)
`cargo run -p grauwolf-mcp --features test-sse-server --example serve_sse_mcp` (127.0.0.1:8931)
together with `docs/user-guide/examples/08-mcp-sse.json` (`transport: "sse"`, tool `reverse`),
model `qwen3.5:9b` on Ollama (`OLLAMA_HOST=http://host.docker.internal:11434`,
`OLLAMA_THINK=false`): `grauwolf spec run … "Reverse the text 'grauwolf'."` – the model called
`reverse` with `{"text":"grauwolf"}` (events `tool_call_starting`, `tool_call_completed` with
`flowuarg`) and answered `flowuarg`. The server was a local test server, not a third-party one;
a real HTTP+SSE server has not been tried.
