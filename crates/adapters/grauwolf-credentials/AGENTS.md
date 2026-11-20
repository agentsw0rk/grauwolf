# grauwolf-credentials

**Layer:** `adapters` · **Status:** ADR-0110 section 3 (credential sources), stage 2. Reads the `[credentials]` section of `grauwolf.toml` (or the file named by `GRAUWOLF_CREDENTIALS`) and provides `CredentialEnv`, an `Env` (`grauwolf-config`) that answers the mapped variables from their source and every other variable from the environment underneath. Sources: `file` and `openbao` (alias `vault`): `OpenBaoClient` logs in with the agent's JWT-SVID (`POST /v1/auth/<mount>/login {role, jwt}`; the JWT-SVID comes from `grauwolf-spiffe`'s `WorkloadIdentity`) and reads KV v2 (`GET /v1/<mount>/data/<path>`, field after `#`). `CredentialEnv::start` resolves every mapped variable once (blocking; the start fails with an error naming variable and source) and then refreshes them in a background thread with its own runtime after `refresh` (default 5 minutes; a failed refresh keeps the old values and reports on stderr). The token is kept in memory, replaced by a fresh login (new JWT-SVID) when it is within 30 s of its lease end and once after a `403`. `CredentialEnv::check` / `check_many(base, spec file, variables, identity)` resolve variables one by one without starting the refresh thread (all mapped ones for an empty list; per variable `resolved <VAR> from <origin> (N bytes)` or its error, one OpenBao login shared; for `grauwolf credentials check`). `CredentialEnv::for_spec(base, spec file)` finds the `[credentials]` section (`GRAUWOLF_CREDENTIALS`, else `grauwolf.toml` next to the spec) and takes the identity from `SPIFFE_ENDPOINT_SOCKET`.

## Purpose
Credentials from files and from a secret broker instead of the process environment, behind the central configuration: whatever reads a variable through `Env` sees the mapped value (ADR-0108, ADR-0110).

## Features / ADRs
ADR-0108, ADR-0109, ADR-0110

## Allowed dependencies
- grauwolf crates: grauwolf-config (the `Env` port and `VARIABLES`), grauwolf-openbao (the HTTP calls, ADR-0120), grauwolf-spiffe (the `WorkloadIdentity` port and the Workload API client)
- External: serde, serde_json, toml, tokio (rt, time); dev: wiremock (the OpenBao API is mocked), grauwolf-spiffe with feature `testing`, grauwolf-test

## Invariants
- Only names from `VARIABLES` may be mapped (unknown: error); a mapped variable that is also set in the environment is an error (no silent precedence).
- A value never appears in an error, a log line, `Debug` output, an event or a span; errors name the variable and the source only.
- A source that cannot be read at start fails the start; nothing falls back to the environment.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- Tests use `grauwolf_test::TestDir` for temporary files; OpenBao is mocked with wiremock (`tests/openbao.rs`), the identity is `FakeIdentity`.
- `cargo test -p grauwolf-credentials`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.

`OpenBaoClient` implements `grauwolf_openbao::TokenSource` (its logged-in token, renewed as for reads), so the identity service signs with Transit through the same login (ADR-0111).
