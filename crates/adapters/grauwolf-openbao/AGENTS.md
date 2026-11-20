# grauwolf-openbao

**Layer:** `adapters` · **Status:** ADR-0120 decision 3. The HTTP side of OpenBao (and HashiCorp Vault) that `grauwolf-credentials` (agents) and `grauwolf-k8-ops` (operators) share.

## Purpose
One place for the OpenBao API calls: JWT login and KV v2 read (what an agent needs, ADR-0110), plus the admin calls of the setup (auth methods, JWT configuration, KV mounts, ACL policies, JWT roles, KV write, read of a whole secret (`kv_read`), list, metadata and delete, `token_policies` = `auth/token/lookup-self`, `create_token` = `auth/token/create` with only the given policy, `no_parent` first and a child token if that answers 403; Transit administration for the identity service of ADR-0111: `ensure_transit`, `transit_key_type`, `ensure_transit_key` (created if missing, never replaced), `delete_transit_key` (sets `deletion_allowed` first)). `Bao<T: Transport>` holds the calls; a `Transport` sends one request (`method`, path below the server address, optional token and JSON body) and returns status and body. `HttpTransport` talks to an address with reqwest; the operator program supplies a transport that goes through the Kubernetes API service proxy.

## Features / ADRs
ADR-0110, ADR-0120

## Allowed dependencies
- grauwolf crates: none
- External: base64 (Transit input and signatures, PEM), reqwest, serde_json, tokio (time); dev: wiremock

## Invariants
- A value or token never appears in an error; errors carry the HTTP status and the server's `errors` messages only (`BaoError`).
- The calls change nothing by themselves beyond what their name says; idempotency (read, compare, write) is the caller's job and is supported by the `read_*` calls.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- Tests mock OpenBao with wiremock (`tests/admin.rs`).
- `cargo test -p grauwolf-openbao`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.

Transit (ADR-0111, the identity service's signing key): `transit_key(token, mount, key)` (`GET /v1/<mount>/keys/<key>`; only type `ecdsa-p256`; `TransitKey { latest_version, public_keys: [(version, 04||x||y)] }` from the PEM SPKI of every version) and `transit_sign(token, mount, key, version, input)` (`POST /v1/<mount>/sign/<key>` with `hash_algorithm: sha2-256`, `marshaling_algorithm: jws`, `key_version`; returns the raw 64-byte `r || s` from `vault:v<N>:<base64url>`). They take the token per call; `TokenSource` (`token()`) is the port a caller logs in through, `StaticToken` a fixed one. Tests: `tests/transit.rs`.
