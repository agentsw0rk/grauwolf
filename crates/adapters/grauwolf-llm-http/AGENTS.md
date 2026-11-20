# grauwolf-llm-http

**Layer:** `adapters` · **Status:** `HttpClient::get_json(path)` (GET, JSON answer; ADR-0094); `HttpClient::new(base_url)` (timeouts: connection `CONNECT_TIMEOUT` = 30 s, read `DEFAULT_READ_TIMEOUT` = 10 min without received data, per client via `with_read_timeout`; no overall timeout, so that streams and long thinking phases can run; ADR-0065) with `post_json(path, &body)` (JSON out and back; `HttpError::Transport`, `HttpError::Status` with the response body and `retry_after` from the `retry-after` header); `From<HttpError> for LlmError` classifies 408/409/429/5xx/529, connection errors, and timeouts as `LlmError::Transient`, otherwise `LlmError::Client`; headers via `with_bearer_token`/`with_header`; `with_signer(Signer)` adds headers computed from the finished POST request (method, URL, serialized body; e.g. AWS SigV4, ADR-0105; signer output is never logged), `post_chunks` returns the raw body in chunks (binary streams); streaming (ADR-0033): `post_lines` (lines without blank lines, NDJSON) and `post_sse` (`SseEvent { event, data }`, multi-line data, comments dropped), error status before the first element; tests against wiremock

## Purpose
Shared HTTP/SSE base for providers: reqwest client, SSE parsing, error mapping, headers/auth.

## Features / ADRs
E02, ADR-0033, ADR-0065, ADR-0094

## Allowed dependencies
- grauwolf crates: grauwolf-llm (error classification)
- External: reqwest (standard TLS rustls, feature `stream`), futures-util, serde, serde_json, thiserror; dev: tokio, wiremock

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- No provider-specific DTOs.
- Header values (API keys) never appear in `Debug` output or error messages.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-llm-http`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
