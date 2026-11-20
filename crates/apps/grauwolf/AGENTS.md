# grauwolf

**Layer:** `apps` · **Status:** Feature `bedrock` (default) re-exports `grauwolf_bedrock` as `bedrock` and enables `grauwolf-host/bedrock`. The public API (ADR-0108, stages 4a and 4c). Re-exports the domain crates (`agents`, `graph`, `interpreter`, `llm`, `prompt`, `rag`, `spec`, `toolbuild`, `tools`, `builder_agent`), the adapters per Cargo feature (`ollama`, `openai`, `anthropic`, `gemini`, `sandbox`, `toolpkg`), the macros `tool`/`toolset` (feature `macros`; `grauwolf_macros::facade_*`, generated code uses `::grauwolf::tools`, ADR-0003 addendum) and every module of the composition root [`grauwolf-host`](../grauwolf-host/AGENTS.md) under the same path (`app`, `attachments`, `builder`, `builtin`, `cache`, `checkpoints`, `config`, `env`, `external`, `features`, `knowledge`, `loader`, `long_term_memory`, `mcp`, `memory`, `otel`, `packages`, `providers`, `runtime`, `self_tools`, `systemone`, `tools_dir`, `spec_schema`); their documentation is in the AGENTS.md of the host. The Cargo features forward to the host's features of the same name (all on by default). `chat_api`, `observations` and (feature `service`) `service` are re-exported from [`grauwolf-server`](../grauwolf-server/AGENTS.md) (stage 4c; documentation there). The facade has no code of its own beyond `lib.rs`; `cargo xtask check-arch` enforces this (no item keywords such as `fn`, `struct`, `impl` in `src/`). No other crate in the workspace depends on it, the CLI included (ADR-0108 stage 4d).
## Purpose
Facade: the public API of grauwolf (re-exports of the crates and of the host's modules via Cargo features), of the host and of the server (`chat_api`, `observations`, `service`).

## Features / ADRs
E11 F11.8, ADR-0003, ADR-0093, ADR-0094, ADR-0108

## Allowed dependencies
- grauwolf crates: domain crates (re-exports), grauwolf-host (always), adapters optional only for their re-export (`ollama`, `openai`, `anthropic`, `gemini`, `sandbox`, `toolpkg`), grauwolf-macros optional (feature `macros`), grauwolf-server (always; feature `service` switches its `service` module on, `ollama` forwards)
- External: serde_json; dev: tokio

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- No composition logic of its own: only re-exports. New composition code belongs into `grauwolf-host`, HTTP code into `grauwolf-server`.
- Module paths of the re-exports stay stable (generator templates, CLI, server and users rely on them).

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
