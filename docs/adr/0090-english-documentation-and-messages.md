# ADR-0090: Handbooks and messages in English

- **Status:** Accepted (User, 2026-11-10: "the things in German, like the user handbook and developer one, can go English"; English replaces the German handbooks, examples are translated too, and messages of the CLI and library likewise)
- **Date:** 2026-11-10

## Context
The README is in English (ADR-0085), while the handbooks, their examples and all messages of the CLI and library are in German. For an audience on GitHub this is a hurdle; handbooks that quote German messages would be inconsistent in English.

## Decision
- **Handbooks:** `docs/handbuch/` → `docs/user-guide/`, `docs/entwickler/` → `docs/developer-guide/`, both in English; the German versions are dropped. Chapter and example names become English (mapping in the commit of the switch-over).
- **Examples:** `docs/user-guide/examples/` with English system prompts, tool descriptions, knowledge documents and names; they remain tested.
- **Messages:** error messages, validation errors, CLI output, error texts of the service and tool results for the model (e.g., "unknown tool") are English.
- **Remains German for now:** code comments and doc comments, ADRs, the crates' `AGENTS.md`/`CLAUDE.md`, the roadmap, and the built-in instructions to the model (compression, facts, moderation; they are tuned live, and a change requires its own live tests).

## Consequences
- Tests that check messages change along with them.
- Quotes of messages in the handbooks are checked against the code.

## Addendum (2026-11-10): German versions as an alternative
User: "could you put the German things next to it before the commits and also link them in the readme.md as an alternative?" The German handbooks are again under `docs/handbuch/` (with `beispiele/`) and `docs/entwickler/`, as they were before the switch-over, with a note on their state and the maintained English version. `README.md` links both versions. The German example specs continue to be tested for validity. Quotes of messages in them show the earlier German wording.

## Addendum (2026-11-10): Only one README
User: "I don't need README.de.md; there should be one readme.md, which should be in English and link to the English and German handbooks and developer guide." `README.de.md` is dropped; `README.md` (English) links the English and the German handbooks. This replaces the German README version in ADR-0085.
