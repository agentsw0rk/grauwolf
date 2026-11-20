# ADR-0127: MCP authorization, tool access rules and a standard token exchange at the boundary

- **Status:** Accepted (User, 2026-11-17: "make an ADR from the findings" of a review of how agent gateways handle authentication; Q1–Q5 confirmed as recommended)
- **Date:** 2026-11-17
- **Related:** ADR-0109 (SPIFFE between agents), ADR-0110 (workload identity for tools, the tool gateway of its section 4), ADR-0111 (delegation with Txn-Tokens; stage 2 "boundary"), ADR-0125 (grant consent), ADR-0068 (per-user tokens).

## Context
grauwolf secures agents from the inside: every agent verifies its callers itself.
- **Workload identity:** SPIFFE mTLS or JWT-SVID, a callers file, NetworkPolicies (ADR-0109); tools with mTLS or JWT-SVID, credentials from OpenBao (ADR-0110).
- **Acting on behalf of a person:** Txn-Tokens through whole agent chains, issued by the identity service and signed by OpenBao Transit, plus grants for unattended runs (ADR-0111).

A review of how agent gateways approach authentication showed this is ahead in workload identity, delegation chains and unattended runs. The same review showed three gaps where grauwolf meets the outside world:

1. **MCP clients cannot sign in to a grauwolf MCP server the standard way.**
   - An agent served as MCP server (`spec serve`, generated services) accepts a shared or per-user token, or SPIFFE.
   - The MCP authorization specification (OAuth 2.1 with Protected Resource Metadata, RFC 9728, and `WWW-Authenticate` discovery) is not supported. Clients such as desktop assistants therefore cannot discover where to sign in.
2. **There are no rules per tool.** Who may call which tool is decided only by:
   - approvals;
   - the tool policy for packages (which code may run, not who may call it);
   - `requires_delegation` and per-route rights, both planned and not built.

   Rules such as "only members of a group may call `jira_create_issue`" or "this MCP tool only on a grant with purpose `report`" cannot be expressed. An MCP server built from a spec lists all its tools to every caller.
3. **The boundary to external services is not built.** ADR-0111 stage 2 (`/exchange`) was planned for Entra On-Behalf-Of and Atlassian only. Standard token exchange covers these and more with one mechanism:
   - RFC 8693, with an actor token;
   - RFC 7523 JWT bearer;
   - RFC 8707 resource indicators;
   - a cache.

## Decision
### 1. MCP authorization for grauwolf MCP servers
An agent served over MCP (Streamable HTTP and HTTP+SSE) can act as an **OAuth-protected resource** per the MCP authorization specification:
- **Discovery:** `GET /.well-known/oauth-protected-resource` (RFC 9728) names the resource, the authorization server(s) (the Entra issuer configured for ADR-0111) and the scopes. A request without a valid token gets `401` with `WWW-Authenticate: Bearer resource_metadata="…"`.
- **Tokens:** a person's access token is verified as at the ADR-0111 entry (issuer, audience, signature, expiry) and exchanged for a Txn-Token, so the MCP run is on behalf of that person with the same events, spans and approvals.
- **Client registration:** grauwolf does **not** broker dynamic client registration. Entra ID has none, and brokering it means writing to the identity provider. Clients use a pre-registered client (the "grauwolf CLI" or a dedicated public client); client ID metadata documents can follow once the specification settles.
- **Configuration:** the existing `GRAUWOLF_IDENTITY_*` settings plus the resource's public URL. SPIFFE and the existing token modes keep working beside it.

### 2. Tool access rules in the spec
A new optional section `tool_access` with ordered rules that allow or deny calls of tools by name or pattern:
- spec tools;
- built-in tools;
- package tools;
- `agent_tools`;
- MCP tools as `<server>/<tool>`.

**Conditions** may refer to:
- the **caller**: SPIFFE ID or prefix, or the user of a per-user token;
- the **person**: `oid`, and group claims when the identity service passes them on;
- **delegation present or not** (this subsumes `requires_delegation` of ADR-0111);
- the **purpose** (`purp`) and the **grant** (`azd.grant`);
- **constant arguments** where cheap (e.g. a host of `fetch`).

**Effects:**
- A denied tool is **not offered** to the model and is **left out of `tools/list`** when the agent serves MCP.
- A call that slips through (e.g. arguments) is refused with a clear tool error and an event.
- Rules are checked at run start and per call, the same way in the interpreter and in generated crates.
- **Expression form:** a small declarative matcher in the spec DSL (fields with exact values, prefixes and lists), validated by `spec validate`, **not a general expression language**. If that proves too narrow, a later ADR may add one.

### 3. A standard exchange at the boundary (ADR-0111 stage 2, generalised)
The identity service's `POST /api/identity/exchange` gets **targets** configured in its policy file. Each target names:
- **the grant type:**
  - **RFC 8693 token exchange:** subject is the person's token or the Txn-Token; **actor token** is the calling agent's JWT-SVID, so the target sees person and agent;
  - **RFC 7523 JWT bearer**, which covers Entra On-Behalf-Of;
  - **stored user credentials:** the person's OAuth token connected through `/connections` (Atlassian), refreshed from OpenBao;
- the token endpoint;
- the client authentication: `private_key_jwt` with the service's JWT-SVID where the authorization server trusts SPIRE's OIDC issuer, else a secret from OpenBao;
- audiences, scopes and RFC 8707 resources;
- which agents (SPIFFE IDs) and purposes may use it.

**Results are cached** per person, agent and target until shortly before they expire. Agents ask for a target by name. Built-in tools (Jira, Confluence, git, `fetch` with an `exchange` auth mode) and MCP clients use the result as their bearer. The token never reaches the model, logs or events (as for Txn-Tokens).

### 4. External gateways instead of an own tool gateway
The tool gateway of ADR-0110 section 4 is not built now. Agents placed behind an external agent gateway keep working:
- they authenticate to it with their workload identity;
- they pass the Txn-Token in the `Txn-Token` header, which a gateway can verify against the identity service's JWKS.

The developer guide describes that setup. A grauwolf tool gateway is reconsidered only if a need appears that rules (2) and the exchange (3) inside agents cannot cover.

## Stages
1. **Tool access rules (2).** Smallest step, no external dependency; includes `requires_delegation`.
2. **MCP authorization (1).** Builds on the ADR-0111 entry; live check with an MCP client against Entra Local.
3. **Boundary exchange (3)** with RFC 8693 + actor token and RFC 7523 first, live check against Entra Local or Keycloak in the cluster; Atlassian connections when an Atlassian OAuth app exists.
4. **Documentation of (4).**

## Consequences
- **A grauwolf MCP server** becomes usable by standard MCP clients with a person's sign-in, and that person's identity flows through the agent chain.
- **Who may call which tool** is visible in the spec, checked by `spec validate`, the same in interpreter and generated crates, and visible in the studio.
- **One exchange mechanism** replaces target-specific code for external APIs. The agent stays visible to the target as actor, which keeps the audit chain of ADR-0111 beyond grauwolf.
- **More configuration in the identity service.** Its policy file grows (targets), and its OpenBao role needs the secrets of targets that do not accept workload identity.

## Decisions of the owner (2026-11-17)
- **Q1:** stage order as above: tool access rules, then MCP authorization, then the boundary exchange, then the documentation of external gateways.
- **Q2:** declarative matchers in the spec DSL, no general expression language.
- **Q3:** no brokering of dynamic client registration; pre-registered clients.
- **Q4:** the person's group claims are passed into Txn-Tokens (from Entra group claims, or a lookup where the token lacks them), so rules can refer to groups.
- **Q5:** no own tool gateway for now; external agent gateways are documented instead.
