# ADR-0035: Builder agent creates and repairs specs

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
Core feature E13: a working agent should be created from a request in text form. An LLM writes an AgentSpec with tool code for this. LLMs regularly make mistakes: invalid JSON, unknown node kinds, type errors on edges, tool code that does not compile. grauwolf already reports all of these errors with JSON pointers (F13.5, F13.9) – exactly what an LLM needs to correct them specifically.

## Decision
- New domain crate `grauwolf-builder-agent`. The builder is itself a GraphAgent on the engine (dogfooding):
  `start → draft → check → (valid) finish`, `check → (errors, attempts left) repair → check`, `check → (no attempts left) give_up`.
- `draft` asks the LLM with a system prompt that contains the JSON schema of the DSL (F13.2), the node and condition kinds with signatures from the catalogs, the rules for tool code (tool ABI v1, forbidden constructs), and an example spec.
- `check` reads the response (code block allowed), loads it strictly (`AgentSpec::from_json`), validates it (`validate`), and then asks a port `SpecChecker` (e.g., to compile tool code). All problems are passed on as `SpecIssue { path, message }` with JSON pointers.
- `repair` sends the problems back as a user message and requests the complete corrected spec. The number of repairs is limited (default 3); after that the run fails with the last problems.
- The port `SpecChecker` lives in the domain; the facade provides an implementation that builds the tools with `grauwolf-toolbuild` (compile errors with path `/tools/<i>/code` and line).
- CLI: `grauwolf spec create "<request>" --model <provider>:<model> --out <file>`.

## Consequences
- The builder automatically benefits from every new check in validation or build.
- Results are not deterministic; tests run against a mock, and quality is shown by a live run with real models.
- The system prompt grows with catalogs and schema; with small models this may hit the limits of the context window.
