# ADR-0009: Web integration with axum

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
Agents should be embeddable in web services.

## Decision
Web integration (agent endpoints, SSE streaming, A2A server, AgentCore runtime) is done with `axum` in adapter crates.

## Consequences
No auto-configuration in the style of large web frameworks; configuration via file/env (figment).
