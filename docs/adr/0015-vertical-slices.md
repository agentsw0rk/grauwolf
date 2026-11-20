# ADR-0015: Vertical slices, spec-first

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
Spec, runtime, interpreter and generator depend on each other and should stay congruent.

## Decision
Development in vertical slices through all layers. Each slice starts at the spec and ends with a runnable agent created from JSON **in both modes** (interpreter and generator, equivalence test). That is why the generator and the codegen fork already belong to slice 1. Single source for order and content of the slices: `docs/architecture.md`, section 5.

## Consequences
Runnable results early; the spec is not designed past the runtime.
