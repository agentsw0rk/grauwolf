# ADR-0109: Workload identity with SPIFFE/SPIRE

- **Status:** Accepted (User, 2026-11-12: "native, with mTLS for agent ↔ agent and MCP, plus JWT-SVID
  as a second option; map SPIFFE IDs to users; keep user tokens for people; first an ADR, then the
  implementation in stages: outbound for A2A and MCP first, then inbound with authorization, then
  the AgentWeaver manifests"). Implemented in stages 1–3 (see "Stages").
- **Date:** 2026-11-12

## Context
Agents authenticate today with static secrets:
- **Inbound** (`spec serve`, generated services, A2A server): the shared `GRAUWOLF_SERVER_TOKEN`
  (ADR-0053) or user tokens from a users file (ADR-0068, `gw_…`, stored as SHA-256). TLS is left
  to a reverse proxy; the service itself speaks plain HTTP.
- **Outbound** to remote sub-agents (A2A, `agent_tools.<name>.url`, ADR-0055) and MCP servers over
  HTTP (`mcp_servers.<name>.url`): `headers` with `${VAR}` from the environment, e.g. a bearer token
  per target.

Once AgentWeaver deploys many agents that call each other and shared MCP servers, this means one
secret per edge, distributed through the environment, rotated by hand, and no reliable statement
about *which workload* is calling. SPIFFE solves exactly this: every workload gets an identity
(`spiffe://<trust domain>/<path>`) as a short-lived, automatically rotated X.509 certificate
(X.509-SVID) or JWT (JWT-SVID), issued by SPIRE after attesting the workload (node, namespace,
service account), fetched through the Workload API on a local socket. No secret has to be handed
to the agent.

## Decision
### Scope
- SPIFFE covers **agent ↔ agent (A2A)**, **agent → MCP servers over HTTP** and **callers → agent
  service**. Model providers and external SaaS (OpenAI, Anthropic, Jira, git hosts, registries)
  keep their API keys and tokens: they do not accept SVIDs. A model gateway inside the trust
  domain could later accept SVIDs; that is not part of this ADR.
- User tokens (ADR-0068) and `GRAUWOLF_SERVER_TOKEN` stay for people and for clients without a
  workload identity (CLI, AgentWeaver UI). SPIFFE is an additional way to authenticate, not a
  replacement.

### Native integration
- **New adapter crate `grauwolf-spiffe`** behind the Cargo feature `spiffe` of host and facade (in
  `default`, so the builder image's pre-built dependencies cover it). It reads the Workload API at
  `SPIFFE_ENDPOINT_SOCKET` and keeps a **live source** of the own X.509-SVID, the trust bundles and
  JWT-SVIDs: rotation needs no restart. Planned dependencies: the `spiffe` crate (Workload API
  client) and `spiffe-rustls` (rustls client/server configs from the live source, authorization
  after cryptographic verification). Stage 1 checks their versions against our `rustls` 0.23 and
  `reqwest` 0.13 and the weight of the gRPC client they pull in; if that is unacceptable, the
  crate gets its own minimal Workload API client. The rest of grauwolf only sees a port.
- **Port** (in the host, not the domain – the domain never does network or TLS):
  `WorkloadIdentity` with `client_tls(expected: &SpiffeMatch) -> rustls::ClientConfig`,
  `server_tls() -> rustls::ServerConfig`, `jwt_svid(audience) -> String` and
  `validate_jwt_svid(token, audience) -> SpiffeId`. Tests implement it with certificates made at
  test time (no SPIRE needed).
- **Trust domains:** `GRAUWOLF_SPIFFE_TRUST_DOMAINS` (comma-separated) limits which trust domains
  are accepted (default: the own trust domain only); federated bundles from the Workload API are
  used only for listed domains.
- Without `SPIFFE_ENDPOINT_SOCKET`, everything works as today; a spec or service that *requires*
  SPIFFE fails at start-up with a clear error naming the variable (no silent fallback to
  unauthenticated calls).

### Outbound (stage 1): A2A and MCP
- **Spec:** `agent_tools.<name>` and `mcp_servers.<name>` (HTTP transport) get an optional `auth`:
  ```json
  "agent_tools": {
    "reviewer": {
      "url": "https://reviewer.reviewer.svc:8443",
      "auth": { "spiffe": { "id": "spiffe://agentswork.de/ns/reviewer/sa/reviewer" } }
    }
  },
  "mcp_servers": {
    "tickets": {
      "url": "https://tickets-mcp.tools.svc/mcp",
      "tools": ["search"],
      "auth": { "spiffe": { "mode": "jwt", "audience": "tickets-mcp" } }
    }
  }
  ```
  - `mode: "mtls"` (default): the client presents its X.509-SVID and accepts the server only if its
    SPIFFE ID matches `id` (exact), `ids` (one of a list) or `trust_domain` (any ID of that
    domain; at least one of the three is required – "any SPIFFE ID" is not offered). The server is
    verified against the SPIFFE bundle, not the web PKI.
  - `mode: "jwt"`: the client fetches a JWT-SVID for `audience` (required) and sends
    `Authorization: Bearer <JWT-SVID>`; the TLS connection is verified as usual (web PKI) unless
    `id`/`ids`/`trust_domain` is given, then against the SPIFFE bundle as for mTLS. For targets
    behind a TLS-terminating ingress.
  - `auth` and an `Authorization` entry in `headers` are mutually exclusive (validation error);
    other headers stay allowed. `auth` with a stdio MCP server is a validation error.
- **Remote A2A sub-agents** inside a flow (`agent` node, ADR-0101) use the same `auth` of their
  `agent_tools` entry.
- **No delegation in this ADR:** an agent calls with **its own** identity, also when it acts for a
  user. The user is passed as today (A2A metadata / request body); passing the caller's identity
  on (token exchange, "on behalf of") is an open point.

### Inbound (stage 2): authentication and authorization
- **TLS in the service:** with `GRAUWOLF_SPIFFE=1` (`spec serve --spiffe`, generated services
  through the environment) the service serves HTTPS itself with its X.509-SVID (main port, A2A
  port). Client certificates are **requested but optional** at the TLS level, so people with user
  tokens (through an ingress) and workloads with SVIDs can use the same port.
- **Who is calling**, in this order:
  1. a valid client X.509-SVID of an accepted trust domain → its SPIFFE ID;
  2. `Authorization: Bearer` with a JWT-SVID (recognized as a JWT, i.e. not starting with `gw_`)
     whose signature checks against the bundle and whose `aud` contains the service's audience
     (`GRAUWOLF_SPIFFE_AUDIENCE`, default: the service's own SPIFFE ID) → its SPIFFE ID;
  3. otherwise the existing rules: user token (ADR-0068), shared token (ADR-0053), anonymous only
     with `GRAUWOLF_ALLOW_ANONYMOUS`.
- **Mapping to users** (authorization): a mapping file (`--spiffe-users <file>`,
  `GRAUWOLF_SPIFFE_USERS`) assigns SPIFFE IDs to grauwolf users:
  ```json
  {
    "spiffe://agentswork.de/ns/coordinator/sa/coordinator": "coordinator",
    "spiffe://agentswork.de/ns/batch/*": "batch-jobs"
  }
  ```
  Exact IDs or a path prefix ending in `/*` (matching whole segments only); the most specific entry
  wins. An authenticated SPIFFE ID **without** an entry gets `403` – authenticated is not
  authorized. The mapped user is the request's user exactly as with a user token: conversations,
  long-term memory, approvals (`POST /approvals/{id}` only by the same user) and the per-user tool
  stores stay separated (ADR-0059, 0060, 0061, 0098). A `user_id` in the body that differs from the
  mapped user is `403`, as for user tokens.
- **Observability:** the caller's SPIFFE ID (never a JWT) is recorded on the request span and in
  the `agent_starting` event as `caller`; the mapped user as today.

### AgentWeaver manifests (stage 3, AgentWeaver repository)
For every harness whose spec uses `auth.spiffe` or whose service is called by other agents:
a dedicated service account per agent; the SPIFFE CSI driver volume with
`SPIFFE_ENDPOINT_SOCKET`; a `ClusterSPIFFEID` (spire-controller-manager) with the template
`spiffe://<trust domain>/ns/<namespace>/sa/<service account>`; `GRAUWOLF_SPIFFE=1`; the mapping
file generated from the flow graph (which agent calls which), and the `auth` blocks of
`agent_tools`/`mcp_servers` filled with the target's SPIFFE ID. The NetworkPolicy rules of ADR-0099
stay. grauwolf documents the expected manifest shape; AgentWeaver generates it.

## Stages
1. **Outbound:** crate `grauwolf-spiffe` (live source, port, test identity), spec `auth` with
   validation, A2A client and MCP HTTP client with mTLS and JWT-SVID, interpreter and generated
   code alike, docs.
2. **Inbound:** HTTPS with the own SVID, JWT-SVID validation, mapping file and authorization in
   `spec serve`, generated services and the A2A server, caller in events and spans, docs.
3. **AgentWeaver:** manifest generation as above (needs AgentWeaver's own repository) and a
   cluster check against a real SPIRE.

## Consequences
- No static secrets between agents and towards internal MCP servers; identities rotate on their
  own, and a service knows which workload is calling and maps it to a user with the same
  separation as for people.
- The service can terminate TLS itself (needed for mTLS end to end); a reverse proxy in front must
  pass TLS through for workload callers or they fall back to JWT-SVIDs.
- A new dependency on the SPIFFE Workload API (gRPC over a Unix socket) behind a feature; nothing
  changes for setups without `SPIFFE_ENDPOINT_SOCKET`.
- Tests run without SPIRE (certificates and JWTs made at test time behind the port); a check
  against a real SPIRE server and agent is only possible in a cluster (stage 3).

## Implementation notes: JWT-SVID for credential sources (User, 2026-11-14)
Decisions of the owner for the first stage that is built:
- **Scope:** only what is needed to get a JWT-SVID. New adapter crate `grauwolf-spiffe` (Cargo
  feature `credentials` of the host, which also covers `grauwolf-credentials`; ADR-0110 section 3
  uses it). It holds the port `WorkloadIdentity` with `jwt_svid(audience) -> JwtSvid` (the token
  and its SPIFFE ID for logs and errors; `Debug` hides the token) and `WorkloadApiClient`, which
  reads `SPIFFE_ENDPOINT_SOCKET` (`unix:///path`). The port is a trait that gets `client_tls`,
  `server_tls` and `validate_jwt_svid` when mTLS and inbound validation are built; neither is part
  of this stage.
- **Workload API client: an own minimal client, not the `spiffe` crate.** The `spiffe` crate (0.19)
  would build offline, its `build.rs` compiles `workload.proto` with `protox` (pure Rust), so no
  `protoc` is needed; but it brings `tonic`, `prost-build`/`protox`, `jsonwebtoken` and
  `x509-parser` into every build for one unary call, and the offline builder image (ADR-0096)
  would have to carry them. That was judged from its manifest and build script, not by building
  it. The own client speaks gRPC over `h2` (already in the lockfile through `reqwest`/`hyper`) on
  the Unix socket, with the messages of `proto/workload.proto` written by hand with `prost`
  derive (only `FetchJWTSVID`: no code generation, nothing to commit that could drift). It sets the
  `workload.spiffe.io: true` header the API requires and connects per call, so a restarted agent
  needs no handling.
- **Tests without SPIRE:** `FakeIdentity` (in-process port implementation) and `FakeWorkloadApi`
  (an `h2` server on a Unix socket, with `PERMISSION_DENIED` for unregistered workloads, and a
  check of the security header); tokens are shaped like JWT-SVIDs but not signed.
- **Trust domain:** examples use `agentswork.de`; grauwolf never needs it (the Workload API returns
  the token), it matters in the OpenBao role's `bound_subject`, see the user guide chapter 12.
- Not done in this stage: X.509-SVIDs, trust bundles, mTLS (`auth.spiffe` on `agent_tools` and
  `mcp_servers`), `validate_jwt_svid`, inbound authentication and `GRAUWOLF_SPIFFE_TRUST_DOMAINS`.

## Implementation notes: stages 1 and 2 (2026-11-15)
Built together as decided in Q1, for A2A and MCP over HTTP (Streamable HTTP and SSE). User guide chapter 9, developer guide section 9.2.
- **Crate:** own Workload API client (Q4) with `FetchX509SVID`, `FetchX509Bundles` (server streams, a live `X509Source`) and `ValidateJWTSVID`; rustls resolvers and verifiers on `rustls-webpki` (both already in the lockfile; `aws-lc-rs` is a direct dependency of the feature `testing` only, to sign test certificates, instead of `rcgen`). The port is `WorkloadIdentity` (JWT, validation) plus `TlsIdentity` (`client_tls`, `server_tls`), not one trait, so the JWT-only users stay stateless.
- **Inbound validation of JWT-SVIDs** uses `ValidateJWTSVID` of the SPIRE agent: no JWT crypto in grauwolf.
- **Spec:** `auth.spiffe` as in the ADR; deviation: `id`, `ids` and `trust_domain` exclude each other (exactly one in `mtls`, at most one in `jwt`), and `audience` with `mtls` is an error.
- **Inbound:** as Q3: the HTTP port accepts JWT-SVIDs, the second port (`GRAUWOLF_SPIFFE_PORT`) requires client certificates. The caller map is `GRAUWOLF_SPIFFE_USERS` (polled every two seconds, the resolved path, mtime and size are compared, so a ConfigMap's `..data` swap is seen). `GRAUWOLF_SPIFFE_TRUST_DOMAINS` extends the accepted domains; federated bundles must be in the Workload API (ADR-0122 is not built). On the HTTP port, requests with no credential at all are anonymous only where they were before (no token or users file and a local bind or `GRAUWOLF_ALLOW_ANONYMOUS`).
- **Feature flag:** `grauwolf-spiffe` is a plain dependency of the host (the adapters `grauwolf-a2a` and `grauwolf-mcp` use `ClientAuth`), not behind a Cargo feature `spiffe`; the pulled-in crates were in the lockfile already.
- **Caller in events (round 3):** `AgentEvent::AgentStarting` has an optional `caller` (SPIFFE ID text, skipped when `None`). It flows `CallerSpiffe` (request extension) → `RunExtras.caller` / host `RunOptions.caller` → `RunHandles.caller` → `AgentRun::with_caller` → the event, for the interpreter and generated services alike. The server has no request span of its own, so the OpenTelemetry feature records it as attribute `grauwolf.caller` on the `invoke_agent` span. A JWT-SVID call has a `caller` too; user-token and shared-token calls have none.
- **A2A over mTLS (round 3):** the A2A routes are on the main mTLS port (`/a2a`), as in `spec serve`, not on a second mTLS listener for the extra A2A port. Reason: `spec serve` has only one A2A address (the main port), so a caller uses the same URL scheme everywhere; the extra port `A2A_PORT` of a generated service exists for ingress routing and stays plain (JWT-SVIDs work there). A2A is built once per port (`service::routers_with_history`, same in `spec serve`) because the agent card names the endpoint an A2A client then sends to: the mTLS card names `GRAUWOLF_SPIFFE_PUBLIC_URL` (default `https://localhost:<port>`), the HTTP card the public URL. Before, the mTLS port served the card of the HTTP port, so a client that fetched the card over mTLS was sent to the plain port. Tests: `grauwolf-server/tests/spiffe_service.rs`.
- **Two URI SANs (round 3):** a certificate with more than one URI SAN is refused by both sides in the handshake (the existing exactly-one rule of `spiffe_id_of_certificate`, used by the verifiers); the test `a_certificate_with_two_uri_sans_is_refused_by_both_sides` in `grauwolf-spiffe/tests/mtls.rs` pins it (`TestCa::issue_with_uris`, `FakeIdentity::with_extra_uris`).
- **Generated code (round 3):** one slow case in `grauwolf-generator/tests/equivalence.rs`, `generated_services_authenticate_callers_by_spiffe`: the generated service binary with `GRAUWOLF_SPIFFE=1` runs against `FakeWorkloadApi`, which the test process serves on a Unix socket (`SPIFFE_ENDPOINT_SOCKET`); a mapped client gets an answer over mTLS, an unmapped one `403`, a client without a certificate is refused, and the `agent_starting` event has `caller`. Nothing from ADR-0109 stages 1 and 2 is left open.
- **Tests:** `grauwolf-spiffe/tests/mtls.rs`, `grauwolf-spec/tests/auth.rs`, `grauwolf-a2a/tests/auth.rs`, `grauwolf-server/tests/spiffe.rs` and `tests/spiffe_a2a.rs`, `grauwolf-host` `spiffe::tests`, all with `TestCa` and `FakeWorkloadApi`/`FakeIdentity`.

## Implementation notes: Q7-Q9, callers in `grauwolf-k8-ops` (2026-11-15)
- **`agent allow <target> --from <caller> [--as <user>]`, `agent deny <target> --from <caller>`, `agent callers <target> [--json]`.**
  `allow` writes `spiffe://<trust domain>/ns/<caller namespace>/sa/<caller>` (trust domain from the SPIRE server, as everywhere in the tool) into the ConfigMap `grauwolf-callers`, key `callers.json`, in the target's namespace, mapped to the caller's name (Q6) or `--as`. Both agents must exist. It says `created`, `updated` or `unchanged` for the entry, the ConfigMap and the NetworkPolicy; a repeat writes nothing. `deny` also removes the entries of an agent that no longer exists (by `/sa/<name>`). `callers` lists the IDs with user, agent (or `no such agent`), whether the policy admits the namespace, and `matches`/`differs`/`missing` for the policy.
- **NetworkPolicy `grauwolf-mtls-callers` (Q9).** Selects the pods `grauwolf.io/agent=<target>`, `policyTypes: [Ingress]`, two ingress rules: the HTTP port without `from` (anywhere), and the mTLS port from the `namespaceSelector`s (`kubernetes.io/metadata.name`) of the callers. **With no callers the second rule is omitted**, not written with an empty `from`: a rule without `from` admits every source, no rule admits none. The ports come from the Deployment's container ports `http` and `mtls` (defaults 8080 and 8443); `agent deploy` syncs the policy with its own ports and creates it (admitting nobody on the mTLS port) if missing. `ensure` trusts its hash annotation, which survives an edit by hand, so `sync_policy` compares the live `spec` and patches it back if it differs.
- **Pod (Q7, Q8).** `agent deploy` always sets `GRAUWOLF_SPIFFE=1`, `GRAUWOLF_SPIFFE_PORT` (`--mtls-port`, 8443), `GRAUWOLF_SPIFFE_USERS=/etc/grauwolf/callers/callers.json` and `GRAUWOLF_SPIFFE_PUBLIC_URL=https://<name>.<ns>.svc:<mtls port>` (all four reserved for `--env`), the container port `mtls`, the Service port `mtls`, and mounts `grauwolf-callers` as a **directory** at `/etc/grauwolf/callers`, never by `subPath`: a `subPath` mount does not follow a ConfigMap update, and the service needs the `..data` swap to reload. The empty map `{}` is made only if the ConfigMap is missing. The probes stay on the HTTP port. The ConfigMap is not part of the hash of the pod template, so `allow` and `deny` never restart the pod. `undeploy` keeps the ConfigMap and the policy.
- **Caller-spec check (Q8).** For every `agent_tools` and `mcp_servers` entry whose URL host is `<name>.<ns>.svc[.cluster.local]` of an agent of the cluster: a warning in `agent deploy` (never an error) and a line per deployed agent in `doctor`, when `auth.spiffe` is missing or its `id`/`ids`/`trust_domain` does not fit the target's ID; when, in the `mtls` mode, the URL is not `https` on the target's mTLS port (the `jwt` mode goes to the HTTP port and is not checked for that); and when the target has not allowed the caller (only for entries that carry `auth.spiffe`). `doctor` reads the specs from the agent's `grauwolf-agent` ConfigMap; an agent deployed from an image without `--spec` has none, and the line says it was not checked.
- **`doctor` (Q9).** Per agent: `callers` (the ConfigMap parses; a warning for each entry whose agent is gone or whose trust domain is another), `NetworkPolicy` (matches the callers) and `caller spec`. One warning `NetworkPolicy enforcement` as soon as a policy exists: the API accepts a policy whether or not the network plugin enforces it, so it cannot be known from there; the hint names the counter-checks (the live check, or a pod in a namespace that is not allowed trying the mTLS port).
- **`agent remove`.** Decided: it removes the agent from the `grauwolf-callers` and NetworkPolicies of the other agents that allowed it (listed before the confirmation, noted in the output), so nothing dangles; `doctor` still warns about dangling entries that come from elsewhere.
- **Open:** `agent deploy` starts `spec serve` without `--events` and `--otel`, so the `caller` of the events is not in `agent logs` yet; the live check reads it from `/run/stream` or a trace. Federation (Q5) and ADR-0122 are not touched.
- **Tests:** `grauwolf-k8-ops-app` `tests/callers.rs`, `tests/caller_spec.rs`, `tests/deploy.rs`.

## Open points
- Tools, credential sources (secret broker), credential gateways and scion tools:
  [ADR-0110](0110-workload-identity-for-tools-and-credentials.md).
- Delegation ("on behalf of"): passing the calling user's or workload's identity on to the next
  agent (token exchange or a signed claim), so a chain of agents can be authorized end to end.
- SVIDs towards a model gateway inside the trust domain instead of provider API keys.
- Per-route permissions beyond "mapped or not" (e.g. a workload that may run the agent but not
  decide approvals).

## Addendum (2026-11-15): one namespace per agent
Since ADR-0120, every agent set up with `grauwolf-k8-ops` lives in a namespace of its own. The agent
`<name>` runs as service account `<name>` in namespace `<name>`, so its SPIFFE ID is
`spiffe://<trust domain>/ns/<name>/sa/<name>`, e.g. `spiffe://agentswork.de/ns/reviewer/sa/reviewer`.
The examples above use this form and the trust domain of the owner's cluster. Before 2026-11-15
they used `spiffe://example.org/ns/agents/sa/<name>`, from the time all agents shared one `agents`
namespace. The service address of an agent follows the same scheme: `https://<name>.<name>.svc:<port>`.
Prefixes in the mapping file (`…/ns/batch/*`) still cover workloads that share a namespace.

## Decisions of the implementation round (User, 2026-11-15)
- **Q1:** stages 1 (outbound) and 2 (inbound) are built together, for A2A and MCP over HTTP. Delegation
  (ADR-0111) and the AgentWeaver manifests (stage 3) come later.
- **Q2:** with `auth.spiffe`, mTLS with X.509-SVIDs is the default. `"mode": "jwt"` is only for
  targets behind a TLS-terminating ingress. `auth.spiffe` without `id`, `ids` or `trust_domain` is a
  validation error.
- **Q3 (changes "Inbound" above):** **two ports instead of one.**
  - The HTTP port (8080) stays as it is, for people with tokens, the ingress and the probes. It also
    accepts JWT-SVIDs in `Authorization: Bearer`.
  - With `GRAUWOLF_SPIFFE=1` a second port (default 8443, `GRAUWOLF_SPIFFE_PORT`) serves mTLS with the
    own X.509-SVID, and **client certificates are required** there.
  - Reason: a TLS-terminating ingress (Traefik) in front of a single HTTPS port would have to trust the
    SPIRE CA or skip verification, and the kubelet probes would become harder.
- **Q4:** the X.509-SVID and the bundles come from the Workload API as a stream (`FetchX509SVID`,
  `FetchX509Bundles`) through the own client in `grauwolf-spiffe`, not the `spiffe` crate. Certificate
  and bundle are swapped without a restart, on the server and the client side, with rustls and an own
  check of the SPIFFE ID in the URI SAN.
- **Q5:** only the own trust domain is accepted by default; `GRAUWOLF_SPIFFE_TRUST_DOMAINS` is reserved
  for federation, which is its own decision: [ADR-0122](0122-spiffe-federation.md) (proposed, later).
- **Q6:** the user of a call from another agent is named after the calling agent, e.g. `coordinator`. Each caller gets an area of its own (conversations, memory, approvals). The mapping file can still map several IDs to one user.
- **Q7:** in a cluster set up with `grauwolf-k8-ops`, the mapping file is a ConfigMap `grauwolf-callers` in the target agent's namespace. It is mounted as the file `GRAUWOLF_SPIFFE_USERS` names, and grauwolf reads it again when it changes, without a restart.
- **Q8:** `grauwolf-k8-ops` gets these commands:
  - `agent allow <target> --from <caller> [--as <user>]`, `agent deny <target> --from <caller>` and `agent callers <target>`;
  - `agent deploy` sets `GRAUWOLF_SPIFFE=1` and opens the mTLS port in the pod and the service;
  - `agent deploy` warns, and `doctor` reports, when a spec calls an agent of the cluster with an `auth.spiffe.id` other than that agent's identity.
- **Q9:** `allow` also writes a NetworkPolicy in the target's namespace. The mTLS port then admits only pods from the namespaces of allowed callers, while the HTTP port stays open for the ingress. `doctor` warns when the cluster does not enforce NetworkPolicies.
- **Q10:** the live check uses two agents, `coordinator` calling `reviewer` over A2A with mTLS, with a real model answer and the caller shown in the events. Counter-checks:
  - an agent `intruder` without `allow` gets `403`, or is stopped earlier by the NetworkPolicy;
  - a wrong `auth.spiffe.id` fails the TLS handshake;
  - a pod without a certificate is refused on the mTLS port.

## Live check (2026-11-15, image built from `420924b`, k3s with SPIRE 1.15.5)
The agents `reviewer`, `coordinator` and `intruder` were set up with `grauwolf-k8-ops`. `coordinator` and `intruder` ran a flow whose `agent` node calls `reviewer` at `https://reviewer.reviewer.svc:8443` with `auth.spiffe.id`; the model was Ollama `qwen3.5:9b` on the owner's LAN. Then `agent allow reviewer --from coordinator`.
- **Allowed caller:** `coordinator` → `reviewer` over mTLS answered 200 with the reviewer's review.
- **No client certificate:** curl on 8443 failed in the TLS handshake (exit 56). `/health` on 8080 answered `ok`.
- **NetworkPolicy:** it is enforced. From the namespace `intruder`, 8443 was unreachable (exit 7) while 8080 answered. `intruder` did not even start: the agent card of `reviewer` could not be fetched.
- **Callers file:** with the policy opened for `intruder` by hand, `intruder`'s call got `403 … spiffe://agentswork.de/ns/intruder/sa/intruder is authenticated but not allowed to call this agent`. `agent callers` reported the edited policy as `differs`, and the next `allow` repaired it (`matches`).
- **Wrong `auth.spiffe.id`:** `agent deploy` warned before the rollout, and the pod failed at start because the handshake was refused.
  - **Finding (fixed 2026-11-15):** the error text only said "no connection … (refused or given up after 30 s)". The A2A and MCP clients now ask `grauwolf_spiffe::tls_refusal` for the cause and say `TLS handshake with <url> failed: the server is <id>, expected <id>; the call was not executed` (tests in `grauwolf-server/tests/spiffe_a2a.rs` and `grauwolf-mcp/tests/spiffe.rs`).
- **`deny` / `allow` without a restart:** they take effect after the kubelet has updated the ConfigMap in the pod. The first `403` came about 40 s after `deny`, and the `reviewer` pod had 0 restarts. A NetworkPolicy change does not cut connections that are already pooled; the callers file is the authoritative check. This is documented in user guide chapter 13.
- **`agent remove coordinator`:** it also took `coordinator` out of `reviewer`'s callers and NetworkPolicy.
- **`doctor`:** every per-agent line was ok, with the expected warning for `intruder` (not allowed). The enforcement warning stays, because the API cannot tell.
- **Not seen live:** `caller` in `agent_starting`, because `agent deploy` started `spec serve` without `--events`/`--otel`; the tests cover it. Since 2026-11-15 `agent deploy --otel` makes `grauwolf.caller` visible in Tempo (ADR-0121).
- Everything was removed afterwards: agents, build namespace, image.
- **Tempo (2026-11-15):** with `agent deploy --otel` the called agent's `invoke_agent` span carries `grauwolf.caller` and can be searched in Tempo (ADR-0121 live check). Traces of caller and callee were not linked then (no `traceparent` over A2A or MCP); ADR-0123 links them.
