# grauwolf-mcp

**Layer:** `adapters` · **Status:** `McpServer::name()`, `with_tools(tools)` (tools per request, ADR-0089); `McpClient::connect(reader, writer)` (handshake `initialize` + `notifications/initialized`, protocol version `PROTOCOL_VERSION`), `server_name()`, `list_tools()` (with `nextCursor`) → `ToolDescriptor`, `call_tool(name, args)` → text contents, `isError` → `McpError::Tool`; JSON-RPC errors → `McpError::Rpc`; one request at a time, foreign messages are skipped. `Arc<McpClient>::tools()` returns every tool as a `DynTool` (for `ToolRegistry::builder().shared(..)`; errors → `ToolError::Execution`). `McpClient::spawn(command, args)` starts the server as a process (stdio, `kill_on_drop`, stderr visible). Spec integration via `mcp_servers` (validation in grauwolf-spec, start in the facade `grauwolf::mcp`, interpreter `with_tools`). `McpClient::connect_http(url, headers)` speaks Streamable HTTP (ADR-0051): `POST` per message with `Accept: application/json, text/event-stream`, `Mcp-Session-Id` from the `initialize` response and `MCP-Protocol-Version` afterwards in every request; responses as JSON or SSE (the matching ID wins); HTTP errors → `McpError::Http`. `McpClient::connect_sse(url, headers)` speaks the older HTTP+SSE transport of protocol 2024-11-05 (ADR-0112, `src/sse.rs`): `GET url` (`Accept: text/event-stream`) opens the stream, the first event `endpoint` names the URI for `POST`s (resolved against `url`; another scheme, host or port than `url` → `McpError::Http` "another origin", nothing is sent; waiting for it at most `CONNECT_TIMEOUT`), any 2xx to a `POST` is accepted (other statuses are named without the body), answers arrive as `message` events and are matched by `id` (notifications and unknown IDs are ignored; requests of the server get a reply: `ping` → `{}`, everything else → -32601 "method not found"); SSE parsing: `event:`, `data:` (joined with `\n`), blank line ends an event, `\r\n`, comments, `id:`/`retry:` ignored; the read timeout applies per call (`connect_sse_with_read_timeout`; `McpError::Timeout`), not to the idle stream. Stream loss: pending calls fail ("event stream … may have been executed"), the next call reconnects once (new `GET`, new endpoint, new `initialize`); if that fails it returns the error; dropping the client closes the stream. Timeouts: connection `CONNECT_TIMEOUT` = 30 s, read `DEFAULT_READ_TIMEOUT` = 10 min without data, per client via `connect_http_with_read_timeout` (ADR-0065); stdio: `DEFAULT_READ_TIMEOUT` per line from the server (notifications reset it), `connect_with_read_timeout(reader, writer, read_timeout)`; exceeding it → `McpError::Timeout` ("… may have been executed"); a read timeout reports "timeout … may have been executed", a missing connection reports "no connection … not executed". `list_resources()` → `Resource { uri, name, description, mime_type }`, `read_resource(uri)` (text; binary contents as a note with length), `list_prompts()` → `PromptTemplate { name, description, arguments }`, `get_prompt(name, arguments)` → messages (text contents); all lists follow `nextCursor`. `Arc<McpClient>::resource_tools(prefix)` offers resources as tools `<prefix>_list_resources` and `<prefix>_read_resource { uri }`. Server side (ADR-0056): `McpServer::new(name, tools)` with `handle(message)` (initialize with version negotiation, ping, tools/list, tools/call with `isError`, -32601) and `serve(reader, writer)` for stdio. Server with resources and prompts (ADR-0089): `with_resources(Vec<McpResource>)` (`resources/list`, `resources/read`; unknown → -32002), `with_prompts(Vec<McpPrompt>)` (`prompts/list`; `prompts/get` fills in `{argument}` and returns a user message; unknown prompt or missing required argument → -32602); `initialize` lists `resources`/`prompts` only if there are any.

Workload identity (ADR-0109): `connect_http_with_auth` and `connect_sse_with_auth` take `Option<Arc<ClientAuth>>` (grauwolf-spiffe): mTLS through `use_preconfigured_tls` (the URL must be `https://`) and/or a fresh JWT-SVID as bearer on every request, also on the SSE `GET` and `POST`s.

## Purpose
MCP client (E09 F09.1, F09.2): use tools from MCP servers over stdio, Streamable HTTP (ADR-0051) or HTTP+SSE (ADR-0112) as grauwolf tools; MCP server (ADR-0056): offer grauwolf tools or agents via MCP.

## Features / ADRs
E09 F09.1–F09.2, F14.20, ADR-0036, ADR-0051, ADR-0056, ADR-0065, ADR-0089, ADR-0112

## Allowed dependencies
- grauwolf crates: grauwolf-prompt, grauwolf-spiffe, grauwolf-tools
- External: futures-util, reqwest, tokio (io-util, process, sync), serde_json, thiserror; optional (feature `test-sse-server`): axum; dev: tokio, wiremock

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- No third-party SDK for the protocol (ADR-0036); protocol changes are followed here.
- MCP tools run outside the WASM sandbox, with the privileges of the server process.
- The `POST` requests of Streamable HTTP and HTTP+SSE carry the headers of `grauwolf_tools::current_trace_headers()` (`traceparent`, `tracestate`) – set by the run for a traced tool call, empty otherwise (ADR-0123). The handshake and the tool listing happen outside a run and carry none; stdio has no headers.
- With a workload identity (`connect_http_with_auth`, `connect_sse_with_auth`) the same `POST` requests also carry `grauwolf_tools::delegation_header()` (`Txn-Token`, ADR-0111) when the run acts for a person; without identity never (`tests/delegation.rs`, needs `test-sse-server`; `SseTestServer::post_header(name)` reports a header of every message).

## Tests
- Against a fake server over `tokio::io::duplex` (`tests/client.rs`).
- stdio against `tests/fixtures/fake_mcp_server.py` (skipped without `python3`).
- Streamable HTTP against wiremock (`tests/http.rs`).
- HTTP+SSE against the in-memory server `grauwolf_mcp::testing::SseTestServer` (feature `test-sse-server`, `src/testing.rs`; `SseBehavior` switches CRLF, multi-line data, a foreign endpoint, required headers, `POST` statuses; `tests/sse.rs`); the example `serve_sse_mcp [port]` serves it with the single tool `reverse` for live checks (`cargo run -p grauwolf-mcp --features test-sse-server --example serve_sse_mcp`).

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-mcp`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.

ADR-0110 stage 1: `McpClient::spawn_with_env(command, args, env)`; `spawn` never passes the agent's `SPIFFE_ENDPOINT_SOCKET`/`SPIFFE_FETCH_TIMEOUT` on to the child (they are removed from the inherited environment), only `env` can set them (`mcp_servers.<n>.workload_api: true` in the host). Test: `tests/stdio.rs` with `tests/fixtures/env_mcp_server.py`.
