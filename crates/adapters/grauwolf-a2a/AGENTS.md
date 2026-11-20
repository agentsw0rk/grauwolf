# grauwolf-a2a

**Layer:** `adapters` · **Status:** Streaming (ADR-0089): `send_streaming(text, on_text)` via `SendStreamingMessage` (SSE; text per `artifactUpdate` goes to `on_text`, `append` appends, otherwise it replaces; final state from `statusUpdate` or `task`; `message` as a direct reply; end without a final state → `Protocol`), `streams()` from `capabilities.streaming` of the card; the tool streams when the card offers it (the read timeout then applies per event); reqwest with feature `stream`; `A2aClient::connect(url, headers)` (timeouts: connection `CONNECT_TIMEOUT` = 30 s, read `DEFAULT_READ_TIMEOUT` = 10 min without data, per client via `connect_with_read_timeout`; a read timeout reports "timeout … may have been executed", a missing connection reports "no connection … not executed"; ADR-0065) loads the agent card (`url` = card or base URL → `/.well-known/agent-card.json`) and selects the interface `protocolBinding: "JSONRPC"`; `send(text)` sends `SendMessage` (`A2A-Version: 1.0`, `ROLE_USER`, text part) and returns the text/`data` parts of the artifacts (`COMPLETED`) or of a direct message; clarification requests (`INPUT_REQUIRED`/`AUTH_REQUIRED`) become "Question from the agent: …", otherwise `A2aError::Task`; JSON-RPC errors → `A2aError::Rpc`. `Arc<A2aClient>::tool(name, description?)` is a `DynTool` with `{ request }`; every call starts without a remembered `contextId`.

Workload identity (ADR-0109): `A2aClient::connect_with_auth(url, headers, read_timeout, Some(Arc<ClientAuth>))`: mTLS through `reqwest::ClientBuilder::use_preconfigured_tls` (the URL must be `https://`, otherwise `A2aError::Http`), and/or a fresh JWT-SVID as `Authorization: Bearer` on every request (card, `SendMessage`, `SendStreamingMessage`); `ClientAuth` and `AuthMode` come from grauwolf-spiffe and are re-exported.

## Purpose
A2A client (E09, ADR-0055): remote agents as sub-agents.

## Features / ADRs
E09, ADR-0054, ADR-0055, ADR-0065

## Allowed dependencies
- grauwolf crates: grauwolf-prompt, grauwolf-spiffe, grauwolf-tools
- External: futures-util, reqwest, serde_json, thiserror; dev: tokio, wiremock

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- No third-party SDK; formats follow `a2a.proto` (ProtoJSON, enums in SCREAMING_SNAKE_CASE).
- No conversation state between calls.
- Every request of a call (`SendMessage`, `SendStreamingMessage`) carries the headers of `grauwolf_tools::current_trace_headers()` (`traceparent`, `tracestate`) – set by the run when it is traced, empty otherwise (ADR-0123). The client knows nothing of OpenTelemetry.
- With a workload identity (`connect_with_auth`, `ClientAuth`) every message also carries `grauwolf_tools::delegation_header()` (`Txn-Token`, ADR-0111) when the run acts for a person; a client without identity never sends it (test `the_txn_token_goes_only_to_an_agent_with_a_workload_identity` in `tests/auth.rs`). A renewal error fails the call as `A2aError::Http`.

## Tests
- Against wiremock (`tests/client.rs`); interplay with the crate's own A2A server in the CLI test.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-a2a`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
