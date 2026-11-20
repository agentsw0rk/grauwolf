# grauwolf-builder-agent

**Layer:** `domain` · **Status:** `Builder::new(executor, model).build(requirement)` → `BuiltSpec { spec, json, repairs }`; `draft`/`repair` request the output structured per the DSL's JSON schema (`AgentSpec`, the providers' native strategy ⇒ syntactically valid JSON; `nodes` and `edges` of a `graph` are required in the builder schema (`graph` itself is optional, so a `strategy` is possible), because models omit optional fields under forced output); the builder is a GraphAgent (`draft → check → finish`, `check → repair → check`, `check → give_up`); `check` loads strictly and validates, problems are reported as `SpecIssue { path, message }`; `repair` sends them back as `<path>: <error>`; JSON syntax errors include the lines around the error position; doubly escaped line breaks in tool code (`\\n` without a real line break, a quirk of smaller models such as qwen3.5:9b) are corrected before checking; after `with_max_repairs(n)` (default 3) → `BuilderError::GaveUp { repairs, issues }`. `with_agent_model(model)` binds the model of the generated agent (also named in the prompt). Port `SpecChecker` (`with_checker`) additionally checks valid specs, e.g. by compiling; its problems likewise go into the repair. The system prompt (`prompt.rs`) is generated from the built-in catalogs: construction rules, all node kinds and conditions with their signatures, the node kinds `subgraph`, `plan` (planner, ADR-0039) and `parallel` (ADR-0045), rules for tool code, proven patterns (without tools, `single_run` strategy as the simplest tool loop, hand-written tool cycle including the `on_multiple_tool_calls` branch) with an explanation of the data flow, the DSL's JSON schema, and `calculator.json` as an example.

## Purpose
Builder agent (F13.12): generates a valid AgentSpec from a textual request and repairs it using the errors, with JSON pointers.

## Features / ADRs
E13 F13.12 (uses F13.2, F13.5, F13.9), ADR-0035, ADR-0039, ADR-0045

## Allowed dependencies
- grauwolf crates: grauwolf-agents, grauwolf-graph, grauwolf-llm, grauwolf-prompt, grauwolf-spec; dev: grauwolf-test
- External: futures-util, serde, serde_json, thiserror; dev: futures-executor

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- The builder uses the framework's own engine (dogfooding); no separate loop alongside the graph.
- Every problem sent back to the LLM has a JSON pointer (empty = the whole spec).
- New catalog kinds appear in the prompt automatically; the rules for tool code (forbidden constructs) mirror `grauwolf-toolbuild` and must be updated there together with any change.
- No building of tool code in the domain: compiling runs via the port `SpecChecker`, with the adapter in the facade.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-builder-agent`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
