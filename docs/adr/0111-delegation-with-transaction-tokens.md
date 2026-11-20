# ADR-0111: Delegation – acting on behalf of a person through a chain of agents

- **Status:** Accepted (User, 2026-11-13: "I find delegation important, let us discuss it
  technically but as its own ADR"; decisions: identity provider Entra ID; transaction token at the
  entry plus token exchange at the boundary to external systems, with an own interface
  `/api/identity`; external services both with user-bound credentials and with a checking gateway;
  unattended runs through stored grants with offline refresh tokens; delegation optional by
  default; the identity service as its own command). Stage 1 is implemented and checked live (see
  the implementation notes of parts A and B and the fixes from the live check); stage 3 (grants and
  schedules) is implemented, see "Implementation notes: stage 3", its live check is open; stages 2
  and 4 are open (see "Stages").
- **Date:** 2026-11-13
- **Related:** ADR-0109 (SPIFFE identities of agents), ADR-0110 (tools, credential sources, OpenBao,
  gateways), ADR-0068 (user tokens), ADR-0061 (approvals), ADR-0101 (flows).

## Context
With ADR-0109 a service knows **which agent** calls (SPIFFE ID, mTLS). It does not know **for whom**
the agent acts. In a chain `Anna → coordinator → Jira agent → Jira` every hop sees only the
previous agent; Jira sees the agent's token. So the agent's rights apply instead of Anna's, and the
audit shows "agent" instead of "Anna through the coordinator". The `user_id` in today's request
body is not proof: anyone who may call can write any name into it.

Delegation means: every call carries two things that can be **verified** – *on whose behalf* (the
person) and *who acts* (the calling agent's SPIFFE ID, already verified by ADR-0109).

People sign in with **Entra ID**. Entra issues access tokens and refresh tokens and offers the
On-Behalf-Of flow (OBO) for APIs it protects; it does not issue tokens that travel unchanged
through a chain of internal workloads.

## Decision
### Model
- **Inside the trust domain: a transaction token (Txn-Token), issued once at the entry**, carried
  unchanged through the chain, verified at every hop together with the caller's SPIFFE identity
  (modelled on the IETF draft "OAuth Transaction Tokens"; we follow its claims and header name where
  they fit and document deviations, because the draft is not final).
- **At the boundary to external systems: token exchange** (RFC 8693 semantics) through the identity
  service: Entra OBO for Entra-protected APIs, the person's own OAuth token for services such as
  Jira/Confluence (user-bound credentials), or a gateway decision (ADR-0110 section 4).
- **Proof, never a field:** "on behalf of Anna" is established only by a token Anna obtained (her
  Entra access token) or by a grant Anna created; a body field (`user_id`) is information, never
  proof of delegation.

### How "on behalf of Anna" is stated
1. **Interactive:** Anna signs in with Entra (AgentWeaver UI, a chat client, `grauwolf login` with
   the device code flow). The client calls the agent with `Authorization: Bearer <Entra access
   token>` (audience: the grauwolf identity API registered in Entra). The agent's service exchanges
   it at the identity service for a Txn-Token. Whoever is signed in is the person the agents act
   for.
2. **An application for Anna:** a backend that serves Anna obtains a token for the grauwolf
   identity API through Entra OBO and sends it – same path as 1.
3. **Unattended (schedules, cron):** Anna creates a **grant** once (UI or `grauwolf identity grant
   create`), consenting in Entra with `offline_access`. A run sends only the grant reference
   (`Grauwolf-Grant: grt_…`); the identity service checks that the caller's SPIFFE ID is the
   trigger named in the grant and issues a Txn-Token from the stored refresh token.

### The identity service: `grauwolf identity serve`
A separate command and deployment, **one per trust domain**; it holds the Txn-Token signing key
(in OpenBao Transit or a mounted key, never in agents). Agents talk to it only with mTLS (their
SPIFFE identity); people and applications with Entra tokens. HTTP interface `/api/identity`:

| Endpoint | Purpose |
|---|---|
| `POST /api/identity/token` | **Entry:** `subject_token` = an Entra access token (verified: tenant `tid`, issuer, signature from the tenant's JWKS, audience = the grauwolf identity API, expiry) **or** a grant reference (caller SPIFFE ID must be the grant's trigger) **or** an expiring Txn-Token for renewal (see "Lifetime"); response: a Txn-Token |
| `GET /api/identity/jwks.json` | public keys for verifying Txn-Tokens (agents cache them; key rotation with overlap) |
| `POST /api/identity/exchange` | **Boundary:** caller (SPIFFE ID) presents its Txn-Token and a target (`entra:<api scope>`, `atlassian`, `scion`, …); response: a token for that target – Entra OBO, the person's stored OAuth token, or a refusal if the policy does not allow this agent to act for this person on this target |
| `/api/identity/grants` | create (with Entra consent), list (own grants), revoke; a grant names the agent (SPIFFE ID), the purposes, the trigger (SPIFFE ID of the scheduler), the validity |
| `/api/identity/connections/{service}` | connect (OAuth authorization code flow with PKCE), callback, disconnect – user-bound credentials, e.g. Anna's Atlassian OAuth token |

- **Storage:** refresh tokens (Entra, Atlassian) encrypted in OpenBao (ADR-0110), keyed by the
  person's Entra object ID (`oid`) and the service; grant metadata in the identity service's store.
- **The identity service is a confidential Entra client** and authenticates to Entra without a
  client secret through workload identity federation with its own SPIFFE JWT-SVID (SPIRE's OIDC
  discovery provider as the federated issuer) or a certificate; to be verified in stage 1.
- **Policy** (file of the identity service): which agents (SPIFFE IDs or prefixes) may act on
  behalf of people, for which purposes and targets, and which triggers may use grants.

### The Txn-Token
- A JWT signed by the identity service, header `Txn-Token` on every internal call (HTTP, A2A, MCP
  over HTTP); never shown to the model, never logged, never in events.
- Claims: `iss` (identity service), `aud` (the trust domain), `sub` (the person: Entra `oid`),
  `name`/`upn` for display, `txn` (unique id of the chain), `purp` (purpose, e.g. `ticket.create`,
  from the request or the grant), `azd` (authorization details: grant id, allowed targets),
  `req_wl` (SPIFFE ID of the agent that requested it), `iat`, `exp`.
- **Lifetime:** short (default 15 minutes). A hop may renew a token that is about to expire at
  `/api/identity/token` with its own SPIFFE identity, up to a maximum chain lifetime (default 8
  hours, or the grant's limit); long flows (`for_each`, approvals that wait) renew transparently.

### In every grauwolf agent
- **Entry:** `spec serve` and generated services accept `Authorization: Bearer <Entra token>`
  (when an identity service is configured: `GRAUWOLF_IDENTITY_URL`) and `Grauwolf-Grant`, and
  exchange them for a Txn-Token at the start of the request. `gw_` user tokens (ADR-0068) stay for
  setups without Entra; they never produce a Txn-Token.
- **Hops:** a request with a `Txn-Token` header is accepted only from a caller authenticated by
  SPIFFE (ADR-0109) whose ID the agent's own policy allows to pass delegation on; the token is
  verified (signature from the cached JWKS, `aud`, `exp`). The person in `sub` becomes the request's
  user: conversations, long-term memory, per-user tool stores and **approvals** belong to Anna –
  only Anna can approve (ADR-0061).
- **Propagation:** the Txn-Token is attached automatically to every internal outgoing call of the
  run: `agent_tools` (A2A), `agent` and `for_each` nodes, MCP over HTTP, `fetch` to hosts with
  `auth.spiffe`. It is not attached to external hosts.
- **Tools at the boundary:** built-in tools that call external services (Jira, Confluence, git,
  scion) ask the identity service's `/exchange` for a token for their target when a Txn-Token is
  present; without one they use their own credentials (ADR-0110), unless the tool requires
  delegation.
- **Observability:** spans and the `agent_starting` event carry `on_behalf_of` (the person's
  display name and `oid`) and `txn`; every hop records the caller's SPIFFE ID, so the chain is
  reconstructable from traces (it is not stored in the token).

### External services: both forms
1. **User-bound credentials:** Anna connects her Atlassian account once
   (`/api/identity/connections/atlassian`); the exchange returns *her* token, and Jira enforces her
   real permissions. Entra-protected APIs (Microsoft Graph, own APIs) through Entra OBO.
2. **Checking gateway:** where per-person credentials do not exist or are not wanted, the gateway
   (ADR-0110 section 4, `grauwolf tools serve`) verifies the Txn-Token, applies its own rules for
   Anna and the calling agent, uses a service credential and records Anna in the external system
   (e.g. "created by the ticket agent for Anna") and in its audit.
- **scion** can take the Txn-Token's `sub` as the principal if its `Login` accepts Txn-Tokens
  (an extension in scion, analogous to its JWT-SVID login); until then the gateway form applies.

### Optional by default, required per tool
- Without a Txn-Token an agent acts with **its own** (narrow) rights, as today.
- A tool can require delegation: `"requires_delegation": true` on the tool's configuration (spec
  tools, built-in tool groups, MCP tools by name). Without a Txn-Token the call is refused with the
  text "this tool acts only on behalf of a person". Recommended for writing tools in external
  systems (`jira_create_issue`, `confluence_update_page`, `git_push`, `scion_land`, …).
- An agent can require delegation for every request: `"delegation": { "required": true }` in the
  spec; its service then refuses requests without an Entra token, grant or Txn-Token (`403`).

## Stages
1. **Identity service, entry and verification:** `grauwolf identity serve` with `/token` (Entra
   tokens) and `/jwks.json`, Txn-Token format and signing, verification in agents, the person as the
   request's user, propagation to A2A/MCP/flows, events and spans; live check against an Entra test
   tenant.
2. **Boundary:** `/exchange` with Entra OBO and user-bound Atlassian credentials
   (`/connections`), refresh tokens in OpenBao, built-in Jira/Confluence tools using it,
   `requires_delegation`.
3. **Grants:** `/grants`, `offline_access` consent, triggers by SPIFFE ID, `Grauwolf-Grant` at the
   entry, renewal for long flows, AgentWeaver schedules using grants.
4. **Gateway form and scion:** Txn-Token checks in `grauwolf tools serve`; scion login with
   Txn-Tokens (scion repository).

## Consequences
- Actions in external systems can be attributed to and limited by the person who asked, across any
  number of agents, without any agent holding a token that would let it act as that person
  elsewhere.
- A new central service per trust domain (availability matters: without it no new delegated
  requests start; running requests continue until their token's renewal fails).
- Entra-specific parts (token validation, OBO, consent, federated client credentials) live in an
  adapter; the Txn-Token format and the agent side do not depend on Entra.
- Stored refresh tokens are sensitive: OpenBao only, revocable by the person, bound to grants with
  purposes, triggers and validity.

## Open points
- Final claim names once the transaction-token draft settles.
- Whether Entra accepts SPIRE-issued JWT-SVIDs as federated client credentials in our setup
  (stage 1 check), else a certificate credential.
- Consent UI: in AgentWeaver or a minimal page of the identity service.

## Decisions for stage 1 with Entra Local (User, 2026-11-15)
The owner runs Entra Local (`github.com/cmaneu/entra-local`, an emulator of a slice of Entra ID) in the cluster at `https://entra.agentswork.de`, tenant `11111111-1111-1111-1111-111111111111`. It supports auth code with PKCE, client credentials, refresh tokens and device code, with RS256 tokens and JWKS. It does **not** support On-Behalf-Of or `client_assertion` (federated credentials). Its admin API is unauthenticated, so it is only for internal tests.
- **Q1, scope.** Stage 1 now, stage 3 (grants) next. From stage 2 only the Atlassian part, once an Atlassian OAuth app exists. Entra OBO and federated client credentials are checked later against a real Entra; until then the identity service uses a client secret.
- **Q2, app registrations.** A script in `examples/delegation/` creates them through Entra Local's admin API:
  - "grauwolf identity API", with the scope `access_as_user`; it is the audience of the people's tokens;
  - "grauwolf CLI", a public client with device code;
  - "grauwolf identity service", a confidential client with a secret, needed from stage 3 on.
  For a real Entra, a guide describes the same settings.
- **Q3, where the identity service runs.** `grauwolf identity serve` is a command of the CLI image. `grauwolf-k8-ops identity install` sets it up in namespace `identity` with its SPIFFE identity, an mTLS port like the agents, and its configuration (tenant, issuer, audience).
- **Q4, signing key.** OpenBao Transit signs the Txn-Tokens, so the key never leaves OpenBao. The identity service has its own OpenBao role.
- **Q5, verification in agents.** Agents fetch the identity service's JWKS (`/jwks.json`) from `GRAUWOLF_IDENTITY_URL` and cache it. `agent deploy --identity` sets this up and adds the identity service to egress. The Txn-Token travels in the `Txn-Token` header over A2A, MCP and flows. It never reaches the model, logs or events.
- **Q6, the person as user.** The request's user is the Entra object ID (`oid`). Events and spans also carry `upn`/`name` for display.
- **Q7, live check.**
  - `grauwolf login` (device code) as alice against Entra Local; a call of `coordinator`, which passes the Txn-Token to `reviewer`; in `reviewer` the user is alice's `oid` and her name appears in the events.
  - Counter-checks: a token of another tenant or audience gets 401; a manipulated or expired Txn-Token gets 401; an agent outside the identity service's policy gets 403 (live check: the entry agent passes it on as 403).

## Decisions for stage 3: grants (User, 2026-11-16)
- **Q1, scope.** In scope:
  - `/api/identity/grants` (create, list, revoke);
  - the `Grauwolf-Grant: grt_…` header at the entry agent;
  - the trigger check by SPIFFE ID;
  - renewal for long runs;
  - a scheduler (Q7).

  Not in scope: stage 2 (OBO, Atlassian), `requires_delegation`, AgentWeaver schedules.
- **Q2, how the refresh token reaches the service.**
  1. `grauwolf identity grant create` runs a **separate** device code sign-in of the public CLI client with `offline_access`, so the grant has its own token family, apart from the login cache.
  2. The CLI hands the refresh token to the identity service over HTTPS, with the person's access token as authorization, and keeps no copy.
  3. The service redeems it later as the same public client.

  The confidential-client consent flow is [ADR-0125](0125-grant-consent-through-the-identity-service.md) (proposed, later).
- **Q3, storage.** OpenBao KV:
  - metadata at `kv/identity/grants/<id>`;
  - the refresh token apart at `kv/identity/grant-tokens/<id>`.

  Each renewal writes the rotated refresh token back. The identity service's role gets exactly these paths. No database.
- **Q4, a grant holds:**
  - `id` (`grt_` + random);
  - the owner (`oid`, and `name`/`upn` when known);
  - **agent**: the entry agent's SPIFFE ID;
  - **trigger**: the SPIFFE ID that may start it;
  - `purposes`;
  - `valid_until`: default 30 days, at most 90;
  - `max_run`: default 4 h; it becomes the chain's `chain_exp`;
  - `created`, `last_used`.
- **Q5, entry.** The trigger calls the entry agent over mTLS with `Grauwolf-Grant`. The agent sends the grant id, the caller's SPIFFE ID and its own identity to the identity service. The service checks that:
  - the grant exists, is valid and is not revoked;
  - the agent is the grant's agent;
  - the caller is the grant's trigger;
  - the policy allows both.

  It also **redeems the refresh token on every run**, so a disabled person or a revoked consent fails. It then issues a Txn-Token with `azd: {grant}` and `purp` from the grant. `agent_starting` carries `on_behalf_of` with the grant id.
- **Q6, policy.** A `grants` section in the identity service's policy file:
  - `agents`: SPIFFE ID or prefix → allowed `purposes`;
  - `triggers`: a list of SPIFFE IDs or prefixes.

  A grant outside it is refused at creation. `identity allow|deny` get `--grants` and `--trigger`.
- **Q7, trigger.** `grauwolf-k8-ops schedule add <name> --agent <agent> --grant grt_… --cron "…" --input "…"` creates a Kubernetes CronJob in namespace `schedules`:
  - its own SPIFFE ID, `spiffe://<td>/ns/schedules/sa/<name>`;
  - the CLI image;
  - it runs a new command, `grauwolf call <url> --spiffe-id <agent> [--grant grt_…] --input …`, which calls an agent over mTLS with the workload identity.

  Also `schedule list | remove | run-now`.
- **Q8, management.**
  - People: `grauwolf identity grant create | list | revoke`, signed in with `grauwolf login`. `revoke` deletes metadata and refresh token at once.
  - Admins: `grauwolf-k8-ops identity grants [--revoke <id>]`, reading OpenBao.
  - Audit: every use of a grant logs a line in the identity service with grant id, `oid`, trigger, agent and `txn`.
- **Q9, purposes.** For grant runs the purposes come from the grant into `purp`, and the policy now enforces them. Interactive entries stay without a purpose for now.
- **Q10, live check with Entra Local.**
  - Alice creates a grant for `coordinator` with trigger `schedules/sa/morning`.
  - A CronJob runs every minute; `reviewer` shows Alice with the same `txn` and the grant id.
  - Counter-checks:
    - wrong trigger → 403;
    - expired or revoked grant → 401;
    - disabled user → 401, if Entra Local can disable users;
    - agent not in `grants.agents` → 403.
  - Cleanup afterwards.

## Implementation notes: stage 1 part A (2026-11-16)
Built: the crate `grauwolf-identity`, OpenBao Transit signing, `grauwolf identity serve`, the entry
and hops in every agent (`grauwolf-server`), the propagation in A2A, MCP over HTTP and `fetch`,
`on_behalf_of` in `agent_starting` and the agent span. User guide chapter 14, developer guide 9.9.

Decisions taken during the implementation:
- **Entra token or JWT-SVID.** Both arrive as `Authorization: Bearer <JWT>`. A JWT whose `sub`
  starts with `spiffe://` is a JWT-SVID. Otherwise the agent compares the unverified `iss` with
  `person_issuer` from `GET /api/identity/config` (fetched once per process and cached); a match is
  exchanged, anything else follows the SPIFFE and token rules as before. Unverified claims only
  choose a path; the identity service verifies the token.
- **Entry errors.** A refused token is `401` at the agent, an agent outside the identity service's
  policy is `403` (the service's own answer, passed on; changed after the live check, see "Fixes from the live check"); an unreachable identity service is `503`.
- **Hops.** A `Txn-Token` is verified only after a SPIFFE caller has been mapped by the agent's
  callers file (ADR-0109). The callers file is "the agent's own policy" of the section "In every
  grauwolf agent"; there is no separate list. From any other caller (shared or user token,
  anonymous, a person's token) the header is `403`; to an agent without `GRAUWOLF_IDENTITY_URL` it
  is `401`. Invalid tokens are `401`, unreachable keys `503`.
- **Renewal.** Any workload of the token's trust domain may renew; `txn`, `sub`, `purp`, `req_wl`
  and `chain_exp` are kept and `exp` is capped by `chain_exp`. The policy is asked only at the
  entry.
- **`on_behalf_of`** carries `oid`, `name`, `upn` and `txn`.
- **Propagation only over SPIFFE connections:** the header is added only by a client that has a
  `ClientAuth` (A2A, MCP over Streamable HTTP and HTTP+SSE, `fetch` hosts with `auth`). Local
  sub-agents share the run's scope. The scope does not cross `tokio::spawn`; the server opens it
  inside the spawned task of streaming runs.
- **Agent environment:** `GRAUWOLF_IDENTITY_URL` (`https://` means mTLS with the agent's X.509-SVID,
  `http://` a JWT-SVID) and `GRAUWOLF_IDENTITY_SPIFFE_ID` (the server's expected ID over `https`,
  required over `http` as the JWT-SVID audience). `GRAUWOLF_IDENTITY_URL` without `GRAUWOLF_SPIFFE`
  is a start error in generated services and in `spec serve`.

Deviations from the transaction-token draft:
- `chain_exp` is a grauwolf claim (the maximum chain lifetime); the draft has none.
- `aud` is the trust domain as a plain string.
- The JOSE `typ` is `txntoken+jwt` and the header name `Txn-Token`, as in the draft; the token
  request uses RFC 8693 token types (`urn:ietf:params:oauth:token-type:access_token` and
  `…:txn_token`) at `/api/identity/token` rather than a separate endpoint per kind.

Open in stage 1 after part A: `agent deploy --identity` and `grauwolf-k8-ops identity install` (Q3,
Q5), the app registration script in `examples/delegation/` (Q2), `grauwolf login` and the live
check (Q7) – part B below, except the live check.

## Implementation notes: stage 1 part B (2026-11-16)
Built: `grauwolf-k8-ops identity install | status | uninstall | allow | deny`, `agent deploy
--identity`, `grauwolf login | logout`, `examples/delegation/` (the Entra Local script and the whole
flow). User guide chapters 13 and 14, developer guide 9.6 and 9.9.

Decisions taken during the implementation:
- **Names.** Namespace `identity` (label `grauwolf.io/component=identity`, no agent label), service
  account, Deployment, Service, `ClusterSPIFFEID`, OpenBao role and policy `grauwolf-identity`; the
  SPIFFE ID is `spiffe://<td>/ns/identity/sa/grauwolf-identity`. The Transit mount is `transit/`, the
  key `grauwolf-identity` (`--transit-key`), created if missing and never replaced or rotated by the
  tool. `--public-url` (`iss` of the Txn-Tokens) defaults to the mTLS URL in the cluster.
- **The policy** is the ConfigMap `grauwolf-identity-policy`, mounted as a directory and not part of
  the pod hash, so the watched file applies without a restart. `--policy <file>` replaces it;
  without it the policy is kept. `identity allow <agent> [--purpose …]` / `identity deny <agent>`
  edit it like `agent allow/deny` (the agent must exist for `allow`; `deny` also removes the entries
  of an agent that is gone). Only entry agents need an entry.
- **Network.** The ingress policy admits the mTLS port from every namespace with `grauwolf.io/agent`
  (any agent of a chain renews), the HTTP port from anywhere like an agent's. Agents need no ingress
  change on the identity side; with `--egress` the service is one of their destinations. The
  service's egress: DNS, OpenBao, and the keys host (`--jwks-url`, else the issuer) – an external
  name only by port. The SPIRE agent is the CSI socket, not a network destination.
- **`agent deploy --identity`** reads the identity Service (refused without it) and sets
  `GRAUWOLF_IDENTITY_URL` = `https://grauwolf-identity.identity.svc:<mtls port>` and
  `GRAUWOLF_IDENTITY_SPIFFE_ID`. A deployed agent always runs with `GRAUWOLF_SPIFFE=1`, so "refuse
  without SPIFFE" cannot happen; `--env` refuses both variables always.
- **`grauwolf login`.** The CLI has no command that calls an agent (no `call`, no `run --remote`),
  so the cached token is handed over by `grauwolf login --print-token` (refreshed when less than 60 s
  are left) instead of a `grauwolf token` command: that name is the user token of ADR-0068.
  `offline_access` is always requested. Settings: flags, then `GRAUWOLF_LOGIN_*`, then the last
  login (stored in the cache). The cache is `<config dir>/login.json`, mode 0600, directory 0700.
- **Entra Local** (from its sources): the admin API has `POST /admin/api/apps` (`displayName`,
  `isConfidential`, `appIdUri`), `POST …/apps/{id}/scopes`, `POST …/apps/{id}/secrets` (the
  `secretText` once); the device code flow works for every public client; an access token's `aud` is
  the resource application's **ID**, not its URI, while v1.0 tokens and the token generator of its admin interface use the URI (changed after the live check: several audiences, see "Fixes from the live check").
- **`trust-domain set|restore`** do not rewrite the role `grauwolf-identity`; `identity install`
  again binds it to the current domain.

Open in stage 1: the live check (Q7).

## Implementation notes: fixes from the live check (2026-11-16)

The live check in a k3s cluster with Entra Local found five issues; each is fixed in its own commit.

- **`grauwolf login` without a name.** A token without `name`, `upn` and `preferred_username` is shown as `logged in as oid <oid>`, otherwise as `logged in as <name> <<upn>> (oid <oid>)`.
- **Policy reload message.** `identity allow|deny` says "Kubernetes passes it to the pod within about a minute or two; the service then reads it again": a mounted ConfigMap reaches the pod only after the kubelet's sync. `agent allow|deny` makes no promise about timing.
- **403 at the entry.** `IdentityClient::exchange` maps the service's `403` to `IdentityError::Forbidden`, the entry answers `403` with the reason (`delegation refused: …`); `401` stays for a refused person's token, `503` for an unreachable service.
- **Several audiences.** `aud` of a v2.0 Entra token is the API's application ID (GUID); v1.0 tokens and Entra Local's token generator carry the application ID URI. `EntraSettings.audiences` is a list and a token is accepted when its `aud` (text or list) names any of them; `GRAUWOLF_IDENTITY_AUDIENCE` is a comma-separated list (`IdentityServiceConfig::audiences`), `identity install --audience` is repeatable and joins its values with commas. The deployment of an existing install keeps its single value until `identity install` is run again.
- **Egress to the cluster's own ingress.** With split DNS the issuer's host resolves, inside the cluster, to the LoadBalancer IP of the ingress Service (k3s: `kube-system/traefik`, port 443 to the named target port `websecure`, container port 8443), and kube-proxy sends the traffic straight to the Traefik pod, which `grauwolf-egress` (443 to everything outside the cluster ranges) blocks. `egress::plan` now resolves external names through `Services.resolver` (`Resolver::System`, tests `Fixed`); a Service whose LoadBalancer IP or external IP is an address of the name gets a rule to its pods (namespace, selector, pod ports via `target_port` as in ADR-0110 finding 2), next to the external rule, and the output says `egress: <host> resolves to the LoadBalancer of <ns>/<service>; allowing its pods (port <p>)`. The same code serves `agent deploy --egress`. Limit: the host is resolved on the machine that runs `grauwolf-k8-ops`, which must see the cluster's split DNS; the hand-made NetworkPolicy of chapter 14 stays as the fallback. The fake cluster of `grauwolf-k8s` lists a namespaced kind across namespaces now.

## Implementation notes: stage 3 (2026-11-16)
Built: grants in the identity service, `Grauwolf-Grant` at the entry agent, `grauwolf identity grant
create | list | revoke`, `grauwolf call`, `grauwolf-k8-ops identity allow|deny --grants|--trigger`,
`identity grants`, `identity install --grant-client-id` and `schedule add | list | remove | run-now`.
User guide chapter 14 ("Unattended runs: grants and schedules"), developer guide 9.9.

Decisions taken during the implementation:
- **Endpoints.** `POST /api/identity/grants` (`{ agent, trigger, purposes?, valid_until?, max_run?,
  refresh_token }`, the person's Entra access token as bearer; `201` with the grant),
  `GET /api/identity/grants` (own grants), `DELETE /api/identity/grants/{id}` (`204`; another
  person's grant is `404` like a missing one). The grant run is `POST /api/identity/token` with
  `subject_token` = the grant ID, `subject_token_type` = `urn:grauwolf:params:oauth:token-type:grant`
  (a grauwolf type; a `grt_` subject without a type means the same) and `trigger` = the SPIFFE ID of
  the entry agent's caller. The service trusts the entry agent to name its caller truthfully: it is
  an authenticated workload, and the caller had to pass the agent's callers file.
- **Order of the checks** at a run: the grant exists (`401`) → the caller is the grant's agent and
  `trigger` its trigger (`403`) → `grants` in the policy allows both with every purpose of the grant
  (`403`) → still valid (`401`) → the stored refresh token redeemed (refused `401`, Entra or OpenBao
  unreachable `503`) and its access token verified like a person's token, naming the owner's `oid`
  (`401`). Then the rotated token and `last_used` are stored and one audit line is printed.
- **Claims.** `purp` is the grant's purposes joined by spaces (no purposes: none), `azd` is
  `{"grant": "<id>"}`, `chain_exp` = now + `max_run`; renewal keeps all three. `max_run` may be at
  most the service's maximum chain lifetime (8 h), at least 60 s. `on_behalf_of.grant` and the span
  attribute `grauwolf.grant` name the grant.
- **Creation** redeems the handed-over refresh token once: a refusal is `400`, another person's
  token `403`; the rotated token is stored, the handed-over one is used up. The refresh token never
  appears in the grant, a list, an error or a log line.
- **Configuration.** Grants are on when `GRAUWOLF_IDENTITY_GRANT_CLIENT_ID` (the public client
  "grauwolf CLI") is set; the token endpoint is `GRAUWOLF_IDENTITY_GRANT_TOKEN_URL` or the issuer's
  OpenID discovery (consistent with the JWKS of stage 1); the scope is `GRAUWOLF_IDENTITY_GRANT_SCOPE`
  or `<first audience that is a URI>/<scope> offline_access`, which is what `grauwolf login` asks for;
  the KV mount `GRAUWOLF_IDENTITY_GRANT_KV_MOUNT` (default `kv`). Without the client ID every grant
  call is `503`.
- **The CLI's identity URL.** `grauwolf identity grant` takes `--identity-url`, else
  `GRAUWOLF_LOGIN_IDENTITY_URL`, else the URL stored by `grauwolf login --identity-url` – the same
  order as the other login settings.
- **`grauwolf call`** posts to `/run` only (no A2A); with `SPIFFE_ENDPOINT_SOCKET` it uses mTLS over
  `https://` and a JWT-SVID for `--spiffe-id` over `http://`; exit codes 0, 1 and 3 (refused). It
  lives as `call_agent` in `grauwolf-identity` with the grants client, so the CLI needs no HTTP
  client of its own.
- **Schedules.** One `ClusterSPIFFEID` per schedule (`grauwolf-schedule-<name>`, pod label
  `grauwolf.io/schedule`), so a schedule's identity disappears with it; the schedule is a caller of
  the agent with the user `schedule-<name>` (the grant's person replaces it in a run).
  `schedule add` does not edit the identity policy; it says what is missing. `run-now` creates a Job
  `<name>-manual-<unix time>` from the job template.
- **Admins** see every grant with `grauwolf-k8-ops identity grants`, which reads only the metadata
  folder; `--revoke` deletes the token's and the metadata's secret without reading the token. The
  operator's policy gained these KV rules (run `openbao setup` again).

Deviations from the brief of stage 3: purposes are enforced in the same commit as the grant run
(step 5 needed no change of its own); `grauwolf call` has no `--stream` and no A2A mode.

