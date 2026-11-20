# grauwolf – notes for AI agents

General information (purpose, setup, commands): see [README.md](README.md) (the only README; links the English and German handbooks).

## Orientation
- Handbooks (ADR-0091): [User guide](docs/user-guide/README.md), [Developer guide](docs/developer-guide/README.md) – keep them up to date with new features; the example specs in `docs/user-guide/examples/` are tested. German versions `docs/handbuch/` and `docs/entwickler/` as an alternative, translated completely from the English state of 2026-11-15 (their examples are tested as well; update only when needed, by translating the changed English chapters again).
- Architecture and layers: [docs/architecture.md](docs/architecture.md)
- Decisions: [docs/adr/](docs/adr/README.md) – read before design changes; record new decisions as an ADR
- Feature catalog with IDs (`F13.5` …): [docs/features/](docs/features/README.md)
- Local progress notes (not in git): `.claude/tasks/`
- Every crate has its own `AGENTS.md` with purpose, allowed dependencies and invariants – read it before working on a crate.

## Structure
```
crates/domain/    domain core, defines ports, no IO dependencies
crates/adapters/  implement ports (HTTP, wasmtime, …)
crates/support/   generic (codegen fork) and test kit
crates/apps/      facade, CLI, the lean programs `grauwolf-tools` and `grauwolf-k8-ops` (ADR-0118, ADR-0120) and the studio backend `grauwolf-studio` (ADR-0126), compose adapters
studio/web/       frontend of grauwolf studio (Next.js static export; `npm run lint|test|check-api|build`), see studio/README.md
xtask/            development tools (`cargo xtask check-arch`)
```

## Rules
- Project name always lowercase: `grauwolf` (also at the start of a sentence, ADR-0012).
- Docs, ADRs and READMEs do not refer to any other framework as a template (User, 2026-11-10).
- Dependency direction according to [ADR-0016](docs/adr/0016-layered-architecture.md); domain crates do not pull in reqwest/sqlx/wasmtime/the tokio runtime.
- Node kinds only via the node catalog in `grauwolf-graph` ([ADR-0018](docs/adr/0018-node-catalog.md)).
- TDD: one test case per cycle; commit = test + implementation + docs.
- Before every commit: `cargo fmt --all`, then `cargo xtask check --affected` (clippy `-D warnings` and the fast tests of the changed crates and everything that depends on them, plus the architecture test). Before every push: `cargo xtask check --full` (whole workspace including the slow tests, `#[ignore = "slow: …"]`). Owner's decision 2026-11-12, ADR-0108.
- Language (ADR-0091): the guides in `docs/user-guide/` and `docs/developer-guide/`, `README.md`, ADRs, code comments, the crates' `AGENTS.md`, test data, all messages, the built-in instructions to the model and identifiers are English. Only `docs/handbuch/` and `docs/entwickler/` are German (the state of 2026-11-15, update only when needed). Add new features to `README.md`.
- Sub-agents (owner's decision 2026-11-20): the main session plans, decides and reviews; coding (features, tests, refactoring, fixes) goes to sub-agents with model **Sonnet**. Heavy thinking (architecture, hard debugging) may use Opus, mechanical work (translation, renaming, searching) Haiku. Sub-agents start no further agents and write commit messages without footers.

## Repositories
- The source of truth is the **private** repository github.com/agentsw0rk/grauwolf-studio (everything, including grauwolf studio's code); in a clone of it `origin` points there, and it is never pushed to github.com/agentsw0rk/grauwolf.
- github.com/agentsw0rk/grauwolf is **public**, holds the open source distribution without grauwolf studio's code, and is written only by the export script of the private repository (a snapshot of its HEAD, one commit per export, run by hand by the owner); changes to the public repository are taken over into the private one by hand. What the export leaves out is listed in `.public-exclude`; it also checks that no relative link points there. grauwolf studio is documented publicly but flagged Enterprise; core code and docs must work in both trees (no hard dependency on `studio/`, no Markdown links into the excluded paths – name them in plain text).

## Environment
The development container is ephemeral: the Rust toolchain (`~/.cargo`) and `build-essential` may be missing after a restart. `scripts/dev-container-setup.sh` reinstalls both, idempotently – build-essential and pkg-config with sudo, Rust 1.99 with rustfmt, clippy and the `wasm32-unknown-unknown` target (otherwise the tool build tests fail); it also checks node (22 or newer), prepares the Linux native bindings for the studio's frontend tests in `~/.cache/studio-linux-bindings` (never touching the host's `studio/web/node_modules`) and writes `~/.grauwolf-env` (PATH, the `CARGO_*` settings below, `NAPI_RS_NATIVE_LIBRARY_PATH`; sourced from `~/.zshrc`/`~/.bashrc`); unreachable hosts skip only their step. Options: `--no-apt`, `--no-rust`, `--no-node`, `--check` (report only). Build output outside the mounted `/workspace`: `CARGO_TARGET_DIR=~/grauwolf-target`. The disk is small and shared: build without debug info (`CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`, build directory ~4 instead of ~15 GB) and delete `~/grauwolf-target/generated-equivalence` after the full test run.
