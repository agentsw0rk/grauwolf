# grauwolf-systemone

**Layer:** `adapters` · **Status:** `SystemOneClient::new(base_url)` implements the port `Decider` (ADR-0100) for a System One service such as RKEV: `POST {base_url}/v1/systemone` with `{ state, model?, questions: { decision: { type, instructions, criteria? } } }` – a single question with the ID `decision`; `choice` has the options as an ordered object name → description or `null`, `score` the level descriptions as a list (lowest first), `noul` (yes/no) optionally `{ "true", "false" }` when the node names what yes and no mean. The state is a string or the value as JSON. `with_api_key(key)` sends `Authorization: Bearer <key>` (only then); `SystemOneClient::from_env(env)` reads `RKEV_BASE_URL` (without it: `None`) and `RKEV_API_KEY`; timeouts `CONNECT_TIMEOUT` = 10 s and `REQUEST_TIMEOUT` = 60 s (`with_request_timeout` for tests); the `Debug` output never shows the key. The response is decoded in the shape of the question: `choice` → the option with `confidence` and the probabilities in the order sent; `noul` → the probability of yes, the answer `yes`/`no` from the node's threshold; `score` → the level description for the mean index rounded and clamped to the scale, probabilities labelled with the level descriptions; `confidence`, `probabilities`, `legend`, `usage` and `latency_ms` may be missing; the answer type must fit the question. Errors: `503` and network errors (including timeouts) are `DecideError::Transient` (the facade wraps the client in `RetryingDecider`); every other status fails with `System One returned HTTP <status>: <message>` (`error.message` of `{ "error": { type, message } }`, otherwise the text). Tests with wiremock against the worked example of the RKEV README (`tests/client.rs`).

## Purpose
Decider for decision nodes with `decider: system_one`: asks a System One service instead of the language model.

## Features / ADRs
E14 F14.12, ADR-0100

## Allowed dependencies
- grauwolf crates: grauwolf-agents (the port `Decider` and the decision types)
- External: reqwest, serde, serde_json, futures-util; dev: tokio, wiremock

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- Order is part of the format: options and probabilities are written and read as ordered pairs, never through `serde_json::Map` (it sorts).
- Never logs or shows the API key.
- The System One format belongs to RKEV/AgentWeaver; changes there must be followed here.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-systemone`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
