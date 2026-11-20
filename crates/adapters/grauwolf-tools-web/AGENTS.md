# grauwolf-tools-web

**Layer:** `adapters` · **Status:** `fetch_tool(FetchConfig { allowed_hosts, allow_http })`/`fetch_tool_with(config, resolver)` (`fetch { url, method?, headers?, body? }`: allow list of exact hosts and `*.domain`, https unless `allow_http`, the host is resolved (port `Resolver`, `SystemResolver`) and the connection pinned to the checked address, private/loopback/link-local/unspecified/shared addresses refused unless the host is listed exactly, redirects followed manually up to 5 with every hop re-checked, 30 s timeout, body cut off after 1 MiB, result `HTTP <code> <reason>` + `content-type`/`content-length` + body, HTML reduced to text; refusals are `ToolError::Execution`; tests against wiremock). `html_to_text(html)` (ADR-0113): readable text for knowledge pages (page furniture dropped, headings as `#` lines, list items `- `, table cells ` | `, entities decoded). `fetch_page(url, &Validators) -> Result<Page, String>` (ADR-0113, `tests/page.rs`): GET with `If-None-Match`/`If-Modified-Since`, `304` = `Page::NotModified`, `Page::Fetched { text, validators }`; redirects only within the same origin (max 5), 30 s, 5 MiB (`MAX_PAGE_BYTES`), `User-Agent: grauwolf`; text, Markdown and HTML by `Content-Type` (fallback extension), UTF-8 or Latin-1 charset; no allow list or address block (the spec's URLs are the list).

## Purpose
The `fetch` tool (E14 F14.8), split off `grauwolf-builtin-tools` in ADR-0108 (stage 3a) so that only builds that need an HTTP client pay for it.

## Features / ADRs
E14 F14.8, ADR-0098; ADR-0108

## Allowed dependencies
- grauwolf crates: grauwolf-prompt, grauwolf-spiffe, grauwolf-tools
- External: futures-util, regex, reqwest, serde_json, tokio (net, time); dev: tokio, wiremock

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- `fetch_page` follows no redirect to another origin and sends no cookies or authorization.
- A request goes only to an allow-listed host, over the address that was checked (pinned); every redirect hop is checked again; private addresses are refused unless the host is listed exactly.
- `fetch` sends the run's `Txn-Token` (`grauwolf_tools::delegation_header()`, ADR-0111) only to a host with a workload identity (`auth`), checked again on every redirect hop; never to an external host (test `the_txn_token_goes_to_an_identity_host_and_never_to_an_external_one` in `tests/spiffe.rs`).

## Tests
- `tests/fetch.rs` against a local wiremock server.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-tools-web`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.

ADR-0110 stage 1: `FetchConfig.auth: Vec<(host pattern, Arc<ClientAuth>)>` (grauwolf-spiffe): for a host that matches an entry (first match; exact or `*.domain`) every hop uses mTLS (`use_preconfigured_tls`, the server's SPIFFE ID checked) and/or a fresh JWT-SVID as `Authorization: Bearer`, and only `https://` is allowed (`refused: '<host>' needs a workload identity …`; never a fall back to plain HTTP, `allow_http` notwithstanding); a refused handshake is `TLS handshake with <host> failed: <reason>` (`tls_refusal`). Hosts without an entry get nothing, also after a redirect from an identity host; the model never sees a token or certificate. Dependency: grauwolf-spiffe (dev: `testing`, tokio-rustls). Tests: `tests/spiffe.rs`.
