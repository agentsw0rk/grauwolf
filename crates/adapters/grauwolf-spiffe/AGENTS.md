# grauwolf-spiffe

**Layer:** `adapters` · **Status:** ADR-0109 stages 1 and 2 (identity part). Port traits: `WorkloadIdentity` (`jwt_svid(audience) -> JwtSvid`, `validate_jwt_svid(token, audience) -> SpiffeId` through the Workload API's `ValidateJWTSVID`, so grauwolf has no JWT crypto; a rejected token gives `SpiffeError::is_invalid_token()`) and `TlsIdentity` (`id()`, `client_tls(&SpiffeMatch) -> rustls::ClientConfig`, `server_tls(&[accepted trust domains]) -> rustls::ServerConfig` that requires a client certificate); `Identity` is both. `WorkloadApiClient` is the stateless JWT client (own minimal gRPC over `h2`, messages in `src/proto.rs` as in `proto/workload.proto`); `SpiffeWorkload::connect(client)` also starts `X509Source` (`src/source.rs`): the streams `FetchX509SVID` and `FetchX509Bundles` feed the latest SVID and the bundle per trust domain, reconnecting with the doubling pause; `connect` waits for the first SVID and bundle until the fetch deadline (`SpiffeError::GaveUp`). `src/tls.rs`: `ResolvesServerCert`/`ResolvesClientCert` read the source at every handshake (rotation without restart); custom verifiers check the chain with `webpki` against the bundle of the certificate's trust domain (not the web PKI, no host name), then the SPIFFE ID (client side: `SpiffeMatch` = `Id`/`Ids`/`TrustDomain`; server side: the trust domain is in the accepted list). TLS 1.3 only. The SPIFFE ID is the single URI SAN with scheme `spiffe` (`spiffe_id_of_certificate`, `src/der.rs` is a minimal DER reader). Feature `testing`: `TestCa` (issues SVIDs for given IDs at test time, P-256, DER by hand, signed with aws-lc-rs), `FakeIdentity` (in-process port with `rotate()`, `expire()`, `trust(ca)`) and `FakeWorkloadApi` (Unix socket server: JWT, `ValidateJWTSVID`, X.509 streams, `rotate()`, `add_bundle()`, `disconnect_streams()`); test tokens carry a placeholder signature that only the fakes accept.

`WorkloadApiClient::jwt_svid` waits for an identity that is not there yet: `PERMISSION_DENIED` ("no identity issued" before the registration entry is synced), `UNAVAILABLE` and an unreachable or missing socket are retried with a pause that starts at 200 ms and doubles to 5 s, until a deadline (default 30 s; `SPIFFE_FETCH_TIMEOUT` such as `30s`, `0s` tries once; `with_fetch_timeout`). Every other error returns at once; at the deadline the error says how long it waited (`SpiffeError::GaveUp`). `FakeWorkloadApi::start_denying` answers the first calls with `PERMISSION_DENIED`.

## Purpose
The agent's workload identity from SPIRE (or any Workload API implementation) behind a port, so credential sources and later mTLS never see the transport.

## Features / ADRs
ADR-0109, ADR-0110

## Allowed dependencies
- grauwolf crates: grauwolf-config (`Env`, to read `SPIFFE_ENDPOINT_SOCKET`)
- External: h2, http, bytes, prost (derive only, no `protoc`), rustls and rustls-webpki (both already in the lockfile through reqwest; aws-lc-rs provider), tokio (net, io-util, rt, time, sync), futures-util, thiserror; feature `testing`: base64, serde_json, aws-lc-rs (signs the test certificates; no `rcgen`); dev: tokio-rustls

## Invariants
- No `protoc` and no code generation at build time (offline builder image, ADR-0096): the protobuf messages are written by hand next to `proto/workload.proto`.
- A token never appears in `Debug` output or in an error; errors name the socket and the reason only.
- Nothing happens without `SPIFFE_ENDPOINT_SOCKET`: `WorkloadApiClient::from_env` returns `None`.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- Tests use `grauwolf_test::TestDir` for socket paths.
- `cargo test -p grauwolf-spiffe`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.

`tls_refusal(error)` (ADR-0109 live check): walks an error chain (also inside `std::io::Error`) for a refused rustls handshake and returns the reason of the verifiers ("the server is …, expected …") or rustls' message; the A2A and MCP clients use it to report `TLS handshake with <url> failed: …` instead of a missing connection.
