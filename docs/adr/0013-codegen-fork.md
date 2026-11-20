# ADR-0013: Fork of carllerche/codegen as grauwolf-codegen

- **Status:** Accepted (User, 2026-11-06)
- **Date:** 2026-11-06

## Context
Generative agents need Rust code generation. `codegen` has a suitable builder API, but bodies as strings, only basic formatting, gaps in items and no source map; it is hardly maintained.

## Decision
Fork as `grauwolf-codegen` (MIT), extended with `syn` validation of code strings, `prettyplease` formatting, missing items and a source map. The crate stays generic (no knowledge of agents); the domain mapping spec → code lives in `grauwolf-generator`.

## Consequences
Improvements can go upstream. The fork has to be maintained by us.
