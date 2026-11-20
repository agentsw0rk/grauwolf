# grauwolf-test

**Layer:** `support` · **Status:** `MockLlmClient` (rules match the text of the last response – if not empty, e.g. a thought in ReAct – and of all following user messages and tool results, joined line by line, without system messages; the first matching rule wins; a response, a tool call, or with `call_tools_when_contains` several tool calls in one response with IDs `<tool>_<n>`; default response); `conformance::check_llm_client(client, model)` checks text, tool call, response after a tool result, structured response, and streaming (error-free, ends with `End`, aggregated text) and returns the failures (worded tolerantly for real models)

## Purpose
Test kit: `MockLlmClient`, the conformance suite for `LlmClient` (`conformance::check_llm_client`) and `TestDir`. **Goal:** MockTool, event recorder, test clock, conformance suite for `ToolRuntime`.

## Features / ADRs
E12 (F12.1–F12.7)

## Allowed dependencies
- grauwolf crates: grauwolf-prompt, grauwolf-llm (planned: grauwolf-tools, grauwolf-agents)
- External: futures-util, serde_json; dev: futures-executor

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- The only support crate that domain crates may depend on (ADR-0016).
- Domain crates use it only in integration tests (`tests/`), never in unit tests.
- Conformance suites are generic over the port trait.
- `TestDir::new(label)` is a unique temporary directory (`grauwolf-<label>-<pid>-<n>`) that is removed recursively on drop (`path()`, `join()`, `Deref<Target = Path>`). Tests use it for every temporary directory instead of building paths in the system temp dir; keep the guard alive for the whole test (also for background tasks), and use one guard where two instances must share a directory.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-test`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
