# Governance

This document describes how grauwolf is run: who decides, how decisions are made and recorded, and
how contributors become maintainers.

## Principles

- **Open:** discussions, decisions and their reasons are public – in issues, pull requests and the
  architecture decision records ([docs/adr/](docs/adr/README.md)).
- **Documented decisions:** every design decision that changes how grauwolf works, what it depends
  on, or what users see is recorded as an ADR before or with the change.
- **Quality gates for everyone:** every change goes through the same checks (formatting, clippy with
  warnings as errors, the full test suite, the architecture test), whoever makes it.

## Roles

- **Users** use grauwolf and report issues.
- **Contributors** open issues, review, write documentation or code. Anyone can contribute; see
  [CONTRIBUTING.md](CONTRIBUTING.md).
- **Maintainers** ([MAINTAINERS.md](MAINTAINERS.md)) review and merge pull requests, triage issues,
  decide on ADRs, cut releases and enforce the [code of conduct](CODE_OF_CONDUCT.md).
- **Lead maintainer:** while the project has few maintainers, the lead maintainer decides when
  maintainers do not reach consensus. This role is meant to end once the project has at least three
  maintainers from more than one organisation; decisions then follow the voting rules below only.

## Decision making

1. **Lazy consensus** for everyday changes: a pull request that passes the checks and has the
   approval of one maintainer (not its author, once there is more than one maintainer) can be merged
   if no maintainer objects within a reasonable time.
2. **ADRs** for design decisions: an ADR is proposed in a pull request with status *Proposed*,
   discussed, and accepted when the maintainers agree. Accepted ADRs are changed only by a new ADR
   that supersedes them.
3. **Voting** when consensus is not reached: a simple majority of maintainers decides; changes to
   this governance, adding or removing a maintainer, and changes of the license need a two-thirds
   majority. Votes are held in a public issue and stay open for at least seven days.

## Becoming a maintainer

A contributor can become a maintainer after sustained, high-quality contributions – code, reviews,
documentation or community work – over several months. An existing maintainer nominates the
contributor in a public issue; the maintainers decide by the voting rules above. New maintainers
are added to [MAINTAINERS.md](MAINTAINERS.md) with their areas.

Maintainers who are no longer active for six months, or who ask for it, move to the emeritus list.
A maintainer can be removed by a two-thirds vote of the other maintainers, e.g. for repeated
violations of the code of conduct.

## Releases

Maintainers cut releases from `main` once the full checks pass. Versions follow semantic versioning
from 1.0 on; before that, APIs may change between releases (see the README, *Versioning*). Release
notes name user-visible changes and the ADRs behind them.

## Code of conduct and security

The project follows the [CNCF Code of Conduct](CODE_OF_CONDUCT.md). Security issues are handled as
described in [SECURITY.md](SECURITY.md).

## Changes to this document

Changes to this governance are proposed in a pull request and need a two-thirds majority of the
maintainers.
