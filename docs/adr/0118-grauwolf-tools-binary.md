# ADR-0118: `grauwolf-tools` – a lean program for building and distributing tools

- **Status:** Accepted (User, 2026-11-13: "is it worth splitting the CLI, e.g. a CLI to build WASM
  tools?" – "measure whether it pays off" – decisions Q1–Q6 after the measurement)
- **Date:** 2026-11-13
- **Related:** ADR-0029/0030 (tool ABI, isolated tool build), ADR-0095 (`tools build`), ADR-0107
  (tool packages: `pack`, `push`, `pull`, `inspect`, `lock`, `keygen`, policy, signatures), ADR-0096
  (images), ADR-0108 (layers, apps order, central configuration), ADR-0115 (agent tree, lockfile).

## Context
`grauwolf` is one CLI for every role: developing and running agents (`spec …`), building and
distributing tools (`tools …`), tokens. Tool authors and CI pipelines that only build, pack and push
WASM tools carry everything the agent runtime needs – model providers, the wasmtime sandbox, MCP,
A2A, sqlx, OpenTelemetry, the HTTP server.

Measured on 2026-11-13 (cold release builds, 4 jobs, Linux arm64 container): the full CLI against a
probe binary that links only what the tool commands need (spec, tool build, packages, OCI, git,
signatures):

| | `grauwolf` | tools only |
|---|---|---|
| cold build | 165 s | 53 s |
| crates compiled | 341 | 181 |
| binary (stripped) | 54 MB (37.7 MB) | 10.9 MB (7.7 MB) |
| build directory | 1.3 GB | 0.4 GB |

## Decision
1. **New program `grauwolf-tools`** with the tool commands of today – `build`, `pack`, `push`,
   `pull`, `inspect`, `lock`, `keygen` – plus `validate` for a tool package folder
   (`grauwolf-tools.json` and `src/*.rs`, not agent specs).
2. **Both stay:** `grauwolf tools …` keeps working with the same behaviour; the command logic lives in
   one library that both programs use (no copy). The big CLI does not shrink; nothing breaks, and the
   builder image keeps one binary for everything.
3. **New app crate `crates/apps/grauwolf-tools-app`** (library + binary `grauwolf-tools`; the package is not called `grauwolf-tools` because the domain crate has that name). In the apps order of ADR-0108 it
   sits **below the CLI and beside the host**: the CLI may depend on it; it depends on neither the
   host, the server nor the CLI.
4. **Lean by rule:** `grauwolf-tools` builds only on domain and adapter crates (spec, toolpkg, oci,
   git, toolbuild, …). Helpers it needs from the host today (`packages::resolver_*`, `load_policy`,
   `lock_path`, the registry/git configuration) move down into the adapters or into the new crate.
   Configuration stays single-sourced: if the `Env` port and the configuration sections it needs live
   in the host, they move into a lower crate shared by both, so `config::VARIABLES` remains the one
   list of variables. `xtask check-arch` gets the rule "grauwolf-tools depends on no app crate", and a
   test keeps the crate count of `grauwolf-tools` well below the CLI's (guarding against an accidental
   host dependency).
5. **Distribution:** a small container image `ghcr.io/agentsw0rk/grauwolf-tools` (published on tags
   like the others) for tool authors' CI; the builder image contains it next to the CLI;
   `scripts/build-macos.sh --tools` builds it on a Mac.

## Consequences
- Tool authors and CI get an ~8 MB program that builds in under a minute.
- One more binary to version alongside the CLI and the builder image (same release, same version).
- The tool commands get a clear interface of their own; the CLI's `tools` subcommand becomes a thin
  delegation.

## Implementation notes (2026-11-13)
- **Crate name.** The app crate is `crates/apps/grauwolf-tools-app` (package `grauwolf-tools-app`,
  library `grauwolf_tools_app`); the binary is `grauwolf-tools`. The package name `grauwolf-tools` is
  taken by the domain crate (tool contract). The architecture rules use `arch::TOOLS_APP`.
- **Configuration** moved into the new adapter crate `grauwolf-config` (no grauwolf dependencies, std
  only): the `Env` port with `ProcessEnv` and `MapEnv`, `Lookup`, `non_empty`/`non_blank`,
  `RegistryConfig` and **the whole table `VARIABLES`** (also the entries of host sections), so the
  list of variables stays one list. `grauwolf-host::config` re-exports it all; the host tests that keep
  `VARIABLES` equal to what `Config` reads and to the CLI reference stay in the host.
- **Helpers from the host** moved into `grauwolf-toolpkg` as module `setup` (resolver constructors,
  `load_policy`, `lock_path`, key helpers, `pack_folder`, `load_packed`, `push_package*`, with their
  tests); `grauwolf-toolpkg` now depends on `grauwolf-toolbuild` and `grauwolf-config`. The host's
  `packages` re-exports `setup`, so every path stays.
- **The agent tree for `tools lock`.** The tree is in the host with its read phase (providers, skill
  directories, companions, signatures) and cannot move below it cheaply. Decision: the *lock phase*
  moved (module `grauwolf_toolpkg::tree_lock`, `lock_tree` over `LockNode { file, relative, spec }`:
  key prefixes, one lockfile, stale entries, `--locked`), and the host's `AgentTree::resolve` calls it.
  `grauwolf-tools lock` reads the tree with its own small reader (`spec_tree`, spec crate only: local
  sub-agents, diamonds once, cycles and sub-specs outside the root's directory are errors like in the
  host, `code_file` stands in as empty code) and calls the same `lock_tree`. `grauwolf tools lock`
  passes the shared command a locker over the host's fully checked tree (`LockSummary` is shared, so
  the output line is identical). Consequence: `grauwolf-tools lock` does not check providers or skill
  directories; `grauwolf spec validate` does.
- **Delegation.** `ToolsCommand` (clap), `Io`, the print macros, `BuildOptions`/`CacheOptions` and all
  command code live in `grauwolf-tools-app`; the CLI re-exports them and keeps only the locker. The
  CLI's existing tests stayed green unchanged. `grauwolf tools validate` exists as a side effect.
- **`validate`** reads the manifest as `pack` does, checks `src/<tool>.rs` of every tool (present,
  UTF-8, forbidden constructs through `grauwolf_toolbuild::guest_crate`) and modules used twice; it
  reports every problem and builds nothing.
- **Guards.** `xtask check-arch`: `grauwolf-tools-app` depends on no other app crate; host and server
  must not depend on it, lower layers never; only the CLI may. `tests/workspace_architecture.rs`:
  the closure of normal dependencies from `cargo metadata --offline --filter-platform <host>` must be
  at most 75% of the CLI's. Measured with that graph (features of the whole workspace): 216 against
  354 crates = 61%; a host dependency gives about 98%. The percentage (not a fixed count) moves with
  the dependencies both programs share.
- **Distribution.** `docker/tools.Dockerfile` (Rust 1.99 slim with the WASM target, entry point
  `grauwolf-tools`), a CI step in the `images` job of `.github/workflows/images.yml` (build, `--version`,
  `validate` of the user guide's package; push of `ghcr.io/agentsw0rk/grauwolf-tools` on tags, amd64
  and arm64), the builder image installs the program next to the CLI and checks its version,
  `scripts/build-macos.sh --tools` writes `dist/grauwolf-tools`. Docker is not available in the
  development container: the Dockerfile and the CI job are **verified by CI only**.
- **Measured** (the real binary, Linux arm64 container, `cargo build --release --locked -j4 -p
  grauwolf-tools-app` in a fresh target directory): **55 s**, 191 crates compiled (workspace crates
  included), binary 13.3 MB with symbols, **9.6 MB stripped** (the probe measured 7.7 MB without
  clap's help text and the commands). Against the full CLI (165 s, 341 crates, 37.7 MB stripped) that
  is a third of the time and a quarter of the size.
- **One tree reader (2026-11-13, supersedes "The agent tree for `tools lock`" above).** The second
  reader (`spec_tree`) is gone. The agent tree moved into the adapter crate `grauwolf-agent-tree`
  (ADR-0115 notes), which the host and `grauwolf-tools-app` both use: `grauwolf-tools lock` reads
  the same tree as `grauwolf spec validate` (real `code_file` / `tool_files` content, local skill
  directories, companions, signatures, warnings, diamonds, cycles, outside paths). The checks that
  need more than the crate knows are options of the read phase (`ReadOptions`): the provider
  check is the only one `grauwolf-tools` skips (it runs no agents and knows no providers; the host
  passes its built-ins), the skill directory check it makes (`grauwolf-builtin-tools` is light).
  `tools lock` still does not build code. Cost: `grauwolf-agent-tree` pulls `grauwolf-generator`
  (`companion_paths`, with `grauwolf-codegen`) and `grauwolf-builtin-tools` into the program.
  Measured with the same graph as above: 225 against 356 crates = 63% (before 61%; limit 75%).
  Test: `grauwolf-cli/tests/lock_equality.rs` – both programs write the same lockfile and output
  for a tree with a diamond, a sub-agent package and a git skill.
