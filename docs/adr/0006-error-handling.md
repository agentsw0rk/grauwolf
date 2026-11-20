# ADR-0006: Error handling with thiserror and Result

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
Library code should report errors in a typed way, without panics.

## Decision
Each crate has its own `thiserror` error enums; public APIs return `Result`. No `unwrap`/`expect` in library code outside of tests.

## Consequences
Precise errors, but conversions between crate errors are necessary.
