# Chapter 2 – Development setup and way of working

This chapter shows how to build and test grauwolf and which rules the project works by: mandatory
checks before every commit, test-driven development, ADRs, the `AGENTS.md` of each crate and the
feature catalog. If you only want to use the library, you mainly need sections 2.1 and 2.2; if you
work on the framework, you need the whole chapter.

## 2.1 Toolchain

You need:

- **Rust ≥ 1.85** (edition 2024, set in `Cargo.toml` via `rust-version = "1.85"`,
  [ADR-0011](../adr/0011-edition-msrv.md)).
- **The WASM target `wasm32-unknown-unknown`.** Tool code from specs is compiled for it
  ([Chapter 4](04-tools-internals.md)). Without the target, all tests that build tools fail.
- **A C linker and build tools** (`build-essential` on Debian/Ubuntu), because some dependencies
  contain native code.

```sh
rustup toolchain install stable
rustup target add wasm32-unknown-unknown
sudo apt-get install -y build-essential   # Debian/Ubuntu
```

For manual runs against a real model, a local Ollama is handy; for the tests you do **not** need
it – they run against mocks.

## 2.2 Building and testing

The whole workspace:

```sh
cargo build --workspace
cargo test --workspace
```

A single crate, a single test program or a single test:

```sh
cargo test -p grauwolf-interpreter                 # one crate
cargo test -p grauwolf-interpreter --test echo     # tests/echo.rs
cargo test -p grauwolf-interpreter --test echo runs_the_echo_spec   # one test case
```

You check the layer rules from [Chapter 1](01-architecture.md) on their own with:

```sh
cargo xtask check-arch
```

You start the CLI from the workspace with `cargo run -p grauwolf-cli -- …`, e.g.
`cargo run -p grauwolf-cli -- spec validate examples/specs/calculator.json`. All commands are
listed in the [CLI reference of the user guide](../user-guide/reference-cli.md).

`grauwolf-cli` is a library plus a thin binary: `src/main.rs` only parses the command line and
calls `grauwolf_cli::dispatch`; the commands live in `src/commands/` (one module per command, for
example `commands/spec/validate.rs`, `commands/tools.rs`), the shared option structs in
`src/options.rs`. Commands write through an `Io` handle (standard streams or captured buffers), so
`crates/apps/grauwolf-cli/tests/commands.rs` runs them in-process with `grauwolf_cli::run` and
checks their output; `tests/cli.rs` and `tests/tool_packages.rs` still start the real binary.

The tool commands (`tools build|pack|push|pull|inspect|lock|keygen|validate`) are not in this crate:
they live in `crates/apps/grauwolf-tools-app`, which also builds the small program `grauwolf-tools`
(`cargo run -p grauwolf-tools-app -- validate <package folder>`; ADR-0118). `commands/tools.rs`
only delegates, and `crates/apps/grauwolf-tools-app/tests/commands.rs` starts the `grauwolf-tools`
binary for every command. Keep that crate free of host, server and CLI dependencies: `cargo xtask
check-arch` and `xtask/tests/workspace_architecture.rs` fail otherwise.

### Tips for constrained machines

A full build with all tests is large (several GB). Three settings help:

| Setting | Effect |
|---|---|
| `CARGO_TARGET_DIR=~/grauwolf-target` | Build output outside the workspace, e.g. when the workspace is on a slow or small volume (container with a mounted directory). |
| `cargo test -j 4 …` | Fewer parallel compiler and linker processes; prevents the linker from being killed when memory is low (`signal 9`). |
| `CARGO_INCREMENTAL=0` | No incremental cache (`debug/incremental`); saves a lot of space with barely longer builds. |

The equivalence test (section 2.5) puts generated crates under
`$CARGO_TARGET_DIR/generated-equivalence` (otherwise `target/generated-equivalence`). The
directory can grow to several GB; you may delete it at any time after a test run, and the next run
rebuilds it.

### Test tiers

The tests run in three tiers:

| Tier | What | How it runs |
|---|---|---|
| fast | unit tests, adapter tests against mocks, in-process integration (about 940 tests, about 20 s of test run time) | `cargo test --workspace` |
| slow | tests that compile generated crates or WASM tool code with cargo, or start the CLI binary which does (about 30 tests, several minutes) | `cargo test --workspace -- --include-ignored` (only the slow ones: `-- --ignored`) |
| live | against real models and services, opt-in by environment variables (section 2.5) | as described there |

You mark a test as slow with a reason that says what makes it slow, and nothing else changes:

```rust
#[test]
#[ignore = "slow: builds tool code for WASM with cargo"]
fn builds_working_code() { … }
```

Mark a test only if it compiles with cargo, builds or starts a process that does, or takes
several seconds; do not mark fast tests, and never let a test rely on a slow test's side effects.
Plain `cargo test --workspace` skips the slow tier, so the full run before a push is the
`--include-ignored` one.

### `cargo xtask check`

The two checks of this section as one command, with a readable selection of what ran
([ADR-0108](../adr/0108-restructuring-for-separation-of-concerns.md)):

```sh
cargo xtask check --full       # fmt --check, clippy, all tests incl. the slow tier, feature matrix, layer rules
cargo xtask check --affected   # the same for what changed, fast tier only
cargo xtask check --affected --base main   # changed since main (plus working tree)
```

`--affected` takes the changed files from `git diff --name-only <base>` and `git status`, maps
them to packages, adds every package that depends on a changed one and prints the selection with
the reason. Changes to `Cargo.toml`, `Cargo.lock`, `.cargo/`, `xtask/` or `rust-toolchain*` select
the whole workspace; the examples of the guides select `grauwolf-host`; other documents run only the
format check. It stops at the first failing step and prints that step's output.

The rule (section 2.3): `--affected` before every commit, `--full` before every push and in CI. On
a warm build a change in a leaf crate takes seconds, one in `grauwolf-prompt` (below almost
everything) about two minutes.

## 2.3 Mandatory checks before every commit and push

Before **every** commit:

```sh
cargo fmt --all
cargo xtask check --affected
```

Before **every** push (and in CI):

```sh
cargo xtask check --full
```

- `cargo fmt` formats; changed files belong in the same commit.
- `--affected` runs `clippy -D warnings` (tests too, `--all-targets`) and the fast tests for the
  changed crates and every crate that depends on them, plus the architecture test; documents alone
  only get the format check.
- `--full` runs the format check, clippy and **all** tests of the workspace including the slow tier
  (`-- --include-ignored`), the architecture test (`xtask/tests/workspace_architecture.rs`) and the
  test of the user guide examples (section 2.8). Then it runs clippy and the fast tests of the
  **feature matrix** (`xtask::check::FEATURE_MATRIX`): `grauwolf-host` without features and with
  `builtin` only, `grauwolf-server` and the facade `grauwolf` without features. The workspace build
  unifies features, so only these builds compile the code behind `#[cfg(not(feature = …))]` (the
  "this build has no …" errors and their tests). An integration test that needs an optional feature
  declares it as `required-features` in a `[[test]]` section of its crate's `Cargo.toml`; otherwise
  the matrix fails to compile it.

All tests must be green. A red test is not fixed "later" but before the commit – even if it seems
to have nothing to do with your change.

## 2.4 Test-driven development

New functionality is developed test-first (TDD) in the cycle *red – green – refactor*:

1. **Red:** Write **one** test case that describes the desired behavior and watch it fail.
2. **Green:** Write the smallest code that makes it pass.
3. **Refactor:** Improve names and structure; the tests stay green.

Then make one commit with **test + implementation + docs** together. Docs means: doc comments,
the status in the crate's `AGENTS.md`, and the README or user guide if needed. One commit per
behavior – many small commits rather than a few large ones.

Tests check **behavior**, not implementation details. Good: "The agent returns the model's
answer", "invalid specs are rejected with a JSON pointer". Bad: "Function X is called", "the file
contains the text Y".

A typical test name describes the behavior as a sentence, and the file's header line names the
feature ID, e.g. `crates/domain/grauwolf-interpreter/tests/echo.rs` (the header line quoted here is
from that file):

```rust
//! Interpreter runs specs from the example corpus (F13.10).

#[test]
fn refuses_specs_that_do_not_validate() {
    // …
}
```

## 2.5 Test strategy

### Mock instead of a real LLM

Almost all tests run without a network against `MockLlmClient` from the test kit `grauwolf-test`.
The mock works with rules: if the last input contains a given text, it answers with a message or a
tool call. Based on `crates/domain/grauwolf-interpreter/tests/echo.rs` (shortened):

```rust
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_test::MockLlmClient;

let executor = PromptExecutor::builder()
    .client(
        "ollama",
        Arc::new(MockLlmClient::new("?").answer_when_contains("Hello world", "Hello world")),
    )
    .build();
let agent = Interpreter::new(Arc::new(executor))
    .load(&example("echo.json"))
    .unwrap();

let output = block_on(agent.run("Hello world")).unwrap();
assert_eq!(output, "Hello world");
```

Without a matching rule, `MockLlmClient::new(default)` answers with `default`. Further rules:
`answer_when_contains(needle, answer)`, `call_tool_when_contains(needle, tool, args)` and
`call_tools_when_contains(needle, [(tool, args), …])` for several tool calls in one answer.
The name under which you register the mock with the `PromptExecutor` must match the spec's
`model.provider`.

Domain tests need no tokio runtime: `futures_executor::block_on` is enough. Tests in adapters and
apps use `#[tokio::test]`.

### HTTP adapters against wiremock

You test provider adapters against a local HTTP mock with recorded responses. Based on
`crates/adapters/grauwolf-ollama/tests/chat.rs` (shortened):

```rust
let server = MockServer::start().await;
Mock::given(method("POST"))
    .and(path("/api/chat"))
    .respond_with(ResponseTemplate::new(200).set_body_json(json!({
        "model": "qwen3.5:9b",
        "message": { "role": "assistant", "content": "Hi" },
        "done": true
    })))
    .expect(1)
    .mount(&server)
    .await;
let client = OllamaClient::new(server.uri());
let prompt = Prompt::builder("p").system("Be brief.").user("Hello").build();

let answer = client.execute(&prompt, &model(), &[]).await.unwrap();
assert_eq!(answer, Message::assistant("Hi"));
```

### Conformance suite and live tests

`grauwolf_test::conformance::check_llm_client(&client, &model)` checks whether an `LlmClient`
behaves as expected (text, tool calls, …) and returns a list of `Failure`s. It only runs against
real models when the matching environment variable is set; otherwise the test skips itself
(`crates/adapters/grauwolf-ollama/tests/live.rs`):

```sh
GRAUWOLF_OLLAMA_URL=http://localhost:11434 GRAUWOLF_OLLAMA_MODEL=qwen3.5:9b \
  cargo test -p grauwolf-ollama --test live
GRAUWOLF_OPENAI_API_KEY=… cargo test -p grauwolf-openai --test live
GRAUWOLF_ANTHROPIC_API_KEY=… cargo test -p grauwolf-anthropic --test live
GRAUWOLF_GEMINI_API_KEY=… cargo test -p grauwolf-gemini --test live
AWS_BEARER_TOKEN_BEDROCK=… AWS_REGION=us-west-2 cargo test -p grauwolf-bedrock --test live
```

Optionally, `GRAUWOLF_<PROVIDER>_MODEL` chooses the model, and `GRAUWOLF_OPENAI_BASE_URL` points
the OpenAI test at a compatible service. The Bedrock test takes the standard AWS credentials
(`AWS_BEARER_TOKEN_BEDROCK`, or `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`) and
`GRAUWOLF_BEDROCK_MODEL`; without credentials it is skipped.

Live tests are optional and have no effect in `cargo test --workspace` – without the variable
they pass immediately.

### Equivalence tests

`crates/domain/grauwolf-generator/tests/equivalence.rs` runs every spec of the corpus
`examples/specs/` once in the interpreter and once as a generated crate against the same mock
and compares the outputs. If you add a node kind, a condition or a spec field, add a case there
(`Case { spec, input, expect, mock }`). More on this in [Chapter 6](06-generator.md).

### Temporary directories

Tests never build directories in the system temp dir by hand (`std::env::temp_dir().join(…)`):
such directories are never removed and pile up by the thousands. Use `grauwolf_test::TestDir`
instead (dev-dependency `grauwolf-test`):

```rust
let dir = grauwolf_test::TestDir::new("chat-history"); // /tmp/grauwolf-chat-history-<pid>-<n>
std::fs::write(dir.join("spec.json"), "{}").unwrap();
// `dir` derefs to `Path`; the directory and its content are removed when `dir` is dropped.
```

The name is unique per test (process id and counter), so parallel tests and parallel test
processes never share a directory. Keep the guard alive for the whole test, also while a
background task (a served agent, a mock server) uses the path, and use one guard where a test
checks that data survives from one instance to the next. A helper that returns only a path
derived from a local guard would remove the directory on return: return the guard itself.

### Other kinds of tests

- **Snapshot tests** with `insta` for generated code (`crates/domain/grauwolf-generator/tests/snapshots`).
- **Catalog conformance**: for every node kind and condition in the catalog there is an
  implementation (`crates/domain/grauwolf-agents/tests/catalog_conformance.rs`) and a generator
  template (`crates/domain/grauwolf-generator/tests/catalog_conformance.rs`).
- **Doc tests**, e.g. the `compile_fail` test on `grauwolf_graph::TypedGraphBuilder::edge`, which
  shows that wrongly typed edges do not compile.

## 2.6 Recording architecture decisions

Every design decision that has effects beyond one crate is recorded as an **ADR** (Architecture
Decision Record) under `docs/adr/`. Read the affected ADRs before you restructure anything. A new
decision gets the next free number and the format below. ADRs are written in **English**
([ADR-0091](../adr/0091-english-everywhere.md)) with the headings `Status`, `Date`, `Context`,
`Decision` and `Consequences`; the status is `Proposed` or `Accepted`.

```markdown
# ADR-0092: Short title

- **Status:** Proposed
- **Date:** 2026-11-10

## Context
Which problem, which constraints?

## Decision
What is done – concretely, with names of types, fields, crates.

## Consequences
What follows, what gets harder, what remains open.
```

Add the ADR to the table in [docs/adr/README.md](../adr/README.md). Superseded ADRs are not
deleted but set to `Superseded by ADR-XXXX`.

## 2.7 `AGENTS.md` per crate

Every crate has an `AGENTS.md` (with a `CLAUDE.md` symlink). It is the crate's compact contract
for humans and AI agents ([ADR-0019](../adr/0019-ai-guardrails.md)):

- **Layer and status** – what the crate can do today, with the most important types and functions.
- **Purpose** – one sentence.
- **Features / ADRs** – related feature IDs and decisions.
- **Allowed dependencies** – grauwolf crates and external crates.
- **Invariants** – what must never be violated; those not yet implemented are marked as "goal".
- **Tests** and **Working on this crate**.

If the purpose, API, dependencies or invariants change, you update the `AGENTS.md` in the same
commit. You only add a new dependency after checking it against
[ADR-0016](../adr/0016-layered-architecture.md); the architecture test checks the layers, but
nobody checks the finer list in the `AGENTS.md` automatically.

## 2.8 Feature catalog and examples

The [feature catalog](../features/README.md) lists all functionality in epics (`E03` tools,
`E13` generative agents, …) with IDs such as `F03.5` (`#[tool]`) or `F13.10` (interpreter). Each
ID belongs to exactly one crate. Mention them in doc comments and in the header line of test
files, so that functionality, code and tests can be found together.

The example specs of the user guide under `docs/user-guide/examples/*.json` are checked on every
`cargo test --workspace` (`crates/apps/grauwolf-host/tests/docs_examples.rs`): every file must load
and validate with `grauwolf::loader::load_spec`. If you change the spec format, you notice right
away when the user guide gets out of date.

## 2.9 Language

The language rules follow [ADR-0091](../adr/0091-english-everywhere.md):

- **English** – everything in the repository: `README.md`, the user and developer guides and
  their examples, ADRs, `docs/architecture.md`, the feature catalog, code comments and doc
  comments, the `AGENTS.md` of the crates, test data, every message of the CLI and the library
  (error messages, validation errors, CLI output, tool results sent to the model), and the
  built-in instructions to the model (compression, facts, moderation, verification, planner,
  builder, …). Their wording influences the results of small models, so check changes to them
  with a live run (for example against a local Ollama).
- **German** – only `docs/handbuch/` and `docs/entwickler/`, translated from the English state of
  2026-11-15 and updated only when needed.
- **Identifiers in code in English** – types, functions, fields, test names.

```rust
/// Limits every run of every loaded agent to `budget` (ADR-0072).
pub fn with_budget(mut self, budget: grauwolf_agents::Budget) -> Self { … }
```

Public items need doc comments. So far this is only enforced in `grauwolf-codegen`
(`#![deny(missing_docs)]`); in all other crates it is a convention.

## 2.10 Cutting a release

Only the owner cuts a release and sets the tag; a release is a tag `v<version>` on `main`. Images
(`.github/workflows/images.yml`, ADR-0096) are published only from a tag.

1. **Bump the version** in `[workspace.package]` of `Cargo.toml` (every crate inherits it) and run
   `cargo check --workspace` so that `Cargo.lock` follows. The version is the contract between the
   CLI, the builder image and generated crates: the builder's `GRAUWOLF_VERSION` must equal the
   CLI's version, or the generated `Dockerfile` refuses to build.
2. **Full check:** `cargo xtask check --full` (the whole workspace including the slow tests).
   Commit the bump, merge it to `main`.
3. **Tag:** `git tag v<version>` on that commit and `git push origin v<version>`. The workflow
   fails at once when the tag and the workspace version differ.
4. **What CI publishes** (private, `ghcr.io/agentsw0rk`): after the smoke tests, the CLI image
   `grauwolf`, `grauwolf-tools` and the builder image `grauwolf-builder`, each built natively for
   `linux/amd64` (`ubuntu-24.04`) and `linux/arm64` (`ubuntu-24.04-arm`), pushed per architecture
   by digest and merged into one multi-arch tag `:<version>` (`:latest` for the CLI and the tools
   image, not for the builder). The builder job also runs the offline agent builds, so the version
   guard is proven positively on both architectures before anything is pushed. The builder job
   pushes its tested image under the helper tag `:<version>-<arch>` (the classic image store cannot
   push without a name); only the digests are used.
5. **Check:** `docker buildx imagetools inspect ghcr.io/agentsw0rk/grauwolf-builder:<version>`
   lists both platforms.

The repository's token needs `packages: write` (set in the workflow); the label
`org.opencontainers.image.source` connects each package to the repository, so the package
inherits the repository's access. A package that exists already may need the repository added
under its "Manage Actions access" setting.

**Pulling private images:** `docker login ghcr.io` with a personal access token that has
`read:packages`. A cluster (AgentWeaver's, which pulls the builder image in its kaniko builds and
the CLI image for its backend) needs an image pull secret for `ghcr.io` with such a token.
Native arm64 runners are free for public repositories only; for a private one a GitHub plan with
arm64 (larger) runners may be needed. The arm64 jobs therefore run only on tags and manual runs
(`workflow_dispatch`) – start one by hand before the first release tag to see whether the plan has
such runners. The workflow runs on `pull_request` and pushes to `main` (amd64 only) without
publishing; it cannot be run locally (no Docker in the development container), so the
first run after a change is the real test.

## Key points

- Rust ≥ 1.85, `rustup target add wasm32-unknown-unknown`, build tools.
- Before every commit: `cargo fmt --all` and `cargo xtask check --affected`; before every push:
  `cargo xtask check --full` (section 2.3).
- TDD: one test case per cycle; commit = test + code + docs.
- Tests run against `MockLlmClient` and wiremock; live tests only with an environment variable;
  equivalence tests keep interpreter and generator in line.
- Decisions as ADRs, crate contracts in `AGENTS.md`, feature IDs in doc comments and tests.
- Fast tier `cargo test --workspace`, slow tier with `-- --include-ignored`;
  `cargo xtask check --affected` before commits, `--full` before pushes (section 2.3).
- Low on space or memory: `CARGO_TARGET_DIR` outside the workspace, `-j 4`, `CARGO_INCREMENTAL=0`,
  delete `generated-equivalence` after the run.

Continue with [Chapter 3 – grauwolf as a library](03-rust-api.md).
