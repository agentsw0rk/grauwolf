# ADR-0125: Grant consent through the identity service as a confidential client

- **Status:** Proposed (User, 2026-11-16: "Q2 a passt, bitte trag b als ADR später ein")
- **Date:** 2026-11-16
- **Related:** ADR-0111 (delegation with transaction tokens; stage 3 grants), ADR-0110 (OpenBao), ADR-0092 (AgentWeaver).

## Context
For stage 3 of ADR-0111 (grants), the owner chose this flow for now (Q2 a):
1. `grauwolf identity grant create` runs a separate device code sign-in of the public CLI client with `offline_access`.
2. The CLI hands the refresh token to the identity service over HTTPS, with the person's access token as authorization.
3. The service stores the token in OpenBao and later redeems it as that public client.

This works with Entra Local and needs no ingress, but it has drawbacks:
- the refresh token passes through the person's machine;
- it is bound to a public client that anyone with its client ID can use;
- consent happens in a terminal, not in a page that names the agent, the trigger and the purposes.

## Decision (proposed, for a later stage)
**The identity service obtains the refresh token itself, as a confidential Entra client:**
- **Starting a grant:** the person starts it from a consent page, either a minimal page of the identity service or AgentWeaver.
- **Sign-in:** an authorization code flow with PKCE and `offline_access`, redirected to the identity service's callback (`/api/identity/grants/callback`). The page shows what the grant allows: agent, trigger, purposes, validity.
- **Client authentication:** the service authenticates to Entra without a client secret if possible, through workload identity federation with its SPIFFE JWT-SVID (an open point of ADR-0111), else with a certificate or a secret from OpenBao.
- **The refresh token never leaves the service.** It goes straight to OpenBao, as in stage 3.
- **The device code flow of Q2 a stays** for headless setups and tests.

## Consequences
- The identity service needs a public callback URL (ingress) and a small UI, or AgentWeaver integration.
- A real Entra ID (or an Entra Local version with confidential-client redirect flows) is needed for a live check.
- Grants made the Q2 a way keep working; they can be re-created through the new flow.
