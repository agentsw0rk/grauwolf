# ADR-0085: README in English, German version alongside

- **Status:** Accepted (User, 2026-11-09: README.md in English with a layout customary on GitHub)
- **Date:** 2026-11-09

## Context
Documentation and comments are in German (CLAUDE.md). But the README is the landing page on GitHub; whoever finds the project there expects English and a familiar layout.

## Decision
- `README.md` is in English and follows a layout customary on GitHub: title with badges and useful links, Overview (key features, providers, quickstart), Using in your projects (requirements, command line, Cargo, generated crates), Development, Versioning, Contributing, License, Support.
- The previous German version remains as `README.de.md`; both link to each other.
- Everything else (handbooks, ADRs, comments) remains German.
- The Rust quickstart example has been compiled against the current state and run live.

## Consequences
- New features must be added to both READMEs.

## Addendum (2026-11-10)
`README.de.md` has been dropped (ADR-0090, addendum "Only one README"): there is only the English `README.md` now; it links the English and the German handbooks. New features are added only there.
