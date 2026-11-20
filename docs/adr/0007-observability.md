# ADR-0007: Domain events plus tracing/OpenTelemetry

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
Agent runs should be observable through typed events that features can hook into. Rust has an observability standard in `tracing`.

## Decision
`grauwolf-agents` defines typed domain events (`AgentEvent`) and the pipeline. Export to `tracing`, JSONL, OpenTelemetry (Langfuse, Weave, Datadog) is handled by adapters.

## Consequences
Events can be used for tests, persistence and debuggers and are independent of export targets.
