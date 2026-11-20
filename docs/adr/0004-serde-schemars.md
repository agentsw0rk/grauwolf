# ADR-0004: Serialization with serde, schemas with schemars

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
Prompts, tool arguments, specs, checkpoints and structured output need (de)serialization and JSON schemas.

## Decision
`serde`/`serde_json` for all data, `schemars` for JSON schemas (tool arguments, structured output, AgentSpec).

## Consequences
De facto standard; no interchangeable serialization layer.
