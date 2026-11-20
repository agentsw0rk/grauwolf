# ADR-0068: User tokens in the HTTP service

- **Status:** Accepted (User, 2026-11-08: "yes, do the user tokens")
- **Date:** 2026-11-08

## Context
`spec serve` knows a shared token (`GRAUWOLF_SERVER_TOKEN`, ADR-0053). Long-term memory (ADR-0059), conversations (ADR-0060) and approvals (ADR-0061) depend on the `user_id` in the request body, which the service trusts. Anyone who knows the shared token can impersonate any user. For clients that use end users directly (e.g., a web interface), the service must determine the user itself.

## Decision
- User file (JSON): `{ "<user>": "sha256:<hex>" }`; only the SHA-256 hash of the token is stored. Tokens are 128 bits of randomness (`gw_` plus 32 hex characters); at this entropy, an unsalted hash is sufficient.
- `grauwolf token <user>` generates a token and prints the token and the line for the file.
- `spec serve --users <file>`: a request with `Authorization: Bearer <user-token>` runs as this user; the `user_id` is set from it. A differing `user_id` in the body (also with `POST /approvals/{id}`) yields `403`. An unknown token yields `401`.
- The shared token remains the token of a trusted backend and may specify any `user_id`.
- Comparison in constant time via the hashes; `/health` and `/.well-known/…` remain open.
- Without a user file and without a shared token, the existing rule still applies: only local addresses.

## Consequences
- End users can use the service directly without reaching other users' memory, conversations or approvals.
- A2A and MCP run with any valid token, but without a user association (as before, without `user_id`).
- Tokens can only be managed via the file; changes take effect after a restart of the service.

## Addendum (2026-11-09): A2A with user tokens
"A2A … without a user association" no longer applies: with a user token, A2A requests belong to the token's user. `contextId` becomes that user's conversation, and `GetTask`/`CancelTask` find only the user's own tasks (addendum in ADR-0054). Memory under A2A continues to use the shared scope; MCP continues to run without a user association.
