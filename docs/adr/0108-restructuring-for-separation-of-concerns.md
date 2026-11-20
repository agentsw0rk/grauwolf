# ADR-0108: Restructuring for separation of concerns and testability

- **Status:** Accepted (User, 2026-11-11: "think about how I can split the project; the goal is better architecture and testability, and separation of concerns" – "write it as an ADR with a stage plan"; 2026-11-12: continue with the stages). The commit rule (`--affected` per commit) needs the owner's separate approval.
- **Date:** 2026-11-11

## Context
The layers of ADR-0016 (domain, adapters, support, apps) still hold, but several crates have grown
into places where unrelated concerns meet (measured on 2026-11-11, lines of `src`):

| Where | Size | Concerns mixed |
|---|---|---|
| `grauwolf` (facade) | 3,600 lines, 26 modules | public API, agent loader, provider wiring, tool provisioning (built-in, MCP, packages, sub-agents), chat API, observations, service start, environment variables |
| `grauwolf-agents/src/context.rs` | 1,270 lines | history, model and fallbacks, sampling, tool scopes, storage (now behind a mutex), usage, cost, budget, limits, interceptors, moderation, fact extraction, compression, decisions |
| `grauwolf-spec/src/validate.rs` | 2,000 lines | every validation rule in one file |
| `grauwolf-builtin-tools` | 2,400 lines | files, user I/O, git (libgit2 and an own HTTP transport), fetch, key-value, todo – one crate, every dependency for every user |
| `grauwolf-cli/src/main.rs` | 1,760 lines | all commands; nearly all CLI tests start the binary |
| `grauwolf-toolpkg` | 2,900 lines (before stage 3b) | a general OCI distribution client and the tool-package logic |

Further:
- Configuration is read from the environment in many places (`OLLAMA_HOST`, `OLLAMA_BASE_URL`,
  `RKEV_*`, `ATLASSIAN_*`, `GIT_TOKEN`, `GRAUWOLF_*`, `DOCKER_CONFIG`, …).
- Apps depend on apps: the facade on `grauwolf-server`, the server on the interpreter.
- There is one test run for everything. With about 970 tests, compiling generated crates and
  starting processes, every commit check takes 15–20 minutes, independent of the size of the
  change. This makes small, test-first commits expensive.

## Decision
### Target structure
```
core          prompt · llm (ports) · tools (ports) · graph engine
runtime       agent-runtime (a slim context) · agent-features (compression, facts, moderation,
              decision, verification – each a component behind a small interface)
spec          spec-model (types, parsing, JSON schema) · spec-check (rules as modules)
              · strategies (templates shared by interpreter, generator and builder)
engines       interpreter · generator
integrations  providers/* · tools-fs · tools-git · tools-web · atlassian · mcp · a2a
              · oci (general registry client) · toolpkg · stores/* · otel
host          configuration (one place for every variable and file) · loader · provisioning of
              providers, tools, packages and sub-agents (the composition root)
surfaces      grauwolf (public API, re-exports only) · service (HTTP, A2A, MCP, chat API,
              observations) · cli (commands as modules over host)
```
Rules, checked by `cargo xtask check-arch`:
- Dependencies point downwards only.
- Surfaces depend on `host`, the engines and ports – never on another surface.
- Integrations know only ports.
- `core` and `runtime` keep the domain rules of ADR-0016: no I/O, no tokio runtime.

### Testability
- **Test tiers:**
  - *fast*: unit tests, adapter contract tests with recorded responses, in-process integration
    through `host`;
  - *slow*: compiling generated crates, CLI tests that start the binary, image-like tests – in
    their own test targets (`tests/slow_*.rs`) or behind a feature;
  - *live*: as today, opt-in by variables.
- **`cargo xtask check`** with `--affected` (fmt, clippy and fast tests of the changed crates and
  everything that depends on them, from `cargo metadata` and `git diff`) and `--full` (today's run
  including slow tests). The commit rule changes only after the owner agrees; proposal:
  `--affected` before every commit, `--full` before every push and in CI.
- **One test kit** (`grauwolf-test`): fake LLM builder, fake clock, in-memory stores, test registry,
  HTTP fakes. The ad-hoc servers in single test files move there.
- **Ports instead of global state:** environment, processes, file system and network behind small
  interfaces, so tests never mutate the process environment (as the Docker credential code already
  does).
- **CLI logic as a library:** commands are functions over `host` and are tested directly; a few
  end-to-end tests still start the binary.

### Stages
Each stage gets its own ADR (or addendum), keeps behavior unchanged and is guarded by the existing
tests; breaking changes of the public Rust API are collected per stage and announced in its ADR.

| Stage | Content | Benefit | Risk |
|---|---|---|---|
| 1 | Test tiers; `xtask check --affected/--full`; slow tests in their own targets | much shorter commit checks | low |
| 2 | CLI commands as modules with library tests; `validate.rs` as rule modules with per-rule tests | readable, targeted tests; no API break | low |
| 3 | Split `builtin-tools` by dependency (`tools-fs`, `tools-git`, `tools-web`, light tools in core); extract `oci` from `toolpkg` | smaller builds, clear boundaries | low–medium |
| 4 | New crate `host` with central typed configuration (sources: environment, spec, files; one validation; one documented list of variables); facade becomes re-exports; `service` gathers the HTTP side; server no longer depends on the interpreter | API and composition separated, surfaces decoupled | medium (facade API moves) |
| 5 | Decompose `AgentContext` (conversation, model selection, accounting/budget/limits, storage, event bus) and move features to `agent-features` behind interfaces | the largest gain in testability | high (core of the runtime) |
| 6 | `strategies` as its own crate; architecture rules in xtask extended to the new layers; crate `AGENTS.md` files and docs updated | templates in one place, rules enforced | low |

Order and scope may change after each stage; the plan is recorded in the local roadmap.

## Consequences
- Every concern gets a home; tests can target one component without building the whole agent.
- More crates (roughly 8–10 new ones); the workspace stays one repository, versions stay together.
- Stages 4 and 5 change public Rust APIs (import paths, `AgentContext`); specs, the CLI, the HTTP
  routes and generated crates keep working (generated code is regenerated by the generator of the
  same version).
- Stage 1 is the precondition for the rest: it makes the many small, checked commits of the
  following stages affordable.

## Stage 1: implementation notes
Measured on 2026-11-12 (8 cores, warm build, `cargo test -j 4`; every test executable run on its own,
as cargo runs them; times are the run of the executables, without compiling). The 241 test
executables hold 969 tests.

| Run | Time of the test executables | Tests run |
|---|---|---|
| before: `cargo test --workspace` | 290 s with a cold `generated-equivalence` (190 s with a warm one) | 969 |
| after: fast tier (`cargo test --workspace`) | 22 s | 939 |
| after: slow tier only (`-- --ignored`) | the rest, about 270 s cold | 30 |

The 20 slowest tests before (wall time inside a parallel test run, cold):

| s | Test |
|---|---|
| 230 | `grauwolf-generator/equivalence` `generated_crates_behave_like_the_interpreter` |
| 75 | `grauwolf-generator/equivalence` `generated_crates_remember_like_the_interpreter` |
| 74 | `grauwolf-generator/equivalence` `generated_crates_route_by_the_decider_they_are_given` |
| 65 | `grauwolf-generator/equivalence` `generated_services_answer_health_and_run` |
| 51 | `grauwolf-generator/equivalence` `generated_apps_provide_providers_tools_and_memory_themselves` |
| 11 | `grauwolf-cli/tool_packages` `pack_writes_a_deterministic_image_layout` |
| 10 | `grauwolf-toolbuild/build` `builds_tools_concurrently_into_a_fresh_cache` |
| 10 | `grauwolf-toolbuild/build` `builds_offline_only_after_dependencies_were_prepared` |
| 10 | `grauwolf-toolbuild/build` `caches_artifacts_by_content` |
| 10 | `grauwolf-toolbuild/build` `runs_cargo_under_a_configured_isolation_wrapper` |
| 9 | `grauwolf-toolbuild/build` `reports_compile_errors_with_lines_relative_to_the_tool_code` |
| 9 | `grauwolf-cli/cli` `build_does_not_pass_the_callers_environment_to_cargo` |
| 8 | `grauwolf-cli/cli` `build_runs_cargo_under_the_configured_wrapper` |
| 8 | `grauwolf-cli/tool_packages` `push_needs_credentials_and_never_prints_them` |
| 8 | `grauwolf-cli/tool_packages` `push_sends_a_packed_layout_and_pull_gives_the_same_digest` |
| 8 | `grauwolf-cli/tool_packages` `push_packs_a_package_folder_first` |
| 8 | `grauwolf-cli/tool_packages` `push_takes_its_credentials_from_the_docker_configuration_and_a_helper` |
| 8 | `grauwolf-cli/cli` `build_points_compile_errors_to_the_tool_code_in_the_spec` |
| 7 | `grauwolf/pack` `the_same_folder_gives_the_same_digests_even_with_a_fresh_build` |
| 6 | `grauwolf/pack` `the_example_package_folder_of_the_user_guide_packs` |

The tests are not the whole cost of a check: a cold build and clippy of the workspace take longer
than the fast tier, which is why `--affected` (below) also narrows `clippy` and the test build to the
changed packages.

**Slow tier.** 30 tests carry `#[ignore = "slow: <reason>"]`; they still compile and run with
`cargo test --workspace -- --include-ignored` (or `-- --ignored` alone). Marked: the five equivalence
tests (generated crates built with cargo), the six `grauwolf-toolbuild` build tests, five
`grauwolf-cli` tests and seven `tool_packages` tests that start the binary which builds tools or
generated crates, the two package-folder tests and four self-tool/tools-directory/builder tests of
`grauwolf` that build WASM tools, and the `compile_fail` test of the macros. Everything below about
1.5 s stays in the fast tier, including the other CLI tests that start the binary (about 100 tests,
in total about 8 s). Splitting the slow tests into their own `tests/slow_*.rs` targets was not done:
the attribute keeps each test next to its fast neighbors and its helpers.

**`cargo xtask check`.** `--full` runs `cargo fmt --all --check`, clippy over the workspace,
`cargo test --workspace -- --include-ignored` and the layer rules. `--affected [--base <rev>]`
reads the changed files from `git diff --name-only <base>` and `git status --porcelain` (default
base `HEAD`, so staged, unstaged and untracked changes count) and selects:
- the package that contains a changed file, and every package that depends on it, directly or not
  (normal, dev and build dependencies, from `cargo metadata --no-deps`);
- every package if `Cargo.toml`, `Cargo.lock`, `.cargo/`, `xtask/` or `rust-toolchain*` changed;
- `grauwolf` (the facade, which checks the example specs of the guides) for
  `docs/user-guide/examples/**` and `docs/handbuch/beispiele/**`, and the seven packages that read
  the corpus `examples/` (spec, interpreter, generator, builder agent, facade, server, CLI);
- nothing for other documents, which only get the format check.

It then runs fmt (check only), clippy with `-p <selected> --all-targets -D warnings`, the fast tier
of the tests of the selected packages and the layer rules, and prints the selection with the reason
for each package. It works offline and exits non-zero at the first failing step. The selection logic
is pure and covered by unit tests with a fake workspace.

Measured on the same machine with a warm build directory (`CARGO_BUILD_JOBS=4`, a one-line change):

| Change | Selected | `--affected` | `--full` |
|---|---|---|---|
| documents only | none | 1 s | about 7 min (390 s of tests, warm) plus cold compile |
| `grauwolf-cli` | 1 package | 23 s (clippy 4 s, tests 16 s) | |
| `grauwolf-spec` | 9 packages | 72 s | |
| `grauwolf-prompt` (the bottom of the graph) | 28 of 33 packages | 105 s | |

The closure is wide for the lowest crates, so the largest saving there is the slow tier; for a
leaf crate or a document the check takes seconds.

**Not done in stage 1.** The commit rule in `AGENTS.md` is unchanged (`cargo test --workspace`
now runs only the fast tier; use `-- --include-ignored` or `cargo xtask check --full` for the
whole run). A CI job for `--full` does not exist yet. Moving slow tests into `tests/slow_*.rs`
targets or behind a feature is left out (see above).

## Stage 2a: implementation notes
`grauwolf-cli` is now a library (`grauwolf_cli`) plus a thin binary: `src/main.rs` only parses
(`Cli::parse`) and calls `dispatch(cli, &Io::stdio())`; `run(args, &io)` is the in-process entry,
which also routes usage errors, `--help` and `--version` to the `Io`. `src/main.rs` of 1,760 lines
became one module per command group: `commands/spec/{validate,graph,schema,build,create,generate,
run,resume,chat,serve,mcp}.rs` with the `SpecCommand` enum and its dispatch in `commands/spec/mod.rs`,
the loader and handle helpers shared by run/chat/serve/mcp in `commands/spec/{loader,handles}.rs`,
`commands/tools.rs`, `commands/token.rs`, and the shared parts `options.rs` (build, cache and run
options, `Usage`), `output.rs` and `errors.rs`. Behavior is unchanged: the `--help` output of every
command is byte-identical to before (compared before and after), and the process tests in
`tests/cli.rs` and `tests/tool_packages.rs` were not touched.

Commands write only through an `Io` handle (two shared writers; `Io::captured()` for tests; macros
`out!`, `outln!`, `err!`, `errln!`), never through `println!`, so `tests/commands.rs` runs them
in-process and checks stdout and stderr: validate (warnings, unknown providers, missing file), graph,
schema, token, `tools lock`/`pull` errors, `build`, `create` and `generate` including argument
errors, and the option checks of run, resume, chat, serve and mcp. These tests avoid the network
and the model; commands that need a provider stay covered by the process tests. Not behind a port
yet, and left to stage 4: standard input (`spec chat`, the console approvers, `--confirm-tools`),
the process environment (credentials, `GRAUWOLF_SERVER_TOKEN`) and the file system. No logic moved
to other crates; `xtask/src/affected.rs` needed no change.

## Stage 2b: implementation notes
`grauwolf-spec/src/validate.rs` (1,994 lines) became the directory `src/validate/` with `mod.rs`
(302 lines: public entry points, error and warning types, `GraphScope`, identifier helpers, the
fixed call order of the rules in `validate_with`) and one module per concern: `schema` (176),
`tools` (70), `tool_packages` (88), `models` (240), `builtin_tools` (187), `sections` (171: MCP,
agent tools, knowledge, long-term memory, self tools, limits), `subgraphs` (237), `goap` (193),
`graph` (227), `edges` (109), `reachability` (67), `decisions` (104) and `warnings` (40). The
rules moved verbatim: same errors in the same order with the same pointers and messages, and the
same warnings; the existing spec, CLI and example tests were not touched. Rules stay `pub(super)`
methods of `AgentSpec` or functions, visible only inside `validate`. A `Rule` trait was not
added: `validate_with` is a plain list of calls in a fixed order, which tests depend on. New unit
tests next to the rules cover the pure helpers (`url_problem`, field, type and identifier names,
pointer escaping); the rules themselves stay covered by the integration tests in `tests/`.

## Stage 3a: implementation notes
`grauwolf-builtin-tools` was split by dependency, without a behavior change for specs, the CLI or
generated crates:

| Crate | Content | Heavy dependencies |
|---|---|---|
| `grauwolf-builtin-tools` | files (with `search_files`), key-value, to-do, think, user I/O | `regex` only |
| `grauwolf-tools-git` | `GitTools`, `GitConfig`, the own smart-HTTP transport and their tests (moved with `git mv`, history follows) | `git2` (vendored libgit2, needs a C compiler), blocking `reqwest` |
| `grauwolf-tools-web` | the `fetch` tool (address checks, redirects, HTML to text) and its tests | `reqwest`, `regex` |

All three are adapters (ADR-0016); the architecture rules and the affected selection of `xtask`
needed no change. The public names moved with their code (`grauwolf_tools_git::{GitTools,
GitConfig, PushPolicy, GIT_TOOLS}`, `grauwolf_tools_web::{fetch_tool, FetchConfig, Resolver, ...}`);
`grauwolf::builtin` still re-exports `GitConfig` and `GitTools`.

**Facade features.** `builtin` is split into `builtin` (light tools), `git` (`grauwolf-tools-git`),
`fetch` (`grauwolf-tools-web`) and `atlassian` (`grauwolf-atlassian`); all four are in `default`.
Without a feature, `builtin::tools_for` refuses the tools of that group before the run:
`this build has no git tools (feature `git`)` (likewise `fetch tool`, `Jira tools`, `Confluence
tools`; the check comes before the check of the `git`/`fetch` section and of credentials). These
errors are tested by unit tests that only exist in builds without the feature (for instance
`cargo test -p grauwolf-host --no-default-features --features builtin --lib builtin`); the default
workspace build does not compile them, the feature matrix of `cargo xtask check --full` does
(see "Feature matrix" below). The generator adds `git`, `fetch` and `atlassian` to the facade
features of a generated crate when its spec uses tools of these groups (tested in
`tests/generate.rs`); `--full-facade` still depends on the facade with all defaults.

**Who pulls what.** `cargo tree -i git2` shows only `grauwolf-tools-git`, `grauwolf` and
`grauwolf-cli`; `grauwolf-builtin-tools` no longer depends on `git2` or `reqwest`
(`grauwolf-generator` has it as a dev dependency for `FileTools` only). `cargo tree -p
grauwolf-builtin-tools` lists 151 packages before and 43 after.

**Build time.** A cold `cargo build -p grauwolf-builtin-tools` (dev profile, no debug info, `-j 4`,
empty target directory) took 24.9 s before and 6.7 s after. Builds of the full facade are
unchanged, since the defaults include every feature.

## Stage 3b: implementation notes
The general OCI distribution client was extracted from `grauwolf-toolpkg` into the new adapter
`grauwolf-oci`, without a behavior change (moved with `git mv`, history follows).

| Crate | Content | Size (src) |
|---|---|---|
| `grauwolf-oci` | `RegistryClient` (pull, push, scopes, bearer flow, digest checks), `RegistryAuth` and the Docker configuration, `OciImage` with the image layout reader/writer, `OciError`, the in-memory test registry (feature `test-registry`), the example `serve_empty_registry`, their tests | 1,830 lines (2,160 with tests and examples) |
| `grauwolf-toolpkg` | package format and media types, `Package`, `PackedPackage`, `source_archive`, cache, lockfile, resolution, `TestPackage`/`PONG_MODULE` | 1,240 lines (1,800 with tests and examples) |

Before the split `grauwolf-toolpkg` had 2,860 lines of source (3,670 with tests and examples).

- **Dependencies.** `grauwolf-oci` depends on `grauwolf-spec` only (for `OciReference`); it has no
  tool-specific code: media types and manifest contents are the callers' business.
  `grauwolf-toolpkg` depends on `grauwolf-oci`, `grauwolf-spec` and `grauwolf-tools` and no longer
  on `reqwest`, `sha2`, `base64` or `axum`. The architecture rules needed no change.
- **Public paths.** `grauwolf-toolpkg` re-exports `RegistryClient`, `RegistryAuth`,
  `REGISTRY_AUTH_ENV`, `DockerConfig`, `default_config_path` and `sha256_digest`, so the facade
  (`grauwolf::toolpkg`) and the CLI did not change. `RegistryClient` methods return `OciError`;
  `PackageFetchError` has `From<OciError>` with the same variants and messages.
- **Image.** `PackedPackage` wraps `grauwolf_oci::OciImage` (digest, manifest, blobs) and derefs
  to it, which is what `RegistryClient::push` takes. `write_layout` passes the artifact type;
  `read_layout` reads the blobs the manifest names and then checks the package media types.
- **Test registry.** `grauwolf_oci::testing::TestRegistry` stores blobs and manifests
  (`add_blob`, `push_raw_manifest`); `grauwolf_toolpkg::testing::TestRegistry` wraps it, derefs to
  it and adds `push(repository, tag, &TestPackage)`. `test-registry` of toolpkg forwards to oci.
- **Tests.** Upload, token flow, refusals, secrets and Docker credentials moved to
  `grauwolf-oci/tests/push.rs` (with generic images); new `tests/pull.rs` there. Toolpkg keeps the
  round trips push then pull, pull, resolve and cache tests.
- **Open for stage 4.** The environment variables (`GRAUWOLF_REGISTRY_AUTH`, `DOCKER_CONFIG`) are
  still read inside `RegistryAuth::from_env`; central configuration should hand a ready
  `RegistryAuth` to the host.

## Stage 4: design (2026-11-12; done with 4d)
The facade `grauwolf` (3,700 lines in 26 modules) holds two different things: the public API
(re-exports of the crates) and the composition root that assembles a runnable agent from a spec
(loader, providers, tool provisioning, memory, knowledge, packages, MCP, service start, chat API,
observations, environment variables). Stage 4 separates them in four steps; specs, the CLI, the HTTP
routes and generated crates keep working, and **the facade keeps its module paths** (`grauwolf::
loader`, `grauwolf::providers`, `grauwolf::service`, `grauwolf::app`, …) as re-exports, so the
generator templates and users of the Rust API need no change in this stage.

- **4a – `grauwolf-host` (new crate, layer `apps`, below the surfaces):** moves the composition root
  out of the facade unchanged: `app`, `attachments`, `builder`, `builtin`, `cache`, `checkpoints`,
  `env`, `external`, `features`, `knowledge`, `loader`, `long_term_memory`, `mcp`, `memory`,
  `otel`, `packages`, `providers`, `runtime`, `self_tools`, `systemone`, `tool_files`, `tools_dir`
  with their tests. The facade re-exports them under the same paths and keeps only `lib.rs`
  (re-exports, macros, features).
- **4b – central configuration in `grauwolf-host::config`:** one typed `Config` with sections
  (providers, registry, Atlassian, git, System One, service, chat API, telemetry, data and cache
  directories) built from an `Env` port (`trait Env { fn var(&self, name: &str) -> Option<String> }`,
  `ProcessEnv`, `MapEnv` for tests) plus explicit overrides (CLI options); one table of every
  variable with its meaning (the source of reference-cli's list); the modules of 4a take their
  settings from `Config` instead of reading the process environment themselves. Tests stop mutating
  the process environment.
- **4c – the HTTP side in one place:** `service`, `chat_api` and `observations` move from the facade
  into `grauwolf-server`, which then depends on `grauwolf-host` (the server is a surface; surfaces
  may depend on host and engines). The facade re-exports them as before (`grauwolf::service`,
  `grauwolf::chat_api`, `grauwolf::observations`). No surface depends on another surface: the facade
  is the public API, not a surface; the CLI depends on host and server.
- **4d – CLI and rules:** the CLI uses `grauwolf-host` (and the server) directly instead of the
  facade where that removes indirection; `cargo xtask check-arch` gets rules for the new layering
  (`grauwolf-host` may not depend on `grauwolf-server`, `grauwolf-cli` or the facade; the server not
  on the CLI or the facade; adapters and domain crates never on host); docs and the remaining stage
  notes.

Public Rust API: paths stay; new direct paths (`grauwolf_host::…`) are additional. A later cleanup
may drop facade re-exports that nobody needs; that would be announced separately.

## Stage 4a: implementation notes
The composition root moved out of the facade into the new crate `grauwolf-host` (layer `apps`)
without a behavior change (`git mv`, history follows). The commit is one big move because the
facade and the host cannot compile separately halfway.

| Crate | Source (lines) | Tests (lines) |
|---|---|---|
| `grauwolf` before | 3,737 in 26 modules | 2,997 in 25 files |
| `grauwolf` after | 965 in 4 files (`lib.rs` with the re-exports, `chat_api`, `observations`, `service`) | 859 (`chat_api`, `chat_api_global`, `macros`, `observations`, `service`) |
| `grauwolf-host` | 2,824 in 24 files (22 moved modules, `lib.rs`, new `chat_mode`) | 2,138 in 20 files |

- **Moved:** `app`, `attachments`, `builder`, `builtin`, `cache`, `checkpoints`, `env`, `external`,
  `features`, `knowledge`, `loader`, `long_term_memory`, `mcp`, `memory`, `otel`, `packages`,
  `providers`, `runtime`, `self_tools`, `systemone`, `tool_files` (private; `spec_schema` is
  re-exported), `tools_dir`, with their unit tests and the integration tests that exercise them.
  They stay in the facade until 4c: `chat_api`, `observations`, `service` and their tests,
  `macros.rs`.
- **Paths.** `lib.rs` of the facade re-exports each module with `pub use grauwolf_host::<module>;`
  under the same feature conditions as before, so `grauwolf::loader`, the generator templates, the
  CLI and the server did not change. The Cargo features of the facade forward to features of the
  same name of the host (`ollama = ["dep:grauwolf-ollama", "grauwolf-host/ollama"]`, …); the
  adapters stay optional dependencies of the facade only where it re-exports them (providers,
  `sandbox`, `toolpkg`). `default` is identical in effect. The workspace entry
  `grauwolf-host` has `default-features = false`, so the facade selects the features itself; the
  host's own `default` lists every feature (for `cargo test -p grauwolf-host`).
- **Seam to the facade (`grauwolf_host::chat_mode`).** The host modules `mcp` and `providers`
  needed the chat API (`observe_mcp_tools` and `enabled`). Instead of moving `chat_api` and
  `observations` (4c), the process-wide state moved to a tiny host module:
  `enabled()`/`set_enabled`, `set_mcp_tools_observer(Arc<dyn Fn(&str, &[ToolDescriptor])>)` and the
  crate-internal `observe_mcp_tools`. `ChatApi::from_dir` sets both; the host knows neither chat
  API nor observations, so the direction stays host ← facade. `providers::ollama_host` became
  `pub` (the facade's `chat_api` uses it). 4c can move `chat_api` next to the server and keep
  `chat_mode` as it is, or replace it by the central configuration (4b did: see below).
- **Tests.** The moved integration tests use the domain crates directly (`grauwolf_spec`, …)
  instead of the facade re-exports and `grauwolf_host::<module>` for the moved modules.
  `docs_examples` and `pack` (which read `docs/`) and the example-spec readers moved with them, so
  `xtask`'s `readers_of` now selects `grauwolf-host` for the guide examples and adds it to the
  readers of `examples/`; the facade stays a reader of `examples/` (its service test).
  The test for features that are off runs in the host:
  `cargo test -p grauwolf-host --no-default-features --features builtin --lib builtin`.
- **Architecture rules.** `xtask check-arch` needed no change (`crates/apps/*` is the apps layer,
  which may use everything). The rules for the new layering come in 4d.
- **Open for 4b (reads of the process environment in the host)**, all in `crates/apps/grauwolf-host/src/`:
  `builtin.rs:43` (`tools_for`: Atlassian and git variables via `tools_with_env_for`),
  `external.rs:38`, `long_term_memory.rs:66` (embedder), `loader.rs:159` (executor) and
  `loader.rs:191` (System One decider), `providers.rs:129` and `:167` (`executor_from_env`,
  `client_from_env`; the provider variables are read in `client_from` through the `env` argument),
  `env.rs:12` (`expand`: `${VAR}` in MCP and A2A headers, used at `mcp.rs:54` and `loader.rs:285`),
  `otel.rs:36` (`OTEL_SERVICE_NAME`; the endpoint variables are read inside `grauwolf-otel`),
  `packages.rs:22` and `:58` (`RegistryAuth::from_env`, see stage 3b).

## Stage 4b: implementation notes
The environment is now read through one typed configuration, `grauwolf_host::config` (also
`grauwolf::config`), without changing a variable name, a default or an error message.

- **`Env` port.** `trait Env: Send + Sync { fn var(&self, name) -> Option<String> }`, `ProcessEnv`
  (`std::env::var(..).ok()`), `MapEnv::new(pairs)` for tests, and a blanket impl for closures
  `Fn(&str) -> Option<String> + Send + Sync`, so the closure-taking functions of 4a stay usable.
- **`Config::from_env(&dyn Env)`** with the sections `providers` (`ProvidersConfig`; the
  OpenAI-compatible variables are read by name from the fixed list `COMPATIBLE_VARIABLES`),
  `registry` (`GRAUWOLF_REGISTRY_AUTH`, `DOCKER_CONFIG`, `HOME`, `PATH` instead of
  `RegistryAuth::from_env`, which read them itself), `atlassian`, `git` (`GitEnvConfig`),
  `system_one`, `service` (ports parsed; the only fallible section), `chat_api` and `telemetry`.
  Each section has `from_env`/`from_lookup`; a module takes the one section it needs, so a bad
  `SERVER_PORT` never fails `spec run`. Empty/blank values are normalized exactly where the old
  code did (`non_empty`/`non_blank` in `config.rs`, documented per field).
- **`config::VARIABLES`**: one table (name, section, meaning). Tests: `Config` reads only names in
  the table (a recording `Env`), no duplicates, and every name appears in backticks in
  `docs/user-guide/reference-cli.md`, whose environment table gained the missing `<NAME>_BASE_URL`
  names; a provider test compares `OPENAI_COMPATIBLE` with `COMPATIBLE_VARIABLES`.
- **Host functions.** New core functions take sections or an environment (`client_with`,
  `executor_with`, `executor_for_config`, `executor_builder_with`, `embedder_with`,
  `decider_with`, `builtin::tools_with_config`, `packages::{registry_auth, resolver_with,
  resolve_tools_with, push_package_with}`, `OtelTracing::init_with`,
  `external::tools_with_env_for`, `mcp::start_servers_with`, `LongTermMemory::for_spec_with`,
  `env::expand_with`); the 4a functions remain as thin wrappers (closure or `ProcessEnv`), so the
  facade paths and the generated crates compile unchanged. `ollama_host` now takes
  `&ProvidersConfig`. `AgentLoader` gained `with_env(Arc<dyn Env>)` (default `ProcessEnv`) and
  `with_chat_api_mode(bool)`.
- **Entry points** read the process: `spec serve`/`run`/`mcp` (token, telemetry, registry, chat
  API), `tools push|lock|pull`, `service::main` (`ServiceConfig::from_env`, now built on the
  host's `ServiceConfig` and `ChatApiConfig`), the generated crates (their own `std::env::var`
  closures) and the wrappers above.
- **Stays outside the configuration:** `OTEL_EXPORTER_OTLP_*` (read by the OpenTelemetry SDK; listed
  in `VARIABLES`), `CARGO`/`RUSTC` (tool build and `generate --check` in the CLI; listed too).
- **`chat_mode` is gone**, split in two. The flag is `ProvidersConfig::chat_api_mode`
  (`GRAUWOLF_CHAT_API=1|true` or `with_chat_api_mode(true)`; the CLI's `serve --chat-api` sets it on
  the loader). One small deviation: because the flag derives from the variable, any `client_from`
  or loader call that sees `GRAUWOLF_CHAT_API=1` now also honors `OLLAMA_BASE_URL`, where 4a only
  did so after a `ChatApi` existed. The MCP tool observer stays a process-wide seam, renamed
  `mcp_observer::set_mcp_tools_observer`: passing it explicitly would run through `external`, the
  loader, `AppOptions` and the generated crates' `run_app_with`, which 4c can do when `chat_api`
  moves next to the server.
- **Tests.** No host test changed the process environment; the one that read it
  (`gives_the_decider_to_specs_with_system_one_decisions`) now loads with an empty `MapEnv`, and the
  chat API test of the `OLLAMA_BASE_URL` fallback uses a `ProvidersConfig` instead of the global
  switch.

## Stage 4c: implementation notes
The HTTP side is in one place: `service`, `chat_api` and `observations` moved from the facade into
`grauwolf-server` (`git mv`, history follows) with their tests, and the process-wide MCP observer
is gone. No route, variable, message or generated service changed behavior. The move is one commit
because the facade and the server cannot compile separately halfway; the observer change came
first as its own commit.

| Crate | Source (lines) | Tests (lines) |
|---|---|---|
| `grauwolf` before | 965 in 4 files | 859 in 5 files |
| `grauwolf` after | 62 (`lib.rs`: re-exports only) | 48 (`macros.rs`) |
| `grauwolf-server` before | 1,313 in 4 files | 1,961 in 10 files |
| `grauwolf-server` after | 2,222 in 7 files | 2,776 in 14 files |

The four moved test files are `chat_api_signature`, `chat_api_models`, `observations` and
`service`.

- **Dependencies.** The server depends on `grauwolf-host` (workspace entry without default
  features) and never on the facade. Its Cargo features: `service` (the `service` module; implies
  host `mcp`, tokio `net`, `signal`, `rt-multi-thread`) and `ollama` (model digests; forwards to
  the host), both in `default`. The facade depends on the server always (`chat_api` and
  `observations` were never feature-gated) and forwards `service` and `ollama`; it lost its own
  `axum`, `sha2`, `futures-util` and most tokio features. `grauwolf::chat_api`,
  `grauwolf::observations` and (feature `service`) `grauwolf::service` are `pub use` re-exports, so
  the CLI and generated crates compile unchanged. `ChatApi::options` is no longer gated by
  `service` (the types are in the same crate).
- **Explicit observer.** `grauwolf_host::mcp_observer` keeps only the type
  `McpToolsObserver = Arc<dyn Fn(&str, &[ToolDescriptor]) + Send + Sync>`; the `Mutex` static,
  `set_mcp_tools_observer` and the one-at-a-time test mutex are gone. `Observations::
  mcp_tools_observer()` builds the observer; it travels as `Option<McpToolsObserver>` through
  `mcp::start_servers_with(spec, env, observer)`, `external::tools_with_env_for(.., observer)`
  (new `external::tools_for_observed` for the process environment), `AgentLoader::
  with_mcp_tools_observer` (applies to sub-agents too) and `AppOptions.mcp_tools_observer`. The CLI's
  `spec serve --chat-api` sets it on the loader; `GeneratedAgent::observed_by` puts it into the
  `AppOptions` of every run. The generated `run_app_with` calls `external::tools_for_observed(..,
  options.mcp_tools_observer.as_ref())` and `AgentLoader::with_mcp_tools_observer` for its
  sub-agents (snapshot `generate__external_tools_lib.snap` changed accordingly). No global state
  is left in the host or the server for this.
- **Tests.** The facade tests moved with their code and use `grauwolf_server`, `grauwolf_host` and
  the domain crates instead of the facade paths; `chat_api_global.rs` became `chat_api_models.rs`
  (no shared state any more, no mutex), the others keep their names (`chat_api.rs` of the facade is
  `chat_api_signature.rs` here, because the server already had `chat_api.rs` for the routes). The
  two Ollama tests are gated by the `ollama` feature. `xtask` no longer lists the facade as a
  reader of `examples/` (its service test is in the server now).
- **Not done.** The generator entry points still use their own `std::env::var` closures instead
  of `ProcessEnv` (optional point; changes the generated code and its snapshots, so it is left for
  a stage with its own generator change).

## Stage 4d: implementation notes
Stage 4 is done. The CLI no longer depends on the facade, and the layering of the apps is checked by
`cargo xtask check-arch` (and so by `cargo test --workspace`). No behavior changed: the `--help`
output of the root command and of every subcommand (20 commands, plus `--version`) is byte-identical
before and after, and the CLI tests pass unchanged.

- **CLI paths.** Every `grauwolf::…` path in `grauwolf-cli` (80 in 15 files, tests included) became
  `grauwolf_host::…` (loader, config, packages, memory, providers, otel, mcp, attachments,
  checkpoints, cache, features, builder, long_term_memory, tools_dir, `spec_schema`),
  `grauwolf_server::…` (`chat_api`, `observations`) or the domain crate (`grauwolf_agents`,
  `grauwolf_interpreter`, `grauwolf_prompt`, `grauwolf_llm`, `grauwolf_tools`, `grauwolf_spec`,
  `grauwolf_builder_agent`, `grauwolf_toolpkg`). New direct dependencies of the CLI:
  `grauwolf-host`, `grauwolf-agents`, `grauwolf-builder-agent`, `grauwolf-interpreter`,
  `grauwolf-llm`, `grauwolf-prompt`, `grauwolf-toolpkg`.
- **Features.** The workspace entry of the host has no default features, so the CLI lists them:
  `ollama`, `openai`, `anthropic`, `gemini`, `sandbox`, `mcp`, `builtin`, `git`, `fetch`,
  `atlassian`, `sql`, `otel`, `a2a`, `systemone`, `packages` on the host and `ollama`, `service` on
  the server. That is everything the facade's default features switched on for the CLI (`macros`
  only concerns the facade itself). The CLI has no features of its own.
- **App order.** `grauwolf-host` → `grauwolf-server` → `grauwolf-cli`; the facade `grauwolf` is the
  public API on top (re-exports, the macros, Cargo features) and nothing in the workspace depends on
  it. Rules in `xtask/src/arch.rs`, for every kind of dependency (dev and build included):
  host must not depend on server, CLI or the facade; server not on the CLI or the facade; CLI not
  on the facade; domain, adapters and support crates never on host, server, CLI or the facade.
  There are no exceptions: no lower crate has a dev-dependency on an app crate today, and
  `APP_DEV_DEPENDENCY_EXCEPTIONS` is empty (a new entry needs a reason next to it). The violation
  messages name the edge and the rule, e.g. `grauwolf-cli (Apps) must not depend on grauwolf
  (dependency): rule app order host -> server -> cli …`. The older message of the layer rules for
  these targets changed with it (`… must not depend on grauwolf (dependency): rule domain, adapters
  and support crates never depend on the app crates …`).
- **Facade without logic.** Checked, not only a convention: `arch::facade_logic_violations` scans
  the files in `crates/apps/grauwolf/src` (comments and string literals ignored) and rejects the
  item keywords `fn`, `struct`, `enum`, `impl`, `trait`, `static`, `const`, `type` and
  `macro_rules`. The check is crude on purpose; a re-export of a macro from `grauwolf-macros` is a
  `pub use`, so it passes. `xtask/tests/workspace_architecture.rs` runs it on the real source.
- **Not done.** The facade re-exports stay as they are (a cleanup of unused ones would be announced
  separately, see the design). Generated crates still depend on the facade, which is its purpose.


## Stage 6: design (2026-11-12)
`strategies.rs` (354 lines) builds graphs from the spec's types (`GraphSpec`, `NodeSpec`, `EdgeSpec`),
and `AgentSpec::from_json` calls it – a separate crate is only possible together with the split of the
target structure (spec-model · strategies · spec-check):
- **`grauwolf-spec-model`** (new, domain): the spec types, parsing into them without expanding the
  strategy, the JSON schema, `OciReference` and the other plain types; no validation.
- **`grauwolf-strategies`** (new, domain): the templates (`single_run`, `react`, `verified`, `chat`,
  `moderate` …) and `expand(&StrategySpec) -> Expansion`, depending only on the spec model and the
  node catalog (`grauwolf-graph`); tests per template.
- **`grauwolf-spec`** (kept): re-exports the model, expands the strategy in `from_json` and validates
  – its public API stays unchanged, so interpreter, generator, CLI, builder and generated crates do
  not notice the split.
- **Rules** (`check-arch`): `grauwolf-spec-model` depends on no other spec crate; `grauwolf-strategies`
  only on the model (and graph); nothing below `grauwolf-spec` depends on it.


## Stage 6: implementation notes (2026-11-12)
Done in four commits: the model split (one commit, because moving the types, the new `from_json`
path and the imports in the consumers only compile together), the strategies, the rule, the docs.
- **`grauwolf-spec-model`** (948 lines `lib.rs`, 90 `mermaid.rs`, 115 `oci.rs`; the last two moved with
  `git mv`): all spec types and constants, `AgentSpec::parse` (version check, `serde_path_to_error`
  format errors, **no** strategy expansion), `json_schema`, `external_tools`, `provider_uses`,
  `providers`, `uses_system_one`, `to_json`, `to_mermaid`, `SpecError`, `escape_pointer_token`.
  Depends on `grauwolf-graph` for `START`/`FINISH` only.
- **`grauwolf-strategies`** (356 lines `lib.rs`, moved with `git mv`; `tests/expand.rs` 190 lines):
  `expand`, `Expansion`, the templates. Depends on the model and `grauwolf-graph` (`check_params`).
  The expansion tests that need `from_json` and `validate` stay in `grauwolf-spec/tests/strategies.rs`.
- **`grauwolf-spec`** (`lib.rs` 75 lines, `validate/` 2229 lines unchanged in behavior): `pub use
  grauwolf_spec_model::*`, so all `grauwolf_spec::…` paths resolve.
- **Call syntax – deviation from the design.** An inherent method cannot be defined for a type of
  another crate, so `from_json`, `validate`, `validate_with`, `warnings` and `warnings_with` are the
  extension trait `grauwolf_spec::AgentSpecExt`. `AgentSpec::from_json(..)` and `spec.validate()`
  keep their syntax, but the trait must be in scope: one `use grauwolf_spec::AgentSpecExt;` was
  added to the ~85 files that call them (tests included; generated code gets
  `use grauwolf::spec::AgentSpecExt as _;` in `run_app_with`, which adds one line to the generator
  snapshots; README and handbook examples updated). The methods that only read the model stay
  inherent, so `spec.providers()`, `to_mermaid()` etc. need no import. The alternative – keeping
  `AgentSpec` in `grauwolf-spec` – would have left nothing to split off.
- **Validation rules.** They were `impl AgentSpec` blocks; now they are methods of the private
  wrapper `Rules<'_>` (dereferences to `AgentSpec`), the rule order is unchanged.
- **Rule** (`check-arch`, `arch::SPEC_ORDER`): graph -> spec-model -> strategies -> spec; a crate
  must not depend on one above it, dev- and build-dependencies included; model and strategies
  therefore depend on no `grauwolf-spec`. Tests in `xtask/tests/arch_rules.rs`.
- `affected::readers_of` unchanged: the new crates' tests read no files outside their directory.

**Addendum (2026-11-12): `grauwolf::prelude`.** Because `from_json`, `validate` and `warnings` are now
methods of the extension trait `AgentSpecExt`, users of the Rust API need it in scope. The facade
offers `grauwolf::prelude` with the trait and the everyday types (spec, interpreter, loader, providers,
configuration, prompts, tools, macros), so `use grauwolf::prelude::*;` is one line for all of them
(User: "a grauwolf::prelude that contains AgentSpecExt and the usual types").


## Stage 5: design (2026-11-12)
`AgentContext` (`grauwolf-agents/src/context.rs`, 1,270 lines, 22 fields, ~70 methods) mixes history,
model selection, tool access, accounting, events, storage, LLM requests and several features. Other
crates use it through a stable set of public methods (`emit`, `usage`, `cost`, `replace_prompt`,
`install`, `intercept`, `with_*`, `store`/`load`, `request_llm*`, `execute_tool`, `snapshot`, …).

Decision: **`AgentContext` stays the public type with its public methods unchanged** and becomes a thin
coordinator over components inside `grauwolf-agents`, each a module with its own unit tests that need
no executor. **No new crate:** stage 6 showed that moving methods of a type into another crate turns
them into extension traits and breaks callers; the components stay in the crate, the features become
modules with free functions over the components, and `impl AgentContext` delegates.

| Component | Responsibility (fields today) |
|---|---|
| `conversation` | history and pending attachments (`prompt`, `pending_attachments`): append, replace, attach to input |
| `model_selection` | active model and the subgraph stack (`model`, `model_scopes`): set, enter, leave |
| `tool_access` | registry, tool scopes, self-made tools (`tools`, `tool_scopes`, `self_tools`): allowed names, descriptors, build permission |
| `accounting` | shared counters and limits of a run incl. forks (`usage`, `cost`, `tool_calls`, `budget`, `prices`, `tokenizer`): add usage and cost, check budget, claim a tool call |
| `event_bus` | features and interceptors (`features`, `interceptors`): emit, prepare a request, finish an answer, tool decisions |
| `storage` | agent storage and progress (`storage`, `progress`): store, load, remove, save progress |
| `llm_gateway` | executor, fallbacks, streaming sink (`executor`, `frame_sink`): requests (plain, streaming, structured, JSON, choices, forced tool choice) using the components above |
| features | `facts` (extract facts, fact compression), `compression` (summarize, keep last), `moderation`, `decision` (`decider`, `decide_with_model`), `fact_memory` – modules with free functions over gateway and conversation |

Steps (each behavior-preserving, guarded by the existing ~1,000 tests, the slow equivalence tests and
new unit tests per component):
- **5a** `accounting` and `storage` (pure state, shared counters; fork semantics kept).
- **5b** `conversation`, `model_selection`, `tool_access`.
- **5c** `event_bus` and `llm_gateway` (requests, fallbacks, streaming, structured, choices).
- **5d** feature modules (facts, compression, moderation, decision); `context.rs` shrinks to the
  coordinator (target: under 400 lines); docs and the notes of stage 5. **Done** (5a–5d).

## Stage 5a: implementation notes (2026-11-12)
- `grauwolf-agents/src/accounting.rs` (`pub(crate) struct Accounting`; `Budget` moved here and is
  still exported from the crate root): usage, cost and tool calls as `Arc<Mutex<…>>` that
  `fork()` shares, plus budget, prices and tokenizer that `fork()` copies. `check_budget(&Prompt)`
  takes the history for `max_prompt_tokens`; `resumed(usage, cost, tool_calls)` is what `restore`
  uses. `tool_limit_message` moved along. Unit tests need no executor (sums, price lookup, limits
  reached exactly at the limit, tokenizer, claiming tool calls, shared counters in forks).
- `grauwolf-agents/src/storage.rs` (`pub(crate) struct Storage`): the value map behind its mutex
  (`call_tool` still works under `lock()`) and the progress hook; `fork()` copies the values and
  drops the hook. `save_progress` takes a closure that makes the snapshot, so nothing is built
  without a hook.
- `AgentContext` keeps its public methods; `add_usage_for`, `check_budget` and the like delegate.
  `context.rs`: 1,269 → 1,119 lines.

## Stage 5b: implementation notes (2026-11-12)
- `grauwolf-agents/src/conversation.rs` (`pub(crate) struct Conversation`): the history and the
  pending attachments; `append` hands the pending attachments to the next user message without
  attachments of its own, `attach_to_input`, `replace_prompt`, `prompt()`/`prompt_mut()` (for the
  compression and the one-request tool choice, which change the history in place), `fork()` copies both.
- `grauwolf-agents/src/model_selection.rs` (`ModelSelection`, `Clone`): the active model and the
  stack of outer models; `outermost()` is what `snapshot()` stores, `set_model` returns the previous one.
  `apply`/`absorb` set only the active model, as before (the stack is not taken over).
- `grauwolf-agents/src/tool_access.rs` (`ToolAccess`, `Clone`): registry, scope stack
  (`ToolScope` moved here) and self-tool settings; `is_allowed` holds the permission logic
  unchanged, `descriptors()` filters by it. `execute_tool` and `call_tool` stay in `context.rs`
  (they need events, interceptors, accounting and storage) and use `is_allowed`/`registry()`.
- Unit tests need no executor (tool tests use a small `DynTool` and a `ToolShelf`).
  `context.rs`: 1,119 → 1,076 lines (the components are 157, 119 and 277 lines with their tests).

## Stage 5c: implementation notes (2026-11-12)
- `grauwolf-agents/src/event_bus.rs` (`EventBus`, `Clone`): features and interceptors. Besides
  `install`/`intercept`/`emit` it holds `prepare` (takes the history as a parameter, no copy without
  interceptors), `finish`, `before_llm_call`, `intercepted_moderation_text`, `judge_tool_call` (the
  before and approval hooks of a tool call; an already set refusal, e.g. the tool limit, is passed in
  and asks nobody) and `after_tool_call`. `MODERATION_PROMPT_ID` moved here and is still exported from
  the crate root. Unit tests need no executor (order, copy-on-prepare, refusals, approvals).
- `grauwolf-agents/src/llm_gateway.rs` (`LlmGateway<'a>`): a borrowed view on executor, model selection,
  bus, accounting and frame sink, built by `AgentContext::gateway()`; the history is not part of it.
  Each request takes the `prompt` to send; the caller appends the answer (`request_llm*` in
  `context.rs` stay thin: history in, answer appended). Budget checks, events and their order,
  fallbacks and the streaming rule (fall back only before the first frame) moved unchanged;
  `request_json` still has no budget check. `forced_call_instruction`/`makes_call` moved along;
  `request_llm_with` (history changes) stays in `context.rs`. Facts, decision and the summary
  request still call `gateway().with_fallbacks(...)`/`llm_call_*` – they move in stage 5d. Unit tests
  use a small in-module `LlmClient` (grauwolf-test is for `tests/` only): fallback order and events,
  budget check before requests, no switch on budget errors.
- `context.rs`: 1,076 -> 781 lines (event bus and gateway together).

## Stage 5d: implementation notes (2026-11-12)
- `grauwolf-agents/src/facts.rs`: `extract_facts`, `request_facts`, `request_fact_lists` and the
  `FACT_*`/`DURABLE_FACTS` instructions as free functions over `LlmGateway` (usage booked through the
  gateway's accounting). The budget check stays in `AgentContext::extract_facts`.
- `compression.rs` got `compress(conversation, gateway, fact_memory, strategy)` next to the types and
  helpers it already had (summarize, keep last, facts incl. the durable-fact storage and its events);
  `compress_history` checks the budget and builds the gateway view from the fields, because the history
  is borrowed mutably at the same time.
- `decision.rs` got `decide_with_model(gateway, budget, decision, state)`. The budget result is passed
  in and consulted after the model name is resolved, so the order of error texts is unchanged.
- Moderation had already moved into `llm_gateway` in 5c; nothing was left to move.
- Unit tests with a small in-module `LlmClient`: facts (known facts named, durable list on request,
  invalid answer), compression (keep last, summarize, facts, nothing older), decision (verdict names the
  answering model, model name checked before the budget, answer outside the options).
- `context.rs`: 781 -> 556 lines (the target of 400 was not reached; what remains are the delegations,
  `execute_tool`/`call_tool`, `request_llm_with`, the structured retry loop and
  snapshot/restore/fork/apply/absorb, which combine several components).

## Stage 5: summary (2026-11-12)
- `context.rs`: 1,269 -> 556 lines; `AgentContext` keeps its public API, events, order and error texts.
- Components (private modules of `grauwolf-agents`): `accounting`, `storage`, `conversation`,
  `model_selection`, `tool_access`, `event_bus`, `llm_gateway`; feature modules: `facts`, `compression`,
  `decision` (moderation lives in the gateway).
- Tests: every component and feature module has in-module unit tests that need no `PromptExecutor` or
  use a small `LlmClient`; the existing workspace tests and equivalence tests ran unchanged.
- Remaining ideas, not needed now: move `execute_tool`/`call_tool` into a `tool_execution` feature module
  and `request_llm_with` into the gateway if `context.rs` is to go under 400 lines.
- **Done 2026-11-12:** `tool_execution.rs` with `ToolExecution` (a borrowed view on tool access, event
  bus, accounting and storage, like `LlmGateway`) runs one tool call; `AgentContext::execute_tool`
  delegates, `call_tool` moved with it. Order of the steps, events and error texts unchanged; four
  unit tests without a `PromptExecutor` (events around the call, error texts and section check,
  tool-call limit, state of stateful tools). `context.rs`: 556 -> 506 lines. `request_llm_with`
  stays (it combines the conversation with the gateway's forced-call fallback).

### Feature matrix in `check --full` (2026-11-12)
- **Problem:** the workspace build unifies features, so every optional feature is on and the code
  behind `#[cfg(not(feature = …))]` – the "this build has no …" errors of the host, the server
  without `ollama`, the packages fallback – was neither compiled nor tested by any check.
- **Decision:** `check --full` runs, after the workspace tests, clippy (`--all-targets`, `-D
  warnings`) and the fast tests of each build in `xtask::check::FEATURE_MATRIX`:
  `grauwolf-host` with `--no-default-features` and with only `builtin`, `grauwolf-server` and the
  facade `grauwolf` with `--no-default-features`. `grauwolf-cli` has no features. No external tool
  (`cargo hack`): the list is small and explicit, and a full powerset would cost far more build
  time and disk than it finds.
- **Integration tests** that need optional features declare them as `required-features` in
  `[[test]]` sections (15 in `grauwolf-host`, `chat_api_models` in the server, `macros` in the
  facade); before, they did not compile without the defaults. `check --affected` stays without
  the matrix (speed).
- **Cost:** about one minute on a warm build (four extra feature sets of the app crates).
