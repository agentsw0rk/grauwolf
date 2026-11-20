# ADR-0110: Workload identity for tools, credential sources and credential gateways

- **Status:** Accepted except delegation (User, 2026-11-13: "write the ideas into an ADR –
  communication between agents, which tools are suitable, the secret broker (HashiCorp Vault or
  OpenBao), a service in front (gateway or MCP server), and tools for my project scion with its
  SPIFFE implementation"; decisions: scion as an own tool that works like the scion CLI, OpenBao as
  the secret broker, Jira and Confluence tools stay in the project). Delegation is decided in ADR-0111.
- **Date:** 2026-11-13
- **Related:** ADR-0109 (SPIFFE/SPIRE for agents), ADR-0046/0098/0104 (built-in tools), ADR-0099
  (`run_command`), ADR-0029 (WASM tool ABI), ADR-0107 (tool packages, signatures), ADR-0108
  (central configuration, layers).

## Context
ADR-0109 gives every agent a SPIFFE identity and uses it between agents and towards MCP servers over
HTTP. Agents use many more things that need credentials: built-in tools (`fetch`, Jira, Confluence,
git), stdio MCP servers, `run_command`, WASM tools, tool registries and model providers. Most
external services do not accept SVIDs; their tokens sit in the environment today (`GIT_TOKEN`,
`ATLASSIAN_API_TOKEN`, `GITHUB_TOKEN`, provider API keys), one long-lived secret per service, shared
by every tool call of the agent and readable by anything in the process.

This ADR records where workload identity fits and where it does not, two patterns that let agents
use services without SPIFFE support without holding long-lived tokens (credential sources through a
secret broker, and a credential gateway in front of the service), and tools for scion, a version
control system with SPIFFE workload identity (github.com/ArekCzarnik/scion).

## 1. Communication between agents (summary of ADR-0109)
- Agent ↔ agent (A2A) and agent → MCP over HTTP: mTLS with X.509-SVIDs by default, JWT-SVIDs for
  targets behind a TLS-terminating ingress; the target's SPIFFE ID is checked (`auth.spiffe` on
  `agent_tools` and `mcp_servers`).
- Inbound: the service authenticates callers by client SVID or JWT-SVID and maps SPIFFE IDs to
  users; unmapped IDs get `403`.
- Each agent calls with its own identity. **Delegation** (a chain of agents acting for the original
  caller) stays open; the candidate is OAuth 2.0 Token Exchange (RFC 8693) with the caller's
  identity as `subject_token` and the agent's JWT-SVID as `actor_token`, so the last service sees
  both "who asked" and "which agent acts".

## 2. Which tools suit workload identity
| Tool kind | Fit | Rule |
|---|---|---|
| **MCP server over HTTP** | **yes – main case** | ADR-0109 stage 1 (`auth.spiffe`). The gain is on the server side: the MCP server authorizes **per agent identity** (which tools, which data). grauwolf as an MCP server (`spec mcp` over HTTP) uses ADR-0109 stage 2. |
| **Built-in `fetch`** | yes, for internal hosts | New optional `builtin_tools.fetch.auth`: per allowed host an `auth.spiffe` block as in ADR-0109 (mTLS or JWT-SVID with audience). External hosts stay without. The model never sees certificates or tokens: the host process makes the request. |
| **WASM tools, ABI v1** | not needed | No imports, no network, nothing to authenticate. |
| **WASM tools, a future ABI v2 with HTTP** | yes – binding rule | Outgoing requests are **made by the host on behalf of the tool** with the agent's identity or a credential from a credential source (section 3); the module never receives a certificate, key or token. Allowed hosts and their `auth` are granted in the spec, as ADR-0107 requires for capabilities. |
| **stdio MCP server** (child process) | possible, opt-in | `mcp_servers.<name>.workload_api: true` passes `SPIFFE_ENDPOINT_SOCKET` to the child. Under the Kubernetes workload attestor it gets **the agent's identity** (same pod and service account); a separate identity needs SPIRE's Unix attestor (user ID or binary path selectors) – cluster configuration, documented, not done by grauwolf. Default: not passed, as today (cleared environment). |
| **`run_command`** | only explicitly, not in shell mode | `builtin_tools.command.workload_api: true` passes the socket to allowlisted programs (e.g. a CLI that logs in with SPIFFE). Refused together with shell mode: a model-controlled shell must not act as the agent towards internal services. |
| **Jira, Confluence, GitHub, gitlab.com, model providers** | no direct support | They accept API tokens, PATs, OAuth or app tokens, not SVIDs. Use a credential source (section 3) or a credential gateway (section 4). |
| **Internal git server** (Gitea, GitLab self-managed, plain git over HTTPS) | yes, with a proxy | An mTLS proxy in front of the git server checks the SPIFFE ID and maps it to a git user. grauwolf's own smart-HTTP transport (ADR-0104 addendum) presents the agent's X.509-SVID: `git` tool configuration and git tool-package/skill sources get the same `auth.spiffe` per host. |
| **Tool-package and skill signatures** | yes, as signer identity | A SPIFFE ID as the identity of a keyless signature (e.g. the CI publisher `spiffe://example.org/ci/tool-publisher`); the policy (ADR-0107 stage 4) would allow signers by SPIFFE ID. Belongs to the "keyless" open point of ADR-0107; whether the chosen signing service issues certificates for SPIFFE identities has to be checked first. |

## 3. Credential sources: tokens from a secret broker instead of the environment
- **Idea:** every credential grauwolf reads through the central configuration (ADR-0108: `Env`,
  `Config` sections, `VARIABLES`) can come from a **source** instead of a plain variable. The
  agent authenticates to the broker with its JWT-SVID; the broker hands out the credential mapped
  to that identity. No long-lived token in the image, the manifest or the environment; rights per
  agent; central revocation.
- **Sources:**
  1. `env` – today's behaviour (default).
  2. `file` – a file mounted by the platform (Kubernetes secret, Vault Agent / OpenBao Agent
     sidecar, Secrets Store CSI driver), re-read when it changes. This needs no broker code in
     grauwolf and already works with brokers that write files.
  3. `openbao` (alias `vault`) – OpenBao (or HashiCorp Vault) directly: login with the agent's JWT-SVID through the
     JWT auth method (role per agent identity), then a KV v2 read or a dynamic secrets engine.
     Leases are renewed or the secret is read again before it expires; a GitHub App installation
     token engine (plugin) gives short-lived, repository-scoped git tokens.
- **Configuration** (deployment side, not in the spec – the spec stays portable): a section
  `[credentials]` in `grauwolf.toml` next to the spec or the file named by
  `GRAUWOLF_CREDENTIALS` (precedence like `GRAUWOLF_TOOL_POLICY`):
  ```toml
  [credentials.openbao]
  address = "https://openbao.internal:8200"
  auth = { spiffe = { audience = "openbao", role = "grauwolf-agents", mount = "jwt" } }

  [credentials.variables]
  GIT_TOKEN           = "openbao:github/token/reviewer#token"     # GitHub App token, short-lived
  ATLASSIAN_API_TOKEN = "openbao:kv/agents/reviewer#jira"      # KV v2
  OPENAI_API_KEY      = "file:/run/secrets/openai/api-key"
  ```
  Only variables listed in `VARIABLES` may be mapped (unknown names are an error); a mapped
  variable that is also set in the environment is an error (no silent precedence).
- **Implementation:** a credential source is another implementation of the `Env` port that the
  central configuration already reads through: `env` → `file` → `openbao`, resolved when a
  configuration section is built and refreshed before expiry. Vault access lives in a new adapter
  (e.g. `grauwolf-credentials`), the SPIFFE part reuses `grauwolf-spiffe` (ADR-0109). Values never
  appear in errors, logs, events or spans; an unreachable broker at start-up fails the run with an
  error naming the variable and the source, not the value.
- **Order:** `file` first (small, works with every broker that writes files), `openbao` second.

## 4. A service in front: credential gateway or MCP server
- **Idea:** the credential for an external service stays in a service **in front of it**; agents
  call that service with their SPIFFE identity (ADR-0109, mTLS or JWT-SVID), and the service
  decides **per agent identity** what is allowed – finer than the external service's own tokens
  can express (e.g. "the agent `ticket-router` may read and comment in project SUP, but not close
  issues").
- **Forms:**
  - **MCP server per service** (Jira, Confluence, GitHub): holds the token (from a broker,
    section 3), offers only the operations an agent should have, checks the caller's SPIFFE ID.
    grauwolf needs nothing new: `mcp_servers.<name>` with `url` and `auth.spiffe`.
  - **Egress gateway that injects credentials:** the agent's HTTP requests go through a gateway
    that authenticates the agent by SPIFFE ID and adds the service's credential per destination;
    the credential never enters the agent's pod. grauwolf needs only that its HTTP clients honour a
    proxy and present the SVID (ADR-0109).
  - **grauwolf as the tool gateway:** `grauwolf tools serve` offers built-in tools (Jira,
    Confluence, git, `fetch`) as an MCP server over HTTP, with ADR-0109 inbound authentication and a
    per-identity tool allowlist (mapping file of ADR-0109 extended by `tools`), its own credentials
    from a credential source. Agents then use Jira through this gateway instead of holding
    `ATLASSIAN_API_TOKEN` themselves. This reuses the built-in tools, the MCP server code and the
    SPIFFE work.
- **Recommendation:** Jira/Confluence and GitHub through a gateway when per-agent rights matter;
  otherwise credential sources (section 3) are the smaller step.

## 5. Tools for scion
scion (github.com/ArekCzarnik/scion) is a version control system for always-online clients, coding
agents and CI: history and expensive operations on the server, objects fetched on demand, sparse
checkouts, commits stored on the server at once, a per-node daemon cache, `land` with a versioned
compare-and-swap. Workloads log in with a JWT-SVID (`scion login --spiffe`): the client fetches a
JWT-SVID for the audience from `GetServerInfo` (`spiffe_audience`) and exchanges it through `Login`
for a scion bearer token whose principal is the SPIFFE ID; on `UNAUTHENTICATED` it exchanges again
once. ACLs name SPIFFE IDs as members.

- **Tool set** (names reserved, offered when configured; final names and arguments are checked
  against scion's CLI JSON schemas `schemas/v1` and its gRPC API):
  | Tool | Purpose |
  |---|---|
  | `scion_checkout { ref, dir, sparse? }` | sparse checkout into a directory under the tool root |
  | `scion_status { dir }`, `scion_diff { dir }` | state of a working copy |
  | `scion_commit { dir, message }` | commit (stored on the server at once) |
  | `scion_land { dir, ref }` | land a stack onto a ref – **needs approval** (ADR-0061) unless `allow_land: true`, like `git_push` |
  | `scion_log { ref, path?, limit? }`, `scion_blame { ref, path }`, `scion_merge_base { a, b }`, `scion_read_file { ref, path }` | server-side queries without a checkout |
- **Identity:** the tools use the agent's SPIFFE identity (JWT-SVID with scion's audience, token
  exchange, re-exchange once on `UNAUTHENTICATED`); `SCION_SERVICE_NAME`/`SCION_SERVICE_SECRET`
  as a fallback for setups without SPIRE. The scion token is held in memory, never offered to the
  model, never logged.
- **Where the code lives – three options:**
  1. **MCP server in the scion repository** (`scion mcp`, stdio and HTTP): scion owns its protocol
     and its build (including `protoc`); grauwolf uses it through `mcp_servers` – over HTTP with
     `auth.spiffe`, or stdio with `workload_api: true` (section 2) so `scion` logs in with the
     agent's identity itself. grauwolf gets an example spec and a guide section, no scion
     dependency. Also usable by every other MCP client.
  2. **Built-in tool group in grauwolf** (`grauwolf-tools-scion`, feature `scion`) on scion's gRPC
     API: typed tools like the git tools, but grauwolf depends on scion's protocol crate (and its
     `protoc` build step, which grauwolf's builds and the offline builder image do not need
     today).
  3. **Built-in tools that run the `scion` CLI** with `--json`: no protocol dependency, but the
     image needs the `scion` binary and the daemon socket (as the git tools needed `git` before
     ADR-0104's addendum).
- **Decision (User, 2026-11-13): an own tool in grauwolf that works like the scion CLI** – a
  built-in tool group `scion_*` (adapter `grauwolf-tools-scion`, feature `scion`) whose tools mirror
  the CLI commands (`checkout`, `status`, `diff`, `commit`, `land`, plus the server-side queries)
  with the CLI's JSON shapes (`schemas/v1`) as results, but implemented natively on scion's gRPC API
  (option 2), not by running the `scion` binary. Consequences and rules:
  - **No `protoc` in grauwolf builds:** the client is generated from scion's `.proto` files with a
    pure-Rust protobuf compiler (e.g. `protox` feeding `tonic-build`/`prost-build`), or the
    generated code is committed; the offline builder image (ADR-0096) must keep building without
    network or `protoc`. The `.proto` files are taken from a pinned scion revision; a scion
    protocol change is a deliberate update in grauwolf.
  - **Working copies:** checkouts live under `builtin_tools.root` (paths as for the file tools);
    the tools talk to `scion-daemon` over its Unix socket when `SCION_ENDPOINT` names one (shared
    object cache), otherwise directly to `scion-server` over TLS/gRPC.
  - **Identity:** SPIFFE token exchange as described above; the scion token stays in the adapter.
  - Option 1 (an MCP server in the scion repository) stays possible for other MCP clients; it is
    not the way grauwolf uses scion.

## Stages (after ADR-0109 stage 1)
1. `workload_api` opt-in for stdio MCP servers and `run_command`; `builtin_tools.fetch.auth`;
   `auth.spiffe` for git hosts (git tools, git sources of tool packages and skills).
2. Credential source `file`, then `openbao` (JWT auth with the agent's JWT-SVID, KV v2, leases),
   as `Env` implementations behind the central configuration.
3. scion tools: built-in `scion_*` group on the gRPC API (section 5).
4. `grauwolf tools serve` as a tool gateway with per-identity tool allowlists.
5. Delegation (token exchange) and SPIFFE identities as signers of tool packages (with ADR-0107).
6. Egress list derived from the spec (`grauwolf spec egress`, `app/egress.json`) and the
   placeholder contract for credential gateways (addendum below).

## Consequences
- Long-lived tokens leave images, manifests and the environment step by step: SPIFFE where the
  target supports it, a broker or a gateway where it does not.
- Rules that keep model-controlled code away from identities: WASM tools never hold credentials;
  shells never get the Workload API socket; stdio servers and allowlisted programs only by explicit
  opt-in.
- New configuration on the deployment side (`[credentials]`), not in the spec; specs stay portable
  between environments.

## Decisions (User, 2026-11-13)
1. **scion:** an own built-in tool group that works like the scion CLI, natively on the gRPC API
   (section 5).
2. **Secret broker: OpenBao.** The broker source targets OpenBao (its API is compatible with
   Vault's, so Vault works as well): JWT auth with the agent's JWT-SVID, KV v2 first; dynamic
   engines (e.g. GitHub App tokens) when needed. The source is named `openbao` in configuration
   (`"openbao:kv/agents/reviewer#jira"`, section `[credentials.openbao]`); `vault:` is accepted as
   an alias. Tests run against a recorded/mocked API; a live check needs an OpenBao server.
3. **Jira and Confluence stay built-in tools of grauwolf** (ADR-0104). Their credentials come from
   a credential source (OpenBao); where per-agent rights are needed, the gateway form is
   `grauwolf tools serve` with the same built-in tools – no separate MCP servers.
4. **Delegation:** its own decision, [ADR-0111](0111-delegation-with-transaction-tokens.md).

## Implementation notes: credential sources `file` and `openbao` (User, 2026-11-14)
Implemented in the new adapter crate `grauwolf-credentials` (adapters; depends on `grauwolf-config`
and `grauwolf-spiffe`); the host applies it behind the Cargo feature `credentials` (in `default`).
Decisions of the owner and what was built:
- **Login:** the JWT auth method with the agent's JWT-SVID (`POST /v1/auth/<mount>/login
  {role, jwt}`), not cert auth. KV v2 reads: `GET /v1/<mount>/data/<path>`, the field after `#`
  (`openbao:kv/agents/reviewer#jira` is mount `kv`, path `agents/reviewer`, field `jira`).
- **Refresh:** values are cached in memory and read again after `refresh` (default 5 minutes;
  `30s`/`5m`/`1h`). The OpenBao token is kept in memory and replaced by a **fresh login** (a new
  JWT-SVID; they live 5 minutes) when it is within 30 seconds of its lease end, and once after a
  `403`; renewal of the token is not used. A failed refresh keeps the old values and reports on
  stderr (variable and source only). An unreachable broker at start fails the start with an
  error naming variable and source. Values never appear in errors, `Debug`, logs, events or spans.
- **Mechanics:** `CredentialEnv::start` resolves everything once (blocking, in a thread with its
  own runtime) and keeps refreshing in that thread; `CredentialEnv` is an `Env`, so the sync
  configuration code needs no change. `file:` sources are re-read on the same interval.
- **Configuration as specified,** plus: `refresh`; relative `file:` paths are relative to the
  configuration file; variables that the process reads from its environment directly (`PATH`,
  `HOME`, `DOCKER_CONFIG`, `OTEL_EXPORTER_OTLP_*`, `CARGO`, `RUSTC`, `SPIFFE_ENDPOINT_SOCKET`,
  `GRAUWOLF_CREDENTIALS`) cannot be mapped (error) because a source would silently not apply to
  them; the `[credentials]` table is ignored by the tool-policy parser. `vault:` is an alias.
- **Wiring:** `grauwolf_host::credentials::env_for(base, spec file)`; `AgentLoader::load_provisioned`
  uses it (so `spec run|chat|resume|mcp|serve`, sub-agents and provisioning read through the
  sources), `spec serve` also for `GRAUWOLF_SERVER_TOKEN` and the chat API, `service::main` of
  generated services for everything it reads (`GRAUWOLF_CREDENTIALS`, else `grauwolf.toml` in
  `GRAUWOLF_DATA_DIR`). Reads of `std::env` that remain outside the
  configuration: the OTLP variables (read by the SDK) and the tool build (`CARGO`, `RUSTC`), both
  listed in the module documentation of `grauwolf-host::config`, and `RegistryAuth::from_env` of
  `grauwolf-oci`, which has no caller in the workspace (the host passes `RegistryConfig`, which
  reads through the `Env`, so `GRAUWOLF_REGISTRY_AUTH` and `GIT_TOKEN` can come from a source).
  `grauwolf tools …` (the lean `grauwolf-tools` program) does not apply credential sources: it
  reads the process environment (`ProcessEnv`).
- **Tools:** `grauwolf credentials check <VAR> [--config <file>]` prints `resolved <VAR> from
  <file|openbao|environment> (N bytes)`.
- **Tests:** unit tests for parsing and the file source; `tests/openbao.rs` runs the login, KV
  read, `403` re-login, token reuse and re-login on expiry, refresh and a failing refresh, errors
  without values against a wiremock OpenBao and a fake identity; the host and the CLI test the
  wiring. The automated tests do not use a real OpenBao or SPIRE; `examples/spire-openbao/` is for that (see the live check below).
- **Live check (2026-11-14):** k3s 1.34, SPIRE 1.15.5, trust domain `agentswork.de`, OpenBao 2.7.2.
  The OIDC discovery provider runs in its own namespace `spire-oidc` because spire-controller-manager
  ignores `spire-system`. OpenBao accepts `jwks_url` over plain HTTP inside the cluster (this open
  point is resolved); `bound_issuer` is not set because the SPIRE server has no `jwt_issuer`. The
  image was built in the cluster with kaniko in a temporary namespace and pushed to a private
  registry. The Job ran under the restricted Pod Security Standard and printed
  `resolved ATLASSIAN_API_TOKEN from openbao (32 bytes)` (the length of the random test value).
  Counter-check: the same Job with the service account `default` got the SPIFFE ID
  `spiffe://agentswork.de/ns/agents/sa/default` and OpenBao refused it with `HTTP 400: error
  validating token: invalid subject (sub) claim`; the error contained no value.
- **Start-up race found live and fixed:** a pod that starts before spire-controller-manager has
  created its registration entry and the SPIRE agent has synced it gets `PERMISSION_DENIED`
  (`no identity issued`, gRPC status 7) from the Workload API, which made grauwolf fail at once with
  a cached image. `WorkloadApiClient::jwt_svid` now retries `PERMISSION_DENIED`, `UNAVAILABLE` and
  an unreachable or missing socket with a pause from 200 ms doubling to 5 s until a deadline
  (`SPIFFE_FETCH_TIMEOUT`, default `30s`, `0s` tries once); other errors are not retried, and at
  the deadline the error says how long it waited. Tests: `FakeWorkloadApi::start_denying`
  (denies first, then issues), a socket that appears later, the deadline.
- Not done: dynamic secrets engines (GitHub App tokens), token renewal, `GIT_TOKEN` etc. from
  anything but KV v2 fields, credential gateways.

## Note (2026-11-14): per-agent policies replace the shared policy
The shared policy `grauwolf-agents` of the example (`kv/data/agents/*`) lets every agent read every
agent's secrets. [ADR-0120](0120-grauwolf-k8-ops.md) supersedes it: `grauwolf-k8-ops agent add` creates
a role and a policy `agent-<name>` per agent, bound to the agent's exact SPIFFE ID and limited to
`kv/data/agents/<name>/*`; `grauwolf-k8-ops doctor` warns while the shared role still exists. The
example in `examples/spire-openbao/` stays as documentation of the manual steps.

## Addendum (2026-11-13): placeholder credentials and egress rules derived from the spec
(User, 2026-11-13: "write the addendum into ADR-0110".)

### A. Placeholder credentials in a credential gateway
When an agent's outgoing requests pass a credential gateway (section 4), the agent must never hold
the credential, not even for a moment. The contract between agent, gateway and deployment:
- **Placeholder in the agent:** a credential source `gateway:<name>` (next to `env`, `file`,
  `openbao`) resolves to a fixed placeholder header value (e.g. `Bearer gw-placeholder:<name>`)
  instead of a secret. Tools and clients send it like any token; no secret is in the process, the
  environment, the image or the manifest.
- **Identification by certificate, not by headers:** the gateway identifies the calling agent by
  its X.509-SVID on the connection (ADR-0109) and refuses connections without one; it never trusts
  identity headers, host names or the placeholder for that. One connection carries requests of
  one agent only.
- **Replace only, never add:** the gateway replaces the header only when the request already
  carries the placeholder, only for the destination rule of that agent, and only on connections it
  terminates (TLS re-originated towards the destination; credentials are never put on cleartext
  `http`). The agent trusts the gateway's TLS certificate through the SPIFFE trust bundle or a
  configured CA.
- **Credentials and short-lived tokens:** the value comes from the broker (section 3, keyed by the
  agent's SPIFFE ID and the destination), or the gateway mints a short-lived token that asserts the
  agent's identity for that destination (audience = destination, minutes of validity) where the
  destination accepts it; where the agent calls directly, the JWT-SVID of ADR-0109 serves this.
- **Errors:** broker refuses → `403`, broker unreachable → `503`; the body never contains the
  credential.
- **Scope in grauwolf:** `grauwolf tools serve` follows this contract for the tools it runs (the
  tools live in the gateway, no proxy needed). A general egress proxy is not part of grauwolf for
  now; agents and AgentWeaver rely only on the contract above, so an existing proxy that implements
  it can be used.

### B. Egress rules derived from the spec
Everything an agent may reach follows from its spec and its deployment configuration, so the
network allowlist can be generated instead of written by hand (deny by default):
- **`grauwolf spec egress <spec>`** (CLI, `--format json`) and **`app/egress.json`** written by
  `spec generate --dockerfile` list every destination with `scheme`, `host`, `port`, `purpose`, the
  spec pointer and `phase` (`run` or `build`):
  - model providers of all models in use (main, subgraph, fallback, `set_model`, moderation,
    embedding, `decide` with `llm`), with their default hosts or the host from configuration
    (`OLLAMA_HOST`, `OPENAI_BASE_URL`, …; such entries name the variable as `from_env` and are
    resolved at deploy time);
  - MCP servers over HTTP, remote `agent_tools` (A2A), `fetch.allowed_hosts`, Jira/Confluence
    `base_url`, git `repo_url`, the scion endpoint, the RKEV/System One service, OpenTelemetry, the
    identity service (ADR-0111), OpenBao (section 3) and credential gateways;
  - tool-package registries and git sources of tool packages and skills with `phase: build` – they
    are fetched at generate time and are not needed at run time when embedded or copied.
- **Rules:** host names only (no IP literals unless configured as such), a wildcard only as the
  leftmost label (`*.example.com`), DNS is always allowed. A destination that cannot be derived
  (e.g. a model host only known at run time) is listed with `from_env` and must be resolved by the
  deployment; the command never guesses.
- **Use:** AgentWeaver generates the egress rules for each harness from `app/egress.json` – a
  DNS-aware network policy (plain Kubernetes NetworkPolicies match IPs only, so an FQDN-capable
  policy engine or an egress gateway is needed) or the gateway's destination rules – together with
  the hardening of ADR-0099. Additional destinations are granted explicitly in the deployment, never
  by the agent.
- The derivation is deterministic and tested against the example specs; spec validation does not
  change.


## Decisions for stage 1 (User, 2026-11-15)
- **Q1, `fetch`:** `builtin_tools.fetch.auth` is a table per host whose values have the same form as `auth.spiffe` of ADR-0109 (`mtls`/`jwt`, `id`/`ids`/`trust_domain`, `audience`).
  - Every key must also match an entry of `allowed_hosts`; patterns such as `*.svc` are allowed.
  - mTLS requires `https`.
  - Other hosts are fetched without an identity. The model sees none of this.
- **Q2, git tools:** `builtin_tools.git.auth` uses the same table per host. For such a host, mTLS or a JWT-SVID replaces `GIT_TOKEN`. The git *sources* of tool packages and skills are not included yet: they are resolved at build time, usually outside the cluster.
- **Q3, stdio MCP:** `mcp_servers.<name>.workload_api: true` passes `SPIFFE_ENDPOINT_SOCKET` to the child process.
  - Within the same pod the child gets the agent's identity (SPIRE does not tell processes of a pod apart); the docs say so.
  - The option is allowed for stdio only. Without the socket at run time the error is clear.
- **Q4, `run_command`:** `builtin_tools.command.workload_api: true` passes the socket to the allowlisted programs. Together with shell mode it is a validation error.
- **Q5, test git server:** the smart-HTTP test server of `grauwolf-git` (`serve_folder`) gets a `--spiffe` mode: an mTLS port and a mapping file from SPIFFE ID to git user, including who may push. Tests run it with the fake Workload API. A live check uses an image of it in the owner's cluster:
  - an agent clones and pushes with its identity;
  - an unmapped agent gets 403;
  - `fetch` to another grauwolf agent over mTLS.

## Implementation notes: stage 1 (2026-11-15)
Built as decided above; the details the decisions left open:
- **`fetch.auth` and `git.auth`** are `BTreeMap<host pattern, { spiffe }>` in the spec (`FetchSpec.auth`, `GitSpec.auth`). A `fetch` key must equal an entry of `allowed_hosts` (`AuthHostNotAllowed`); a git key must match the host of `repo_url` (`AuthHostNotRepository`); the `spiffe` rules are those of ADR-0109 (shared check).
- **mTLS with `allow_http`:** not a validation error (`allow_http` is global, one entry may need plain HTTP for an external host). Instead every request to an identity host that is not `https://` is refused at run time, and the same for `jwt` (a bearer must not travel in clear). There is no fall back to plain HTTP. A redirect to a host without an entry carries no identity.
- **`use_token` together with `auth`** is a validation error (`GitTokenWithAuth`).
- **Resolution:** `builtin::resolve_auth` (async) before the synchronous build; without `SPIFFE_ENDPOINT_SOCKET` the error names `builtin_tools.fetch.auth.<host>` or `builtin_tools.git.auth.<host>`. Git JWT-SVIDs are fetched per call in the async part and passed into the blocking transport.
- **stdio MCP:** the child **inherited the whole environment** until now, including `SPIFFE_ENDPOINT_SOCKET` (the table above says "cleared environment", which is true only for `run_command`). `McpClient::spawn` now removes `SPIFFE_ENDPOINT_SOCKET` and `SPIFFE_FETCH_TIMEOUT`; `workload_api: true` adds them back. Other variables are inherited as before (clearing them would break `npx` and the like).
- **`run_command`:** `workload_api` adds the two variables to the allowlisted programs; shell mode is `WorkloadApiWithShell`.
- **Test git server:** `grauwolf-git` serves push (receive-pack) and, with `--spiffe`, mTLS with the mapping `{ "<id or prefix/*>": { "user", "push" } }`; the TLS listener and mapping are a copy of the server's (an adapter must not depend on an app). Image `docker/test-git-server.Dockerfile` (not a release image), manifests in `examples/test-git-server/`.
- **Not done:** git sources of tool packages and skills (Q2); the live check on a cluster (see the final report of the implementation for the steps).

### Live check of stage 1 (2026-11-15, images built from `f9e8035`)
- **Test git server.** It ran in namespace `git-test` with the restricted standard, logging `spiffe: serving as spiffe://agentswork.de/ns/git-test/sa/test-git-server`. Its mapping file gave `writer` push and `reader` read only. Five agents were set up and deployed with the restricted OpenBao token: `writer`, `reader`, `outsider`, `fetcher` and `target`.
- **Git.**
  - `writer` cloned over mTLS with `git.auth`, created a branch, committed and pushed (`Pushed test-writer`).
  - `reader` cloned and committed, and the push was refused with `Push failed: access denied (HTTP 403)`.
  - `outsider`, which is not in the mapping file, got `git clone failed: access denied (HTTP 403)`.
  - With a wrong `id` the clone failed with `TLS handshake with test-git-server.git-test.svc failed: the server is spiffe://agentswork.de/ns/git-test/sa/test-git-server, expected …/sa/somebody-else`.
- **fetch.**
  - `fetcher` read `https://target.target.svc:8443/.well-known/agent-card.json` over mTLS with `fetch.auth`, after `agent allow target --from fetcher`, because the NetworkPolicy admits only callers. It got `HTTP 200`.
  - `http://target.target.svc:8080/health` was refused: `'target.target.svc' needs a workload identity (auth), which only works over https://`.
- **Findings.**
  - The example README named the repository `/team/repo.git`; the server serves `/tools.git`. The README is fixed.
  - `agent deploy --spec` mounts the spec folder read-only. A relative `builtin_tools.root` (`work`) therefore does not exist in the pod, and the agent failed at start with `root of the file tools /app/spec/work: No such file or directory`. The check used `"root": "/data"`, the agent's data volume. **Open point for ADR-0121 (done, 2026-11-15):** `deploy` now places a relative `root` and a `tools_dir` on the data volume at their place under `/app/spec` (ADR-0121, writable folders).
  - The small model sometimes called `git_checkout` with `origin main` instead of `main`, or produced a tool call Ollama could not parse. These are model issues, not tool issues; the spec's prompt names the exact arguments.
- **Not checked live:** `workload_api` for stdio MCP and `run_command` (the CLI image has no test program for it; the tests cover it).
- Everything was removed afterwards: agents, policies, secrets, the `git-test` namespace, the ClusterSPIFFEID, the build namespace and the images. The restricted token was revoked.

## Decisions for stage 6, egress (User, 2026-11-15: "do spec egress and deploy --egress")
- **`grauwolf spec egress <spec> [--format text|json]`** and **`app/egress.json`** (written by `spec generate --dockerfile`) list the destinations as in addendum B, with `phase` and `from_env`. Sub-agents of the agent tree are included, with their spec path.
- **`grauwolf-k8-ops agent deploy --egress`** writes a NetworkPolicy `grauwolf-egress` (policy type `Egress`) for the agent's pods:
  - DNS (UDP and TCP 53) to kube-dns is always allowed.
  - Destinations inside the cluster (`<svc>.<ns>.svc[...]`) are allowed by namespace and port.
  - IP literals are allowed by `ipBlock` /32 and port. This includes `from_env` values that `deploy` knows from `--env` or from its own settings (e.g. `OLLAMA_HOST`, the OpenBao address, the OTLP endpoint).
  - External host names cannot be matched by plain NetworkPolicies. By default they are allowed by port to everything outside the cluster's pod and service ranges, and `deploy` and `doctor` warn which names are only port-restricted. `--egress-strict` leaves them out, so they are blocked.
  - The policy is part of what `deploy` keeps in sync. A redeploy without `--egress` removes it. `agent status` and `doctor` show it.
- **Live check:** an agent reaches its own destinations (model, other agent, OpenBao), and a `fetch` to a host that is not in its spec, or a direct connection attempt, is blocked.

## Implementation notes: stage 6, egress (2026-11-15)
- **Where it lives.** The crate `grauwolf-egress` (adapters) derives the destinations from an `AgentTree` for the CLI and for `grauwolf-k8-ops`, which must not depend on the host. It holds the one table of the providers' default addresses (kept equal to the adapters by a host test) and the resolution of `from_env`.
- **`spec egress`.** `--format text|json`, `--otel` (adds the OTLP endpoint, which no spec says). Entries have `scheme`, `host`, `port`, `purpose`, `pointer` (`<spec file>:<pointer>` for sub-agents), `phase` and `from_env`; equal destinations are merged. An address a variable alone knows has `null` for `scheme`, `host` and `port` (never guessed); an address with a default has the default and names the variable that may replace it (`OLLAMA_HOST`, `OPENAI_BASE_URL`, …; `bedrock`: the default region and `AWS_REGION`, `BEDROCK_BASE_URL` also replaces it).
- **`app/egress.json`** holds all entries, run and build phase, told apart by `phase` (a deployment filters; build entries are not needed at run time). It does not contain the OTLP endpoint.
- **`agent deploy --egress`** needs `--spec`. DNS goes to `k8s-app=kube-dns` in `kube-system` (the k3s labels; `--egress-dns-namespace` and `--egress-dns-selector`). Cluster destinations are allowed by namespace and the **pod** port read from the Service (a missing Service or a named target port: the service port, with a warning). IP literals by `ipBlock`. `localhost` needs no rule. External names: port only to everything outside `--cluster-cidr` (default the k3s ranges), with a warning naming them; `--egress-strict` leaves them out and says they are blocked. Addresses that come from OpenBao or from no `--env` stay unknown: a warning, no rule. The policy carries annotations (`grauwolf.io/egress-destinations`, `-port-restricted`, `-strict`) from which `status` (`egress: <n> destinations, <m> only port-restricted`, or `off`) and `doctor` read their line.
- **Lifecycle.** Updated on every redeploy, deleted by a redeploy without `--egress` and by `undeploy`, removed with the namespace by `agent remove`; not in the pod template, so no restart.
- **Open for the live check:** that the k3s DNS labels and the cluster ranges are as assumed, that a policy engine honours `ipBlock` with `except`, and that the Service target ports match the real pods.

### Live check of stage 6 and its fix (2026-11-15, k3s 1.34 with its built-in network policy controller)
The check ran with `fetcher` (`fetch.allowed_hosts = [target.target.svc, example.com]`, `--egress`) and `target`, a grauwolf agent whose Service has the ports 8080 and 8443 (mTLS).
- **Allowed and reached:** Ollama by IP, OpenBao, `example.com:443`.
- **Blocked** (probe after the start-up window): `example.com:80`, `lgtm:3000`, `scion-server:7443`.
- **Port-only:** `1.1.1.1:443` was reached, as documented.
- A pod without the agent label is unrestricted.
- **The k3s assumptions held:** `k8s-app=kube-dns` in `kube-system`, the ranges `10.42.0.0/16` and `10.43.0.0/16`, and `ipBlock` with `except` works.
- **Start-up window.** A new pod was unrestricted for a few seconds until the policy controller had applied the rules: a probe started right away reached `example.com:80`, Grafana and scion; after 20 s it was blocked. NetworkPolicies are applied asynchronously by the CNI or policy controller; this is not a grauwolf property and is documented as a limit (user guide 13, developer guide 9).
- **Finding 1 (bug): `fetch` hosts inside the cluster.** `fetch.allowed_hosts` has no ports, so the derivation listed `https://target.target.svc:443`; the policy allowed namespace `target` on 443, `deploy` warned that the Service has no port 443, and a real `fetch` of `https://target.target.svc:8443/...` was blocked.
  - **Fix:** a cluster host in `fetch.allowed_hosts` (`<svc>.<ns>.svc[...]`, `*.<ns>.svc`) has no fixed port: `Destination.port` is `None` and the new `any_port` is `true` (`spec egress` prints `https://target.target.svc:*`; JSON has `"any_port": true`, also in `app/egress.json`, `false` otherwise). External `fetch` hosts keep 443 (and 80 with `allow_http`); another port of an external host is not covered, and the docs say so. A2A, MCP, git and the other URLs carry explicit ports or their scheme's default, so they are unaffected.
  - **Deploy:** an "any port" destination is allowed on the pod ports of all ports of its Service (for `target`: namespace `target`, TCP 8080 and 8443). A wildcard (`*.<ns>.svc`) allows the namespace without a port restriction (simpler than all ports of all Services, and it does not go stale when Services change; `*.svc` is every namespace). A Service that is not there (or names its target port) allows the namespace without ports as well, with a warning: the spec names the host, so allowing nothing would break the `fetch`, and a redeploy narrows the rule once the Service exists.

- **Finding 2 (live, after the fix): named target ports.** The Services `agent deploy` writes name their target ports (`http`, `mtls`). A `fetch` host that is another agent therefore fell back to the whole namespace without a port restriction. **Fixed:** a named target port is resolved through the container ports of the Deployments in the namespace whose pod labels match the Service's selector. Only a name nobody defines falls back, with a warning. Test: `named_target_ports_are_resolved_through_the_deployment_the_service_selects`.
- **Live result after both fixes:**
  - The agent `fetcher` fetched `https://target.target.svc:8443/.well-known/agent-card.json` with `HTTP 200`.
  - A probe with the agent's label reached `target:8443` at the network level; curl's own TLS failed for lack of a client certificate, as intended.
  - `example.com:80`, `lgtm:3000` and `scion-server:7443` stayed blocked.
  - `--egress-strict` also blocked `example.com:443` and `1.1.1.1:443`, and `doctor` reported the strict mode.
