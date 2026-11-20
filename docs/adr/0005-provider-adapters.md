# ADR-0005: LLM providers as separate adapter crates with their own DTOs

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
Provider details (cache control, reasoning, tool formats) should be fully controlled. Initially a single crate with Cargo features was planned.

## Decision
The port `LlmClient` lives in `grauwolf-llm` (domain, without HTTP). Each provider is an adapter crate (`grauwolf-openai`, `-anthropic`, `-ollama`, …) with its own DTOs on the shared base `grauwolf-llm-http`. No third-party crates such as async-openai, genai or rig. The facade bundles providers via Cargo features.

## Consequences
The domain stays IO-free; providers can be developed and tested independently. More code for DTOs; every adapter crate must pass the LlmClient conformance suite.
