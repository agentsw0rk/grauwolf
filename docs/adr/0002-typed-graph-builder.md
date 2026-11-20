# ADR-0002: Typed graph builder instead of a macro DSL

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
Agent graphs should be built in code and checked for matching types at compile time. Alternatives: builder, proc-macro DSL, enum state machine.

## Decision
Graphs are built with a typed builder using `NodeHandle<I, O>`. Edge conditions are filter maps (`Message -> Option<ToolCall>`) that combine condition and type conversion. No graph macro.

## Consequences
Edges are type-checked at compile time, good IDE support. Somewhat more typing than a macro DSL. The builder is a facade over the engine from ADR-0017; edge conditions in JSON come from the condition catalog (ADR-0021).
