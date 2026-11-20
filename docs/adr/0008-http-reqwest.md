# ADR-0008: HTTP exclusively via reqwest

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
The provider adapters need an HTTP client; interchangeable HTTP clients would be extra effort without clear benefit.

## Decision
Only `reqwest`, encapsulated in `grauwolf-llm-http`. No interchangeable HTTP client; tests mock at the HTTP level (fixtures/wiremock).

## Consequences
Less abstraction, less code.
