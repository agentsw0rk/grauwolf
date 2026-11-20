# ADR-0060: Binding conversations to users

- **Status:** Accepted (User, 2026-11-08: "keep going with binding conversations to users")
- **Date:** 2026-11-08

## Context
Since ADR-0059, `user_id` separates the long-term memory, but conversation histories (ADR-0037) still lie only under `conversation_id`. Whoever knows or guesses the ID of someone else's conversation reads and continues it – even with their own `user_id`. The SQL store accepts arbitrary IDs, the file store only `[A-Za-z0-9_-]`.

## Decision
- The storage key is formed by `grauwolf_agents::memory::conversation_key(user, id)`: without a user, `id`; with a user, `users/<hex(user)>/<id>`. The user ID is hex-encoded so that arbitrary IDs (e.g., email addresses) are allowed.
- Conversation IDs from outside may contain only `[A-Za-z0-9_-]` (as before in the file store); user IDs are not empty and at most 128 bytes long. Because `/` is forbidden in IDs, no key without a user can equal one with a user – not even via A2A (`contextId`).
- The file store puts conversations with a user under `<dir>/users/<hex>/<id>.json`; the SQL store stores the key unchanged.
- The HTTP service (`user_id` + `conversation_id`) and the CLI (`--user` + `--conversation`) use the same key; invalid IDs are errors (HTTP 400).

## Consequences
- The same `conversation_id` is a separate conversation for each user; without `user_id` everything remains as before (existing conversations without a user remain reachable).
- IDs with other characters that the SQL store previously accepted are now rejected.
- Authentication remains the caller's responsibility (ADR-0059).
